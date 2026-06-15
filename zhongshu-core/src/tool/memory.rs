use std::path::PathBuf;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tool::{Tool, ToolOutput};

/// A single memory entry stored in the agent profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryEntry {
    #[serde(default)]
    id: String,
    text: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    created_at: String,
}

/// Lightweight view of the agent profile JSON (only memory-relevant fields).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProfileView {
    #[serde(default)]
    long_term_memory: Vec<MemoryEntry>,
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

/// Read or write the long-term memory stored in the agent profile.
///
/// The LLM uses this to remember user preferences, important facts, and
/// other cross-session context.
#[derive(Clone)]
pub struct MemoryTool {
    profile_path: PathBuf,
}

impl MemoryTool {
    pub fn new(profile_path: PathBuf) -> Self {
        MemoryTool { profile_path }
    }

    fn load(&self) -> ProfileView {
        std::fs::read_to_string(&self.profile_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, view: &ProfileView) -> Result<(), String> {
        // Preserve the full file by reading, merging, and writing back.
        let raw: serde_json::Value = std::fs::read_to_string(&self.profile_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}));

        let mut raw = match raw {
            serde_json::Value::Object(m) => m,
            _ => return Err("profile is not a JSON object".into()),
        };

        raw.insert("long_term_memory".into(), serde_json::to_value(&view.long_term_memory).unwrap());

        if let Some(parent) = self.profile_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = self.profile_path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&raw).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.profile_path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "读取或更新长期记忆（用户偏好、重要事实、跨会话上下文）。\n\
         总字符数上限 2000，超出时你必须自己取舍精简——用 append 追加精简后的版本替换旧内容。\n\
         每一条记忆是独立的条目。\n\
         \n\
         - `read` — 返回所有记忆条目\n\
         - `append` — 添加一条新记忆\n\
         \n\
         使用场景：用户明确表达了偏好、约定了习惯、或你发现了需要长期记住的信息。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["read", "append"],
                    "description": "read=查看全部记忆, append=添加新记忆"
                },
                "text": {
                    "type": "string",
                    "description": "记忆内容（append 时必填）"
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let operation = match arguments["operation"].as_str() {
            Some(op) => op,
            None => return ToolOutput::error("'operation' 必须是 read/append"),
        };

        match operation {
            "read" => {
                let view = self.load();
                let texts: Vec<&str> = view.long_term_memory.iter().map(|e| e.text.as_str()).collect();
                let char_count: usize = texts.iter().map(|t| t.chars().count()).sum();
                let content = if texts.is_empty() {
                    String::from("（暂无长期记忆）")
                } else {
                    texts.join("\n")
                };
                ToolOutput::success(json!({"content": content, "count": texts.len(), "char_count": char_count, "char_limit": 2000}))
            }
            "append" => {
                let text = match arguments["text"].as_str() {
                    Some(t) => t,
                    None => return ToolOutput::error("append 操作需要提供 'text'"),
                };
                let mut view = self.load();
                let ts = timestamp();
                view.long_term_memory.push(MemoryEntry {
                    id: format!("mem-{ts}"),
                    text: text.to_string(),
                    source: "tool".into(),
                    created_at: ts,
                });
                if let Err(e) = self.save(&view) {
                    return ToolOutput::error(&format!("保存失败: {e}"));
                }
                ToolOutput::success(json!({"status": "已记录"}))
            }
            _ => ToolOutput::error("'operation' 必须是 read/append"),
        }
    }
}
