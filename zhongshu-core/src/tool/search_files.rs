use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;

pub struct SearchFilesTool;

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("执行失败: {e}"))?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(err)
    }
}

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str { "search_files" }
    fn description(&self) -> &str {
        "Search for files on the local filesystem by name or pattern. Uses locate > fd > find."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Filename or search pattern (case-insensitive)"
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory path to search in (default: entire filesystem)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let query = arguments["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return ToolOutput::error("请提供搜索关键词");
        }
        let path = arguments["path"].as_str().filter(|p| !p.is_empty());

        if which("locate") {
            let mut args = vec!["-i", query];
            if let Some(p) = path { args.push(p); }
            return match run_cmd("locate", &args) {
                Ok(out) => {
                    let results = if out.is_empty() { "未找到匹配文件".to_string() } else { out };
                    ToolOutput::success(json!({ "tool": "locate", "results": results }))
                }
                Err(e) => ToolOutput::error(e),
            };
        }
        if which("fd") {
            let mut args = vec!["-i", query];
            if let Some(p) = path { args.push(p); }
            return match run_cmd("fd", &args) {
                Ok(out) => {
                    let results = if out.is_empty() { "未找到匹配文件".to_string() } else { out };
                    ToolOutput::success(json!({ "tool": "fd", "results": results }))
                }
                Err(e) => ToolOutput::error(e),
            };
        }

        // No search tool — let the AI ask the user naturally.
        ToolOutput::success(json!({
            "tool": null,
            "results": null,
            "note": "系统中未安装 locate 或 fd，文件搜索会较慢。请用户决定是否安装 plocate（sudo apt install plocate）。",
        }))
    }
}
