use async_trait::async_trait;
use serde_json::json;

use crate::core::goal::GoalRepository;
use crate::core::models::*;
use crate::tool::{Tool, ToolOutput};

#[derive(Clone)]
pub struct GoalTool {
    repo: GoalRepository,
}

impl GoalTool {
    pub fn new(repo: GoalRepository) -> Self {
        GoalTool { repo }
    }
}

#[async_trait]
impl Tool for GoalTool {
    fn name(&self) -> &str { "goal" }

    fn description(&self) -> &str {
        "管理长期目标。支持创建、查看、暂停、完成目标。\
         \n- create: 创建新目标（需 title, type: one_shot/recurring/ongoing）\
         \n- list: 查看所有活跃目标\
         \n- pause <id>: 暂停目标\
         \n- complete <id>: 标记目标完成\
         \n\n用户表达长期意图时使用 create。"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "pause", "complete", "remove"],
                    "description": "操作类型"
                },
                "title": {
                    "type": "string",
                    "description": "目标标题（create 时必填）"
                },
                "type": {
                    "type": "string",
                    "enum": ["one_shot", "recurring", "ongoing"],
                    "description": "目标类型（create 时必填）"
                },
                "description": {
                    "type": "string",
                    "description": "目标描述（可选）"
                },
                "goal_id": {
                    "type": "string",
                    "description": "目标 ID（pause/complete 时必填）"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &serde_json::Value) -> ToolOutput {
        let action = arguments["action"].as_str().unwrap_or("");
        match action {
            "create" => {
                let title = match arguments["title"].as_str() {
                    Some(t) => t,
                    None => return ToolOutput::error("create 需要 title"),
                };
                let type_str = arguments["type"].as_str().unwrap_or("one_shot");
                let goal_type = GoalType::from_str(type_str).unwrap_or(GoalType::OneShot);
                let desc = arguments["description"].as_str();
                match self.repo.create(title, desc, goal_type) {
                    Ok(goal) => ToolOutput::success(json!({
                        "status": "created",
                        "goal": { "id": goal.id, "title": goal.title, "type": goal.goal_type.as_str() }
                    })),
                    Err(e) => ToolOutput::error(&format!("创建目标失败: {e}")),
                }
            }
            "list" => {
                match self.repo.list_active() {
                    Ok(goals) => {
                        let items: Vec<serde_json::Value> = goals.iter().map(|g| json!({
                            "id": g.id,
                            "title": g.title,
                            "type": g.goal_type.as_str(),
                            "status": g.status.as_str(),
                        })).collect();
                        if items.is_empty() {
                            ToolOutput::success(json!({"goals": [], "note": "暂无活跃目标"}))
                        } else {
                            ToolOutput::success(json!({"goals": items}))
                        }
                    }
                    Err(e) => ToolOutput::error(&format!("读取目标失败: {e}")),
                }
            }
            "pause" => {
                let id = match arguments["goal_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("pause 需要 goal_id"),
                };
                match self.repo.update_status(id, GoalStatus::Paused) {
                    Ok(true) => ToolOutput::success(json!({"status": "paused"})),
                    Ok(false) => ToolOutput::error("目标不存在"),
                    Err(e) => ToolOutput::error(&format!("暂停目标失败: {e}")),
                }
            }
            "complete" => {
                let id = match arguments["goal_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("complete 需要 goal_id"),
                };
                match self.repo.update_status(id, GoalStatus::Completed) {
                    Ok(true) => ToolOutput::success(json!({"status": "completed"})),
                    Ok(false) => ToolOutput::error("目标不存在"),
                    Err(e) => ToolOutput::error(&format!("完成目标失败: {e}")),
                }
            }
            "remove" => {
                let id = match arguments["goal_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("remove 需要 goal_id"),
                };
                match self.repo.update_status(id, GoalStatus::Archived) {
                    Ok(true) => ToolOutput::success(json!({"status": "removed"})),
                    Ok(false) => ToolOutput::error("目标不存在"),
                    Err(e) => ToolOutput::error(&format!("删除目标失败: {e}")),
                }
            }
            _ => ToolOutput::error("action 必须是 create/list/pause/complete/remove"),
        }
    }
}
