use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::block::{BlockTree, MessageBlock};

/// Parse markdown text into a block tree.
///
/// HTML is disabled at the parser level: `Event::Html` is treated as
/// plain text and merged into paragraphs. This prevents HTML blocks
/// (like `<observation>`) from consuming subsequent markdown content.
pub fn parse(text: &str) -> BlockTree {
    tracing::debug!(len = text.len(), preview = %text.chars().take(200).collect::<String>(), "parser input");
    let parser = Parser::new_ext(text, markdown_options());
    let mut blocks: Vec<MessageBlock> = Vec::new();

    // Accumulators for progressive block building
    let mut paragraph_text = String::new();
    let mut code_language: Option<String> = None;
    let mut code_content = String::new();
    let mut heading_text = String::new();
    let mut heading_level: u8 = 0;
    let mut list_items: Vec<String> = Vec::new();
    let mut list_ordered = false;
    let mut quote_text = String::new();
    let mut in_paragraph = false;
    let mut in_code = false;
    let mut in_heading = false;
    let mut in_list = false;
    let mut in_item = false;
    let mut in_quote = false;
    let mut in_table = false;
    let mut table_headers: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut table_row: Vec<String> = Vec::new();
    let mut table_cell = String::new();

    let flush_paragraph = |blocks: &mut Vec<MessageBlock>, text: &mut String| {
        if !text.is_empty() {
            let t = std::mem::take(text);
            blocks.push(MessageBlock::Paragraph { text: t });
        }
    };

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    in_paragraph = true;
                }
                Tag::CodeBlock(kind) => {
                    in_code = true;
                    code_language = match kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                        _ => None,
                    };
                    code_content.clear();
                }
                Tag::Heading { level, .. } => {
                    in_heading = true;
                    heading_text.clear();
                    heading_level = match level {
                        pulldown_cmark::HeadingLevel::H1 => 1,
                        pulldown_cmark::HeadingLevel::H2 => 2,
                        pulldown_cmark::HeadingLevel::H3 => 3,
                        pulldown_cmark::HeadingLevel::H4 => 4,
                        pulldown_cmark::HeadingLevel::H5 => 5,
                        pulldown_cmark::HeadingLevel::H6 => 6,
                    };
                }
                Tag::List(ordered) => {
                    in_list = true;
                    list_items.clear();
                    list_ordered = ordered.map_or(false, |_| true);
                }
                Tag::Item => {
                    in_item = true;
                }
                Tag::BlockQuote(_) => {
                    in_quote = true;
                    quote_text.clear();
                }
                Tag::Table(_) => {
                    in_table = true;
                    table_headers.clear();
                    table_rows.clear();
                }
                Tag::TableHead => {
                    table_row.clear();
                }
                Tag::TableCell => {
                    table_cell.clear();
                }
                Tag::TableRow => {
                    table_row.clear();
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if in_paragraph {
                        flush_paragraph(&mut blocks, &mut paragraph_text);
                        in_paragraph = false;
                    }
                }
                TagEnd::CodeBlock => {
                    if in_code {
                        blocks.push(MessageBlock::Code {
                            language: code_language.take(),
                            content: std::mem::take(&mut code_content),
                        });
                        in_code = false;
                    }
                }
                TagEnd::Heading(_) => {
                    if in_heading {
                        blocks.push(MessageBlock::Heading {
                            level: heading_level,
                            text: std::mem::take(&mut heading_text),
                        });
                        in_heading = false;
                    }
                }
                TagEnd::List(_) => {
                    if in_list {
                        blocks.push(MessageBlock::List {
                            ordered: list_ordered,
                            items: std::mem::take(&mut list_items),
                        });
                        in_list = false;
                        in_item = false;
                    }
                }
                TagEnd::Item => {
                    in_item = false;
                }
                TagEnd::BlockQuote(_) => {
                    if in_quote {
                        blocks.push(MessageBlock::Blockquote {
                            text: std::mem::take(&mut quote_text),
                        });
                        in_quote = false;
                    }
                }
                TagEnd::Table => {
                    if in_table {
                        blocks.push(MessageBlock::Table {
                            headers: std::mem::take(&mut table_headers),
                            rows: std::mem::take(&mut table_rows),
                        });
                        in_table = false;
                    }
                }
                TagEnd::TableHead => {
                    table_headers = std::mem::take(&mut table_row);
                }
                TagEnd::TableCell => {
                    table_row.push(std::mem::take(&mut table_cell));
                }
                TagEnd::TableRow => {
                    if in_table {
                        table_rows.push(std::mem::take(&mut table_row));
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_code {
                    code_content.push_str(&t);
                } else if in_heading {
                    heading_text.push_str(&t);
                } else if in_item {
                    if list_items.is_empty() {
                        list_items.push(t.to_string());
                    } else if let Some(last) = list_items.last_mut() {
                        last.push_str(&t);
                    }
                } else if in_quote {
                    quote_text.push_str(&t);
                } else if in_table {
                    table_cell.push_str(&t);
                } else {
                    paragraph_text.push_str(&t);
                    in_paragraph = true;
                }
            }
            Event::SoftBreak => {
                if in_code {
                    code_content.push('\n');
                } else if in_paragraph {
                    paragraph_text.push(' ');
                } else if in_heading {
                    heading_text.push(' ');
                } else if in_item {
                    if let Some(last) = list_items.last_mut() {
                        last.push(' ');
                    }
                } else if in_quote {
                    quote_text.push(' ');
                }
            }
            Event::HardBreak => {
                if in_code {
                    code_content.push('\n');
                } else if in_paragraph {
                    paragraph_text.push('\n');
                } else if in_item {
                    if let Some(last) = list_items.last_mut() {
                        last.push('\n');
                    }
                }
            }
            // HTML is treated as plain text (HTML disabled in options, but
            // pulldown_cmark may still produce some HTML events for entity
            // references or raw HTML-like constructs).
            Event::Html(t) | Event::InlineHtml(t) => {
                if in_code {
                    code_content.push_str(&t);
                } else if in_paragraph {
                    paragraph_text.push_str(&t);
                } else {
                    paragraph_text.push_str(&t);
                    in_paragraph = true;
                }
            }
            Event::Code(t) => {
                if in_paragraph {
                    paragraph_text.push_str("`");
                    paragraph_text.push_str(&t);
                    paragraph_text.push_str("`");
                } else if in_heading {
                    heading_text.push_str(&t);
                } else if in_item {
                    if let Some(last) = list_items.last_mut() {
                        last.push_str(&t);
                    }
                }
            }
            Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Rule
            | Event::TaskListMarker(_) => {}
        }
    }

    // Flush remaining paragraph
    if in_paragraph {
        flush_paragraph(&mut blocks, &mut paragraph_text);
    }

    BlockTree::new(blocks)
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_paragraph() {
        let tree = parse("Hello world");
        assert_eq!(tree.blocks.len(), 1);
        assert!(matches!(tree.blocks[0], MessageBlock::Paragraph { .. }));
    }

    #[test]
    fn test_parse_code_block() {
        let tree = parse("```rust\nfn main() {}\n```");
        assert!(tree.blocks.iter().any(|b| matches!(b, MessageBlock::Code { .. })));
    }

    #[test]
    fn test_parse_heading() {
        let tree = parse("# Title");
        assert!(matches!(tree.blocks[0], MessageBlock::Heading { level: 1, .. }));
    }

    #[test]
    fn test_parse_list() {
        let tree = parse("- a\n- b");
        assert!(tree.blocks.iter().any(|b| matches!(b, MessageBlock::List { .. })));
    }

    #[test]
    fn test_parse_blockquote() {
        let tree = parse("> quote");
        assert!(tree.blocks.iter().any(|b| matches!(b, MessageBlock::Blockquote { .. })));
    }

    #[test]
    fn test_observation_tag_does_not_eat_content() {
        // Without HTML blocks, `<observation>` should not consume
        // the rest of the content.
        let tree = parse("<observation tool=\"sys\">\n{json}\n</observation>\n\nAnswer.");
        let text: String = tree.blocks.iter()
            .map(|b| match b {
                MessageBlock::Paragraph { text } => text.clone(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("<observation"));
        assert!(text.contains("Answer"));
    }

    #[test]
    fn test_markdown_formatting_preserved() {
        let tree = parse("**bold** and *italic* and `code`");
        assert!(!tree.blocks.is_empty());
    }

    #[test]
    fn test_table() {
        let tree = parse("|a|b|\n|---|---|\n|1|2|");
        assert!(tree.blocks.iter().any(|b| matches!(b, MessageBlock::Table { .. })));
    }

    #[test]
    fn test_empty_input() {
        let tree = parse("");
        assert!(tree.blocks.is_empty());
    }

    #[test]
    fn test_html_does_not_consume() {
        // Without HTML blocks, <tag> should not eat following content
        let tree = parse("<example>hello</example>\n\n# heading");
        assert!(tree.blocks.iter().any(|b| matches!(b, MessageBlock::Heading { .. })));
    }
}
