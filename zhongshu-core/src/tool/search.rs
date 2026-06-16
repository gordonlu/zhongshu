use crate::tool::{build_browser_client, Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use tracing::info;

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web via DuckDuckGo and return results."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "The search query"},
                "max_results": {"type": "integer", "description": "Max results (default 5, max 10)", "default": 5},
                "region": {"type": "string", "description": "Region code, e.g. cn-zh for Chinese, us-en for US, jp-jp for Japan (default cn-zh)", "default": "cn-zh"}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let query = match arguments["query"].as_str() {
            Some(q) => q,
            None => return ToolOutput::error("'query' must be a string"),
        };
        let max = arguments["max_results"].as_u64().unwrap_or(5).min(10) as usize;
        let region = arguments["region"].as_str().unwrap_or("cn-zh");

        info!("web_search: {query}  region={region}");

        let client = match build_browser_client() {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("创建 HTTP 客户端失败: {e}")),
        };

        let url = format!(
            "https://html.duckduckgo.com/html/?q={}&kl={}",
            urlencoding(query),
            urlencoding(region)
        );

        let html = match client.get(&url).send().await {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(e) => return ToolOutput::error(format!("读取响应失败: {e}")),
            },
            Err(e) => return ToolOutput::error(format!("搜索请求失败: {e}")),
        };

        let results = parse_duckduckgo(&html, max);

        ToolOutput::success(json!({
            "query": query,
            "results": results,
            "count": results.len(),
        }))
    }
}

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char)
            }
            b' ' => result.push('+'),
            other => {
                result.push('%');
                result.push(hex_char(other >> 4));
                result.push(hex_char(other & 0x0f));
            }
        }
    }
    result
}

fn hex_char(b: u8) -> char {
    match b {
        0..=9 => (b'0' + b) as char,
        _ => (b'A' + (b - 10)) as char,
    }
}

fn parse_duckduckgo(html: &str, max: usize) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    let snippets: Vec<&str> = html.split("class=\"result__snippet\"").skip(1).collect();
    for snippet in snippets.iter().take(max) {
        let text = snippet
            .split("</a>")
            .nth(1)
            .unwrap_or("")
            .split('<')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let href = snippet
            .split("href=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("")
            .to_string();
        let title = snippet
            .split("class=\"result__a\"")
            .nth(1)
            .and_then(|s| s.split('>').nth(1))
            .and_then(|s| s.split('<').next())
            .unwrap_or("")
            .trim()
            .to_string();
        if !text.is_empty() {
            results.push(json!({ "title": title, "url": href, "snippet": text }));
        }
    }
    results
}
