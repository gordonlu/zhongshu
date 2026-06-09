use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read the contents of a file at the specified path." }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let path: PathBuf = match arguments["path"].as_str() {
            Some(p) => p.into(),
            None => return ToolOutput::error("'path' must be a string"),
        };

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("读取 {path:?} 失败: {e}")),
        };

        let total_lines = content.lines().count();
        let truncated = if total_lines > 500 {
            let short: String = content.lines().take(500).collect::<Vec<_>>().join("\n");
            format!("{short}\n\n... (truncated, {} lines omitted)", total_lines - 500)
        } else {
            content
        };

        ToolOutput::success(json!({
            "path": path.display().to_string(),
            "content": truncated,
            "total_lines": total_lines,
        }))
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str { "Write content to a file. Creates parent directories if needed." }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the file"},
                "content": {"type": "string", "description": "Content to write"}
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let path: PathBuf = match arguments["path"].as_str() {
            Some(p) => p.into(),
            None => return ToolOutput::error("'path' must be a string"),
        };
        let content = match arguments["content"].as_str() {
            Some(c) => c,
            None => return ToolOutput::error("'content' must be a string"),
        };

        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolOutput::error(format!("创建父目录失败: {e}"));
            }
        }

        match std::fs::write(&path, content) {
            Ok(_) => ToolOutput::success(json!({ "path": path.display().to_string(), "written": true })),
            Err(e) => ToolOutput::error(format!("写入 {path:?} 失败: {e}")),
        }
    }
}

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str { "List files and directories at the specified path." }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to the directory"}
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let path: PathBuf = match arguments["path"].as_str() {
            Some(p) => p.into(),
            None => return ToolOutput::error("'path' must be a string"),
        };

        let entries: Vec<serde_json::Value> = match std::fs::read_dir(&path) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    json!({ "name": e.file_name().to_string_lossy(), "is_dir": is_dir })
                })
                .collect(),
            Err(e) => return ToolOutput::error(format!("读取 {path:?} 失败: {e}")),
        };

        ToolOutput::success(json!({
            "path": path.display().to_string(),
            "entries": entries,
            "count": entries.len(),
        }))
    }
}
