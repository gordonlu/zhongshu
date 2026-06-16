use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::core::models::SuggestionStatus;
use crate::core::suggestion::SuggestionEngine;
use crate::event::{Event, SuggestionEvent};
use crate::tool::{Tool, ToolOutput};

#[derive(Clone)]
pub struct SuggestionTool {
    engine: SuggestionEngine,
    eb: Option<Arc<crate::event::EventBus>>,
}

impl SuggestionTool {
    pub fn new(engine: SuggestionEngine) -> Self {
        SuggestionTool { engine, eb: None }
    }

    pub fn with_event_bus(mut self, eb: Arc<crate::event::EventBus>) -> Self {
        self.eb = Some(eb);
        self
    }
}

#[async_trait]
impl Tool for SuggestionTool {
    fn name(&self) -> &str {
        "suggestion"
    }

    fn description(&self) -> &str {
        "查看和处理系统建议。系统会从观察中自动发现可能值得做的事情。\
         \n- list: 查看待处理的建议\
         \n- accept <id>: 接受建议（系统会自动创建目标或任务）\
         \n- reject <id>: 拒绝建议"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "accept", "reject"],
                    "description": "操作类型"
                },
                "suggestion_id": {
                    "type": "string",
                    "description": "建议 ID（accept/reject 时必填）"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let action = arguments["action"].as_str().unwrap_or("");
        match action {
            "list" => match self.engine.list_pending() {
                Ok(sugs) => {
                    let items: Vec<serde_json::Value> = sugs
                        .iter()
                        .map(|s| {
                            json!({
                                "id": s.id, "type": s.type_, "content": s.content,
                                "confidence": s.confidence, "created_at": s.created_at,
                            })
                        })
                        .collect();
                    if items.is_empty() {
                        ToolOutput::success(json!({"suggestions": [], "note": "暂无待处理建议"}))
                    } else {
                        ToolOutput::success(json!({"suggestions": items}))
                    }
                }
                Err(e) => ToolOutput::error(&format!("读取建议失败: {e}")),
            },
            "accept" => {
                let id = match arguments["suggestion_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("accept 需要 suggestion_id"),
                };
                let content = match self.engine.get_content(id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("suggestion: get_content failed for id={id}: {e}");
                        return ToolOutput::error(&format!("读取建议失败: {e}"));
                    }
                };
                match self.engine.update_status(id, &SuggestionStatus::Accepted) {
                    Ok(true) => {
                        if let Some(eb) = &self.eb {
                            eb.publish(Event::Suggestion(SuggestionEvent::Accepted {
                                suggestion_id: id.to_string(),
                                content,
                            }));
                        }
                        ToolOutput::success(json!({"status": "accepted"}))
                    }
                    Ok(false) => ToolOutput::error("建议不存在"),
                    Err(e) => ToolOutput::error(&format!("接受建议失败: {e}")),
                }
            }
            "reject" => {
                let id = match arguments["suggestion_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("reject 需要 suggestion_id"),
                };
                match self.engine.update_status(id, &SuggestionStatus::Rejected) {
                    Ok(true) => {
                        if let Some(eb) = &self.eb {
                            eb.publish(Event::Suggestion(SuggestionEvent::Rejected {
                                suggestion_id: id.to_string(),
                            }));
                        }
                        ToolOutput::success(json!({"status": "rejected"}))
                    }
                    Ok(false) => ToolOutput::error("建议不存在"),
                    Err(e) => ToolOutput::error(&format!("拒绝建议失败: {e}")),
                }
            }
            _ => ToolOutput::error("action 必须是 list/accept/reject"),
        }
    }
}
