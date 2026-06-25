use crate::tool::{Tool, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Read the contents of a file at the specified path."
    }

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
            format!(
                "{short}\n\n... (truncated, {} lines omitted)",
                total_lines - 500
            )
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
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed."
    }

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
            Ok(_) => {
                ToolOutput::success(json!({ "path": path.display().to_string(), "written": true }))
            }
            Err(e) => ToolOutput::error(format!("写入 {path:?} 失败: {e}")),
        }
    }
}

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }
    fn description(&self) -> &str {
        "List files and directories at the specified path."
    }

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

/// Search file contents for a pattern (delegates to shell grep).
pub struct GrepTool;
#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        "Search file contents using grep pattern. Use glob to find files by name."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","default":"."}},"required":["pattern"]})
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        crate::tool::shell::ShellTool.execute(&json!({"command": format!("grep -rn '{}' {} 2>/dev/null | head -100", arguments["pattern"].as_str().unwrap_or(""), arguments["path"].as_str().unwrap_or("."))})).await
    }
}

/// Find files by glob pattern (delegates to shell find/fd).
pub struct GlobTool;
#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Find files by name glob pattern."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","default":"."}},"required":["pattern"]})
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        crate::tool::shell::ShellTool.execute(&json!({"command": format!("find {} -name '{}' 2>/dev/null | head -100", arguments["path"].as_str().unwrap_or("."), arguments["pattern"].as_str().unwrap_or(""))})).await
    }
}

/// Edit a file by replacing text (read → replace → write).
pub struct EditTool;
#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        "Edit a file: replace first occurrence of old_text with new_text."
    }
    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},"required":["path","old","new"]})
    }
    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let path = match arguments["path"].as_str() {
            Some(p) => p,
            None => return ToolOutput::error("'path' required"),
        };
        let old = match arguments["old"].as_str() {
            Some(o) => o,
            None => return ToolOutput::error("'old' required"),
        };
        let new = arguments["new"].as_str().unwrap_or("");
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("read {path}: {e}")),
        };
        if !content.contains(old) {
            return ToolOutput::error(format!(
                "'{}' not found in {path}",
                old.chars().take(50).collect::<String>()
            ));
        }
        let result = content.replacen(old, new, 1);
        let tmp = format!("{path}.tmp");
        match std::fs::write(&tmp, &result) {
            Ok(_) => {}
            Err(e) => return ToolOutput::error(format!("write tmp: {e}")),
        }
        match std::fs::rename(&tmp, path) {
            Ok(_) => {}
            Err(e) => return ToolOutput::error(format!("rename: {e}")),
        }
        ToolOutput::success(json!({"path": path, "replaced": 1}))
    }
}
