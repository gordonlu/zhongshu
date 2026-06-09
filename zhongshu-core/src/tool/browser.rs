use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;

pub struct BrowserTool;

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str { "browser" }
    fn description(&self) -> &str { "Open a URL in the default browser." }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "The URL to open"}
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let url = match arguments["url"].as_str() {
            Some(u) => u,
            None => return ToolOutput::error("'url' must be a string"),
        };

        match open::that(url) {
            Ok(_) => ToolOutput::success(json!({ "opened": url })),
            Err(e) => ToolOutput::error(format!("打开浏览器失败: {e}")),
        }
    }
}
