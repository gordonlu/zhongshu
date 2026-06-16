use async_trait::async_trait;
use serde_json::json;

use crate::agent::llm::OpenAiProvider;
use crate::core::memory::candidate::MemoryCandidateStore;
use crate::core::memory::policy::MemoryPolicy;
use crate::tool::{Tool, ToolOutput};

#[derive(Clone)]
pub struct MemoryQueryTool {
    policy: MemoryPolicy,
    candidates: MemoryCandidateStore,
    provider: Option<OpenAiProvider>,
}

impl MemoryQueryTool {
    pub fn new(policy: MemoryPolicy, candidates: MemoryCandidateStore) -> Self {
        MemoryQueryTool {
            policy,
            candidates,
            provider: None,
        }
    }

    pub fn with_provider(mut self, provider: OpenAiProvider) -> Self {
        self.provider = Some(provider);
        self
    }
}

#[async_trait]
impl Tool for MemoryQueryTool {
    fn name(&self) -> &str {
        "memory_query"
    }

    fn description(&self) -> &str {
        "搜索已有记忆，或提议新的记忆。\
         \n- search <keyword>: 搜索已有记忆\
         \n- list: 查看所有记忆\
         \n- propose <content> [type]: 提议一条新记忆（type: preference/profile/project/decision/procedure）\
         \n\n注意：你是提出建议，不是直接写入。系统会评估后决定是否采纳。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["search", "list", "propose"],
                    "description": "操作类型"
                },
                "keyword": {
                    "type": "string",
                    "description": "搜索关键词（search 时必填）"
                },
                "content": {
                    "type": "string",
                    "description": "记忆内容（propose 时必填）"
                },
                "memory_type": {
                    "type": "string",
                    "enum": ["preference", "profile", "project", "decision", "procedure"],
                    "description": "记忆类型（propose 时可选，默认 preference）"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let action = arguments["action"].as_str().unwrap_or("");
        match action {
            "search" => {
                let kw = match arguments["keyword"].as_str() {
                    Some(k) => k,
                    None => return ToolOutput::error("search 需要 keyword"),
                };
                let results = if let Some(ref provider) = self.provider {
                    self.policy
                        .search_with(kw, provider, 20)
                        .await
                        .unwrap_or_else(|_| self.policy.search(kw, 20).unwrap_or_default())
                } else {
                    self.policy.search(kw, 20).unwrap_or_default()
                };
                let items: Vec<serde_json::Value> = results
                    .iter()
                    .map(|m| {
                        json!({
                            "id": m.id, "type": m.memory_type.as_str(), "content": m.content
                        })
                    })
                    .collect();
                if items.is_empty() {
                    ToolOutput::success(json!({"memories": [], "note": "未找到匹配记忆"}))
                } else {
                    ToolOutput::success(json!({"memories": items}))
                }
            }
            "list" => match self.policy.list_memories(50) {
                Ok(mems) => {
                    let items: Vec<serde_json::Value> = mems
                        .iter()
                        .map(|m| {
                            json!({
                                "id": m.id, "type": m.memory_type.as_str(), "content": m.content
                            })
                        })
                        .collect();
                    ToolOutput::success(json!({"memories": items}))
                }
                Err(e) => ToolOutput::error(&format!("读取记忆失败: {e}")),
            },
            "propose" => {
                let content = match arguments["content"].as_str() {
                    Some(c) => c,
                    None => return ToolOutput::error("propose 需要 content"),
                };
                let mem_type = arguments["memory_type"].as_str();
                match self
                    .candidates
                    .insert(content, mem_type, 0.8, Some("agent"), None)
                {
                    Ok(mc) => ToolOutput::success(json!({
                        "status": "proposed",
                        "candidate_id": mc.id,
                        "content": mc.content,
                        "note": "已提议，系统评估后将决定是否存入长期记忆"
                    })),
                    Err(e) => ToolOutput::error(&format!("提议失败: {e}")),
                }
            }
            _ => ToolOutput::error("action 必须是 search/list/propose"),
        }
    }
}
