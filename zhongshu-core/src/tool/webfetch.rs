use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str { "webfetch" }
    fn description(&self) -> &str {
        "Fetch a URL and return the page text content (HTML stripped). Use this to read articles, check weather, or get structured data from web pages."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The URL to fetch"},
                "max_length": {"type": "integer", "description": "Max characters to return (default 5000)", "default": 5000}
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

        let client = match reqwest::Client::builder()
            .user_agent(BROWSER_UA)
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("HTTP 客户端创建失败: {e}")),
        };

        // Simulate human-like delay (500-2000ms) to avoid bot detection.
        let ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let delay_ms = 500 + (ns % 1501) as u64;
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

        let html = match client.get(url).send().await {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(e) => return ToolOutput::error(format!("读取响应失败: {e}")),
            },
            Err(e) => return ToolOutput::error(format!("请求失败: {e}")),
        };

        let text = extract_text(&html);
        let truncated = if text.len() > max_len {
            format!("{}...\n\n[页面过长，已截断至 {} 字符]", &text[..max_len], max_len)
        } else {
            text
        };

        ToolOutput::success(json!({
            "url": url,
            "content": truncated,
            "chars": truncated.len(),
        }))
    }
}

/// Simple HTML-to-text extraction: strip tags, extract meaningful content.
fn extract_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_script = false;
    let mut in_style = false;

    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Check for script/style tags to skip their content
            let lower = html[i..].to_lowercase();
            if lower.starts_with("<script") { in_script = true; }
            if lower.starts_with("<style") { in_style = true; }
            // Find end of tag
            while i < bytes.len() && bytes[i] != b'>' { i += 1; }
            if i < bytes.len() { i += 1; } // skip '>'
            if lower.starts_with("</script") { in_script = false; }
            if lower.starts_with("</style") { in_style = false; }
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        if bytes[i] == b'&' {
            let rest = &html[i..];
            let (entity, skip) = if rest.starts_with("&amp;") { (Some("&"), 5) }
                else if rest.starts_with("&lt;") { (Some("<"), 4) }
                else if rest.starts_with("&gt;") { (Some(">"), 4) }
                else if rest.starts_with("&quot;") { (Some("\""), 6) }
                else if rest.starts_with("&#") {
                    let end = rest.find(';').map(|p| p + 1).unwrap_or(rest.len());
                    (Some(""), end)
                } else { (None, 0) };
            if let Some(e) = entity {
                result.push_str(e);
                i += skip;
                continue;
            }
        }

        // Collapse multiple whitespace/newlines
        if bytes[i] == b'\n' || bytes[i] == b'\r' {
            if !result.ends_with('\n') { result.push('\n'); }
            i += 1;
            continue;
        }
        if bytes[i].is_ascii_whitespace() {
            if !result.ends_with(' ') { result.push(' '); }
            i += 1;
            continue;
        }

        result.push(bytes[i] as char);
        i += 1;
    }

    // Remove excessive blank lines
    let lines: Vec<&str> = result.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
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
        assert!(text.contains("Hello world"), "expected Hello world in '{text}'");
    }

    #[test]
    fn extract_text_handles_script() {
        let html = "<html><script>alert('x')</script><body><p>Content</p></body></html>";
        let text = extract_text(html);
        assert!(!text.contains("alert"), "script content leaked: {text}");
        assert!(text.contains("Content"));
    }

    #[test]
    fn extract_text_html_entities() {
        let html = "<p>AT&amp;T</p>";
        let text = extract_text(html);
        assert_eq!(text, "AT&T");
    }
}
