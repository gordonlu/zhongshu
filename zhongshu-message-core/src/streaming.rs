use serde::{Deserialize, Serialize};

use crate::block::{BlockTree, MessageBlock};
use crate::parser;

/// Lifecycle state of a streaming message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageState {
    Streaming,
    Complete,
    Interrupted,
    Failed,
}

/// Known control token prefixes (case-insensitive).
const CONTROL_PREFIXES: &[&str] = &[
    "<final_answer",
    "</final_answer",
    "<final-answer",
    "</final-answer",
    "<observation",
    "</observation",
];

/// Filters agent protocol control tokens from the raw LLM stream
/// before they enter the message buffer.
///
/// Known control tokens:
///   `<final_answer>`, `</final_answer>`
///   `<final-answer>`, `</final-answer>`
///   `<observation ...>`, `</observation>`
///
/// Handles split tokens across chunk boundaries via a pending buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlTokenFilter {
    #[serde(skip)]
    pending: String,
}

impl ControlTokenFilter {
    pub fn new() -> Self {
        Self { pending: String::new() }
    }

    /// Feed a text delta through the filter.
    /// Returns sanitized text (control tokens removed).
    /// Incomplete control tokens are buffered in `pending` and
    /// completed on subsequent calls to `feed()`.
    pub fn feed(&mut self, delta: &str) -> String {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(delta);

        let mut output = String::with_capacity(input.len());
        let mut chars = input.char_indices().peekable();

        while let Some((i, ch)) = chars.next() {
            if ch != '<' {
                output.push(ch);
                continue;
            }

            let rest = &input[i..];
            let lower = rest.to_lowercase();

            let is_control = lower.starts_with("<final_answer")
                || lower.starts_with("</final_answer")
                || lower.starts_with("<final-answer")
                || lower.starts_with("</final-answer")
                || lower.starts_with("<observation")
                || lower.starts_with("</observation");

            if !is_control {
                // Check if this `<` could be the start of an incomplete
                // control token split across chunks (e.g. `</` + `final_answer>`).
                let is_prefix = CONTROL_PREFIXES.iter().any(|p| p.starts_with(&lower));
                if is_prefix {
                    self.pending = rest.to_string();
                    break;
                }
                // `<` at end of input — also defer
                if chars.peek().is_none() {
                    self.pending.push('<');
                    break;
                }
                output.push(ch);
                continue;
            }

            // Try to consume the complete tag: text up to and including '>'
            let mut tag = String::from("<");
            let mut found_close = false;
            for (_, c) in chars.by_ref() {
                tag.push(c);
                if c == '>' {
                    found_close = true;
                    break;
                }
            }

            if found_close {
                // Control tokens are block-level — insert paragraph break
                // so following text starts on a new line.
                if !output.is_empty() {
                    if let Some(&(_, next_ch)) = chars.peek() {
                        if next_ch != '\n' && next_ch != ' ' && next_ch != '\t' {
                            output.push('\n');
                        }
                    }
                }
                continue;
            }

            // Incomplete tag — buffer for next delta
            self.pending = tag;
            break;
        }

        output
    }

    /// Discard any uncompleted pending data.
    /// At stream end there is no way to confirm an incomplete tag is
    /// actually a control token, so we drop it rather than leaking it
    /// into the message buffer.
    pub fn flush(&mut self) -> String {
        self.pending.clear();
        String::new()
    }
}

impl Default for ControlTokenFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip all known control tokens from `input` in one pass.
pub fn strip_control_tokens(input: &str) -> String {
    let mut filter = ControlTokenFilter::new();
    let out = filter.feed(input);
    // pending buffer discarded (flush clears it)
    out
}

/// A streaming-aware message that accumulates raw text and
/// incrementally builds a block tree.
///
/// Text deltas go through `ControlTokenFilter` first to strip
/// agent protocol control tokens before they reach the markdown parser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingMessage {
    /// Control token filter state.
    #[serde(skip)]
    filter: ControlTokenFilter,
    /// Raw accumulated text (after filtering).
    pub buffer: String,
    /// Parsed block tree from the buffer.
    pub blocks: BlockTree,
    /// Lifecycle state.
    pub state: MessageState,
}

impl StreamingMessage {
    pub fn new() -> Self {
        StreamingMessage {
            filter: ControlTokenFilter::new(),
            buffer: String::new(),
            blocks: BlockTree::empty(),
            state: MessageState::Streaming,
        }
    }

    /// Append new text and re-parse into blocks.
    /// Control tokens are stripped before the text enters the buffer.
    /// Returns the index of the first block that may have changed.
    pub fn append(&mut self, delta: &str) -> usize {
        let cleaned = self.filter.feed(delta);
        if cleaned.trim().is_empty() {
            if !delta.trim().is_empty() {
                tracing::debug!(original = %delta, "append: cleaned was all whitespace");
            }
            return self.blocks.len();
        }
        tracing::debug!(original_len = delta.len(), cleaned_len = cleaned.len(), cleaned_preview = %cleaned.chars().take(120).collect::<String>(), "append");
        self.buffer.push_str(&cleaned);
        self.sync()
    }

    /// Re-parse the buffer into blocks.
    /// Returns the index of the first block that may have changed.
    pub fn sync(&mut self) -> usize {
        if self.buffer.is_empty() {
            return 0;
        }

        let new_tree = parser::parse(&self.buffer);
        let divergence = find_divergence(&self.blocks.blocks, new_tree.blocks.as_slice());
        self.blocks = new_tree;
        divergence.min(self.blocks.len())
    }

    /// Mark the message as complete. Any unclosed control tokens
    /// in the filter buffer are discarded.
    pub fn complete(&mut self) {
        self.filter.flush();
        self.state = MessageState::Complete;
        self.sync();
    }

    pub fn interrupt(&mut self) {
        self.state = MessageState::Interrupted;
    }

    pub fn fail(&mut self) {
        self.state = MessageState::Failed;
    }

    /// Replace the entire content (for setting from history).
    /// Does NOT run the filter (history text should already be clean).
    pub fn set_content(&mut self, text: &str) {
        self.buffer = text.to_string();
        self.state = MessageState::Complete;
        self.blocks = parser::parse(text);
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty() && self.blocks.is_empty()
    }

    pub fn blocks(&self) -> &[MessageBlock] {
        self.blocks.blocks.as_slice()
    }
}

impl Default for StreamingMessage {
    fn default() -> Self {
        Self::new()
    }
}

/// Find the first index where two block slices diverge.
fn find_divergence(old: &[MessageBlock], new: &[MessageBlock]) -> usize {
    let min_len = old.len().min(new.len());
    for i in 0..min_len {
        if old[i] != new[i] {
            return i;
        }
    }
    min_len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_filter_removes_final_answer() {
        let mut msg = StreamingMessage::new();
        msg.append("Hello");
        msg.append("<final_answer>World");
        msg.append("</final_answer>");
        assert_eq!(msg.buffer, "HelloWorld");
    }

    #[test]
    fn test_control_filter_split_token() {
        let mut msg = StreamingMessage::new();
        msg.append("Hello");
        msg.append("</final_answer");  // split: still starts with </final_answer → buffered
        msg.append(">");               // completes the tag → stripped
        assert_eq!(msg.buffer, "Hello");
    }

    #[test]
    fn test_control_filter_split_close_tag_across_chunks() {
        let mut msg = StreamingMessage::new();
        msg.append("正文。</");           // `</` deferred as control prefix
        msg.append("final_answer>");     // completes → stripped
        assert_eq!(msg.buffer, "正文。");
    }

    #[test]
    fn test_control_filter_preserves_non_control() {
        let mut msg = StreamingMessage::new();
        msg.append("hello <world>");
        assert!(msg.buffer.contains("<world>"));
    }

    #[test]
    fn test_append_basic_paragraph() {
        let mut msg = StreamingMessage::new();
        msg.append("Hello world");
        assert!(!msg.blocks.is_empty());
        assert!(matches!(msg.blocks.blocks[0], MessageBlock::Paragraph { .. }));
    }

    #[test]
    fn test_append_code_block() {
        let mut msg = StreamingMessage::new();
        msg.append("Some code:\n```rust\nfn main() {}\n```");
        assert!(msg.blocks.len() >= 2);
        let code_block = msg.blocks.blocks.iter().find(|b| matches!(b, MessageBlock::Code { .. }));
        assert!(code_block.is_some());
        if let Some(MessageBlock::Code { language, content }) = code_block {
            assert_eq!(language.as_deref(), Some("rust"));
            assert!(content.contains("fn main()"));
        }
    }

    #[test]
    fn test_complete_state() {
        let mut msg = StreamingMessage::new();
        msg.append("hello");
        msg.complete();
        assert_eq!(msg.state, MessageState::Complete);
    }

    #[test]
    fn test_observation_table_separated() {
        let mut msg = StreamingMessage::new();
        msg.append("<observation tool=\"system_info\">\n{\n  \"os\": \"Linux\"\n}\n</observation>\n\n| 类别 | 项目 | 值 |\n|---|---|---|\n| CPU | AMD | x64 |\n| RAM | 32G | DDR5 |");
        assert!(!msg.buffer.contains("<observation"), "observation tags stripped");
        assert!(msg.buffer.contains("CPU"), "table data preserved");
        let table = msg.blocks.blocks.iter().find(|b| matches!(b, MessageBlock::Table { .. }));
        assert!(table.is_some(), "should have a table block");
        if let Some(MessageBlock::Table { headers, rows }) = table {
            assert_eq!(headers.len(), 3);
            assert_eq!(rows.len(), 2, "should have 2 data rows");
        }
    }

    #[test]
    fn test_streaming_append_diverges_last_block() {
        let mut msg = StreamingMessage::new();
        msg.append("First paragraph");
        assert_eq!(msg.blocks.len(), 1);

        msg.append("\n\nSecond paragraph");
        assert_eq!(msg.blocks.len(), 2);
    }

    #[test]
    fn test_divergence_from_old_blocks() {
        let old = vec![
            MessageBlock::Paragraph { text: "Hello".into() },
            MessageBlock::Paragraph { text: "World".into() },
        ];
        let new = vec![
            MessageBlock::Paragraph { text: "Hello".into() },
            MessageBlock::Paragraph { text: "Universe".into() },
        ];
        assert_eq!(find_divergence(&old, &new), 1);
    }

    #[test]
    fn test_divergence_same() {
        let old = vec![MessageBlock::Paragraph { text: "Hello".into() }];
        let new = vec![MessageBlock::Paragraph { text: "Hello".into() }];
        assert_eq!(find_divergence(&old, &new), 1);
    }

    #[test]
    fn test_heading_block() {
        let mut msg = StreamingMessage::new();
        msg.append("# Title\n\nSome text");
        let blocks = &msg.blocks.blocks;
        assert!(blocks.len() >= 2);
        assert!(matches!(blocks[0], MessageBlock::Heading { level: 1, .. }));
    }

    #[test]
    fn test_list_block() {
        let mut msg = StreamingMessage::new();
        msg.append("- item 1\n- item 2\n- item 3");
        let has_list = msg.blocks.blocks.iter().any(|b| matches!(b, MessageBlock::List { .. }));
        assert!(has_list);
    }

    #[test]
    fn test_blockquote() {
        let mut msg = StreamingMessage::new();
        msg.append("> quoted text");
        let has_quote = msg.blocks.blocks.iter().any(|b| matches!(b, MessageBlock::Blockquote { .. }));
        assert!(has_quote);
    }

    #[test]
    fn test_set_content() {
        let mut msg = StreamingMessage::new();
        msg.set_content("# Fixed\n\nContent");
        assert_eq!(msg.state, MessageState::Complete);
        assert!(!msg.blocks.is_empty());
    }

    #[test]
    fn test_control_filter_preserves_angle_brackets_in_text() {
        let mut msg = StreamingMessage::new();
        msg.append("a < b and c > d");
        assert_eq!(msg.buffer, "a < b and c > d");
    }

    #[test]
    fn test_reduce_responses_smoke() {
        // Simulate the actual LLM streaming output: tool call + response with control tokens
        let deltas = [
            "<observation",
            " tool=\"system_info\"",
            ">",
            "\n{\n  \"os\": \"Linux\"\n}\n",
            "</observation",
            ">",
            "\n\n",
            "你好。请问有什么需要处理的？",
            "<final_answer>",
            "有什么需要处理的？",
            "</final_answer>",
        ];
        let mut filter = super::ControlTokenFilter::new();
        let mut combined = String::new();
        for d in &deltas {
            let cleaned = filter.feed(d);
            combined.push_str(&cleaned);
        }
        filter.flush();
        assert!(!combined.contains("<observation"));
        assert!(!combined.contains("</observation"));
        assert!(!combined.contains("<final_answer"));
        assert!(!combined.contains("</final_answer"));
        assert!(combined.contains("你好"));
        assert!(combined.contains("有什么需要处理的"));
        assert!(!combined.contains("<"), "bare '<' leaked: '{combined}'");
        assert!(!combined.contains(">"), "bare '>' leaked: '{combined}'");
        println!("SMOKE OK: \"{combined}\"");
    }

    #[test]
    fn test_reduce_responses_actual_deltas() {
        // Exact split from the user's log: <final_answer>你好，说事。</final_answer>
        // Log shows: < | final | _answer | > | 你好 | 。 | 请问 | ... | </ | final | _answer | >
        let deltas = [
            "<",         // preview=<
            "final",     // preview=final
            "_answer",   // preview=_answer
            ">",         // preview=>
            "你好",      // 你好
            "。",        // 。
            "请问",
            "有什么",
            "需要",
            "处理的",
            "？",
            "</",        // preview=</
            "final",     // preview=final
            "_answer",   // preview=_answer
            ">",         // preview=>
        ];
        let mut filter = super::ControlTokenFilter::new();
        let mut combined = String::new();
        for d in &deltas {
            let cleaned = filter.feed(d);
            combined.push_str(&cleaned);
        }
        filter.flush();
        assert!(!combined.contains("<final_answer"), "tag leaked");
        assert!(!combined.contains("</final_answer"), "close tag leaked");
        assert!(!combined.contains("<"), "bare '<' leaked: '{combined}'");
        assert!(!combined.contains(">"), "bare '>' leaked: '{combined}'");
        assert!(combined.contains("你好"), "content preserved");
        assert!(combined.contains("请问有什么需要处理的"), "full content preserved");
        println!("ACTUAL OK: \"{combined}\"");
    }

    #[test]
    fn test_control_filter_final_answer_variants() {
        let mut msg = StreamingMessage::new();
        msg.append("<final-answer>ok");
        msg.append("</final-answer>");
        assert_eq!(msg.buffer, "ok");
    }

    #[test]
    fn test_control_filter_split_at_less_than() {
        let mut msg = StreamingMessage::new();
        msg.append("hello<");           // `<` at chunk boundary — deferred to pending
        msg.append("final_answer>world"); // completes the tag → stripped
        msg.append("</final_answer>");
        assert_eq!(msg.buffer, "helloworld");
    }

    #[test]
    fn test_control_filter_less_than_preserved_when_not_control() {
        let mut msg = StreamingMessage::new();
        msg.append("a <");              // `<` at boundary, deferred
        msg.append("b> c");            // not a control token → released
        assert_eq!(msg.buffer, "a <b> c");
    }

    #[test]
    fn test_control_filter_narrow_patterns_preserves_finally() {
        let mut msg = StreamingMessage::new();
        msg.append("<finally, the answer is 42>");
        assert_eq!(msg.buffer, "<finally, the answer is 42>");
    }

    #[test]
    fn test_complete_discards_unclosed_pending() {
        let mut msg = StreamingMessage::new();
        msg.append("Hello");
        msg.append("</final_answer");  // unclosed — goes to pending
        msg.complete();                // flush discards pending
        assert_eq!(msg.buffer, "Hello");
        assert_eq!(msg.state, MessageState::Complete);
    }

    #[test]
    fn test_append_skips_whitespace_after_control() {
        let mut msg = StreamingMessage::new();
        msg.append("Hello");
        msg.append("<final_answer>World");
        msg.append("</final_answer>");
        msg.append("\n\n");  // trailing whitespace
        assert_eq!(msg.buffer, "HelloWorld");
    }

    #[test]
    fn test_control_filter_partial_at_start() {
        let mut msg = StreamingMessage::new();
        msg.append("</final_answer");  // starts with </final_answer → buffered
        msg.append(">rest");           // tag completed → stripped, "rest" passes through
        assert_eq!(msg.buffer, "rest");
    }
}
