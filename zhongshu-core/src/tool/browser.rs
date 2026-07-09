use crate::tool::{
    acquire_http_permit, build_browser_client, decode_html, detect_security_page, human_delay,
    sanitize_web_content, Tool, ToolOutput,
};
use async_trait::async_trait;
use serde_json::json;

pub struct BrowserTool;

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }
    fn description(&self) -> &str {
        "Fetch a web page and return its text content. Simpler than browser_automation — no JavaScript execution. Optionally opens page in your browser."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The URL to read"},
                "open_browser": {"type": "boolean", "description": "Also open in your default browser so you can see it (default false)", "default": false},
                "max_length": {"type": "integer", "description": "Max characters to return (default 5000)", "default": 5000},
                "raw": {"type": "boolean", "description": "Return raw HTML source instead of stripped text (default false)", "default": false}
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let url = match arguments["url"].as_str() {
            Some(u) => u,
            None => return ToolOutput::error("'url' must be a string"),
        };
        let max_len = arguments["max_length"].as_u64().unwrap_or(5000).min(20000) as usize;
        let open_browser = arguments["open_browser"].as_bool().unwrap_or(false);

        let client = match build_browser_client() {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("HTTP 客户端创建失败: {e}")),
        };

        human_delay().await;
        let _permit = acquire_http_permit().await;

        let html = match client.get(url).send().await {
            Ok(r) => {
                let ct = r
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok().map(|s| s.to_owned()));
                let bytes = match r.bytes().await {
                    Ok(b) => b,
                    Err(e) => return ToolOutput::error(format!("读取响应失败: {e}")),
                };
                decode_html(&bytes, ct.as_deref())
            }
            Err(e) => return ToolOutput::error(format!("请求失败: {e}")),
        };

        // Check for anti-bot / captcha pages before processing.
        if let Some(reason) = detect_security_page(&html) {
            if open_browser {
                let _ = open::that(url);
            }
            return ToolOutput::success(json!({
                "url": url,
                "warning": format!("目标网站返回了安全验证页面（{reason}），无法获取内容"),
                "note": "该网站有反爬机制，已在默认浏览器中打开，你可以直接查看。",
                "opened_browser": open_browser,
            }))
            .external();
        }

        let raw = arguments["raw"].as_bool().unwrap_or(false);
        let text = if raw {
            sanitize_web_content(&html)
        } else {
            sanitize_web_content(&extract_text(&html))
        };
        let truncated = if text.len() > max_len {
            format!(
                "{}...\n\n[页面过长，已截断至 {} 字符]",
                &text[..text.floor_char_boundary(max_len)],
                max_len
            )
        } else {
            text
        };

        if open_browser {
            let _ = open::that(url);
        }

        ToolOutput::success(json!({
            "url": url,
            "content": truncated,
            "chars": truncated.len(),
            "browser_opened": open_browser,
        }))
        .external()
    }
}

fn extract_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_script = false;
    let mut in_style = false;

    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        let ch = match html[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        let ch_len = ch.len_utf8();

        if ch == '<' {
            let lower = html[i..].to_lowercase();
            if lower.starts_with("<script") {
                in_script = true;
            }
            if lower.starts_with("<style") {
                in_style = true;
            }
            i += 1;
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            if lower.starts_with("</script") {
                in_script = false;
            }
            if lower.starts_with("</style") {
                in_style = false;
            }
            continue;
        }

        if in_script || in_style {
            i += ch_len;
            continue;
        }

        if ch == '&' {
            let rest = &html[i..];
            let (entity, skip) = if rest.starts_with("&amp;") {
                (Some("&"), 5)
            } else if rest.starts_with("&lt;") {
                (Some("<"), 4)
            } else if rest.starts_with("&gt;") {
                (Some(">"), 4)
            } else if rest.starts_with("&quot;") {
                (Some("\""), 6)
            } else if rest.starts_with("&#") {
                let end = rest.find(';').map(|p| p + 1).unwrap_or(rest.len());
                (Some(""), end)
            } else {
                (None, 0)
            };
            if let Some(e) = entity {
                result.push_str(e);
                i += skip;
                continue;
            }
        }

        if ch == '\n' || ch == '\r' {
            if !result.ends_with('\n') {
                result.push('\n');
            }
            i += ch_len;
            continue;
        }
        if ch.is_ascii_whitespace() {
            if !result.ends_with(' ') {
                result.push(' ');
            }
            i += ch_len;
            continue;
        }

        result.push(ch);
        i += ch_len;
    }

    let lines: Vec<&str> = result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_strips_tags() {
        let html = "<html><body><h1>Title</h1><p>Hello world</p></body></html>";
        let text = extract_text(html);
        assert!(text.contains("Title"), "expected Title in '{text}'");
        assert!(
            text.contains("Hello world"),
            "expected Hello world in '{text}'"
        );
    }

    #[test]
    fn extract_text_handles_script() {
        let html = "<html><script>alert('x')</script><body><p>Content</p></body></html>";
        let text = extract_text(html);
        assert!(!text.contains("alert"));
        assert!(text.contains("Content"));
    }

    #[test]
    fn extract_text_html_entities() {
        let html = "<p>AT&amp;T</p>";
        let text = extract_text(html);
        assert_eq!(text, "AT&T");
    }
}
