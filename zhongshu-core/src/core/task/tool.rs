use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::core::models::*;
use crate::core::task::TaskRepository;
use crate::event::{Event, EventBus, TaskEvent};
use crate::tool::{Tool, ToolOutput};

#[derive(Clone)]
pub struct TaskTool {
    repo: TaskRepository,
    eb: Option<Arc<EventBus>>,
}

impl TaskTool {
    pub fn new(repo: TaskRepository) -> Self {
        TaskTool { repo, eb: None }
    }

    pub fn with_event_bus(mut self, eb: Arc<EventBus>) -> Self {
        self.eb = Some(eb);
        self
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "管理具体执行任务。任务从目标派生。\
         \n重要：任务完成后必须立即调用 cancel 或 complete。不要留下挂起任务。\
         \n- create: 创建任务（需 title, 可选 goal_id）\
         \n- list: 查看待办任务\
         \n- recent: 查看最近任务（含 output/error/summary）\
         \n- complete <task_id>: 标记任务完成（可选 output）\
         \n- cancel <task_id>: 取消任务（可选 reason）\
         \n- retry <task_id>: 重试失败任务（受 max_retries 限制）\
         \n- add_step <task_id> <order> <action>: 添加执行步骤"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "recent", "complete", "cancel", "retry", "add_step"],
                    "description": "操作类型"
                },
                "title": {
                    "type": "string",
                    "description": "任务标题（create 时必填）"
                },
                "goal_id": {
                    "type": "string",
                    "description": "关联目标 ID（可选）"
                },
                "task_id": {
                    "type": "string",
                    "description": "任务 ID（cancel/retry/add_step 时必填）"
                },
                "output": {
                    "type": "string",
                    "description": "完成输出（complete 时可选）"
                },
                "reason": {
                    "type": "string",
                    "description": "取消原因（cancel 时可选）"
                },
                "step_order": {
                    "type": "integer",
                    "description": "步骤序号（add_step 时必填）"
                },
                "action_text": {
                    "type": "string",
                    "description": "步骤描述（add_step 时必填）"
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
                let goal_id = arguments["goal_id"].as_str();
                match self.repo.create(goal_id, title) {
                    Ok(task) => ToolOutput::success(json!({
                        "status": "created",
                        "task": { "id": task.id, "title": task.title, "status": task.status.as_str() }
                    })),
                    Err(e) => ToolOutput::error(&format!("创建任务失败: {e}")),
                }
            }
            "list" => match self.repo.list_pending() {
                Ok(tasks) => {
                    let items: Vec<serde_json::Value> = tasks
                        .iter()
                        .map(|t| {
                            json!({
                                "id": t.id,
                                "title": t.title,
                                "status": t.status.as_str(),
                                "goal_id": t.goal_id,
                            })
                        })
                        .collect();
                    if items.is_empty() {
                        ToolOutput::success(json!({"tasks": [], "note": "暂无待办任务"}))
                    } else {
                        ToolOutput::success(json!({"tasks": items}))
                    }
                }
                Err(e) => ToolOutput::error(&format!("读取任务失败: {e}")),
            },
            "recent" => match self.repo.list_recent(20) {
                Ok(tasks) => {
                    let items: Vec<serde_json::Value> = tasks
                        .iter()
                        .map(|t| {
                            json!({
                                "id": t.id,
                                "title": t.title,
                                "status": t.status.as_str(),
                                "output": t.output,
                                "error": t.error,
                                "summary": t.summary,
                                "created_at": t.created_at,
                            })
                        })
                        .collect();
                    ToolOutput::success(json!({"tasks": items}))
                }
                Err(e) => ToolOutput::error(&format!("读取任务失败: {e}")),
            },
            "complete" => {
                let id = match arguments["task_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("complete 需要 task_id"),
                };
                let output = arguments["output"].as_str().unwrap_or("");
                match self.repo.mark_completed(id, output) {
                    Ok(true) => {
                        if let Ok(Some(task)) = self.repo.get(id) {
                            let _ = self
                                .repo
                                .set_summary(id, &format!("completed: {}", task.title));
                            if let Some(eb) = &self.eb {
                                eb.publish(Event::Task(TaskEvent::Completed {
                                    task_id: id.to_string(),
                                    title: task.title.clone(),
                                    output: output.to_string(),
                                }));
                            }
                        }
                        ToolOutput::success(json!({"status": "completed"}))
                    }
                    Ok(false) => {
                        let t = self.repo.get(id).ok().flatten();
                        let reason = match t {
                            Some(task) if task.status == TaskStatus::Cancelled => {
                                "任务已被取消".to_string()
                            }
                            Some(task) => format!("任务状态为 {}", task.status.as_str()),
                            None => "任务不存在".to_string(),
                        };
                        ToolOutput::error(&format!("无法完成: {reason}"))
                    }
                    Err(e) => ToolOutput::error(&format!("完成任务失败: {e}")),
                }
            }
            "cancel" => {
                let id = match arguments["task_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("cancel 需要 task_id"),
                };
                let reason = arguments["reason"].as_str().unwrap_or("用户取消");
                match self.repo.mark_cancelled(id, "", reason) {
                    Ok(true) => {
                        if let Ok(Some(task)) = self.repo.get(id) {
                            if let Some(eb) = &self.eb {
                                eb.publish(Event::Task(TaskEvent::Cancelled {
                                    task_id: id.to_string(),
                                    title: task.title.clone(),
                                    reason: reason.to_string(),
                                }));
                            }
                        }
                        ToolOutput::success(json!({"status": "cancelled"}))
                    }
                    Ok(false) => ToolOutput::error("无法取消：任务已处于终态"),
                    Err(e) => ToolOutput::error(&format!("取消任务失败: {e}")),
                }
            }
            "retry" => {
                let id = match arguments["task_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("retry 需要 task_id"),
                };
                match self.repo.schedule_retry(id) {
                    Ok(ScheduleRetryResult::Scheduled) => {
                        if let Ok(Some(task)) = self.repo.get(id) {
                            if let Some(eb) = &self.eb {
                                eb.publish(Event::Task(TaskEvent::Triggered {
                                    task_id: id.to_string(),
                                    title: task.title.clone(),
                                }));
                            }
                        }
                        ToolOutput::success(json!({"status": "retry_scheduled"}))
                    }
                    Ok(ScheduleRetryResult::NotFound) => ToolOutput::error("任务不存在"),
                    Ok(ScheduleRetryResult::NotRetriable { reason }) => {
                        ToolOutput::error(&format!("不可重试: {reason}"))
                    }
                    Ok(ScheduleRetryResult::RetriesExhausted {
                        retry_count,
                        max_retries,
                    }) => {
                        ToolOutput::error(&format!("重试次数已耗尽 ({retry_count}/{max_retries})"))
                    }
                    Err(e) => ToolOutput::error(&format!("重试失败: {e}")),
                }
            }
            "add_step" => {
                let task_id = match arguments["task_id"].as_str() {
                    Some(i) => i,
                    None => return ToolOutput::error("add_step 需要 task_id"),
                };
                let order = arguments["step_order"].as_i64().unwrap_or(0) as i32;
                let action_text = match arguments["action_text"].as_str() {
                    Some(a) => a,
                    None => return ToolOutput::error("add_step 需要 action_text"),
                };
                match self.repo.add_step(task_id, order, action_text) {
                    Ok(step) => ToolOutput::success(json!({
                        "status": "step_added",
                        "step": { "id": step.id, "order": step.step_order, "action": step.action }
                    })),
                    Err(e) => ToolOutput::error(&format!("添加步骤失败: {e}")),
                }
            }
            _ => ToolOutput::error("action 必须是 create/list/recent/cancel/retry/add_step"),
        }
    }
}
