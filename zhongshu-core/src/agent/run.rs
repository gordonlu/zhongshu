use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::intent::{intent_classify, InterruptionIntent};
use crate::event::{Event, EventBus, ResponseEvent, RunEvent};
use crate::tool::spec::SideEffect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Thinking,
    Streaming,
    ToolExecuting {
        tool_name: String,
        tool_call_id: String,
        side_effect: SideEffect,
    },
    WaitingApproval {
        request_id: String,
    },
    Interrupted {
        reason: String,
    },
    Resuming,
    Finished {
        stop_reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub tool_name: String,
    pub tool_call_id: String,
    pub side_effect: SideEffect,
    pub start_time: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct InterruptionCtx {
    pub original_goal: String,
    pub completed_steps: Vec<String>,
    pub current_state: String,
    pub active_tool: Option<ToolCallInfo>,
    pub partial_response: String,
    pub user_message: String,
}

#[derive(Debug, Clone)]
pub enum InterruptionAction {
    ContinueWithNote { note: String },
    PauseAndRespond { summary: String },
    CancelAndReplan { reason: String },
    RequireConfirmation { question: String },
}

pub struct RunController {
    run_id: Arc<RwLock<Option<Uuid>>>,
    state: Arc<RwLock<RunState>>,
    cancel_token: Arc<RwLock<CancellationToken>>,
    partial_response: Arc<Mutex<String>>,
    completed_steps: Arc<Mutex<Vec<String>>>,
    current_tool: Arc<Mutex<Option<ToolCallInfo>>>,
    interruption_ctx: Arc<Mutex<Option<InterruptionCtx>>>,
    original_goal: Arc<RwLock<String>>,
    event_bus: Arc<EventBus>,
    last_action: std::sync::Mutex<Option<InterruptionAction>>,
    interrupted: Arc<AtomicBool>,
}

impl RunController {
    pub fn new(event_bus: Arc<EventBus>, _response_tx: mpsc::Sender<ResponseEvent>) -> Self {
        Self {
            run_id: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(RunState::Idle)),
            cancel_token: Arc::new(RwLock::new(CancellationToken::new())),
            partial_response: Arc::new(Mutex::new(String::new())),
            completed_steps: Arc::new(Mutex::new(Vec::new())),
            current_tool: Arc::new(Mutex::new(None)),
            interruption_ctx: Arc::new(Mutex::new(None)),
            original_goal: Arc::new(RwLock::new(String::new())),
            event_bus,
            last_action: std::sync::Mutex::new(None),
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn interruption_ctx(&self) -> Option<InterruptionCtx> {
        self.interruption_ctx.blocking_lock().clone()
    }

    pub fn take_last_action(&self) -> Option<InterruptionAction> {
        self.last_action.lock().unwrap().take()
    }

    pub fn start_run(&self, goal: &str) -> Uuid {
        let run_id = Uuid::new_v4();
        *self.run_id.blocking_write() = Some(run_id);
        *self.state.blocking_write() = RunState::Thinking;
        *self.original_goal.blocking_write() = goal.to_string();
        self.interrupted.store(false, Ordering::SeqCst);
        self.partial_response.blocking_lock().clear();
        self.completed_steps.blocking_lock().clear();
        self.interruption_ctx.blocking_lock().take();

        self.event_bus.publish(Event::Run(RunEvent::Started {
            run_id: run_id.to_string(),
            goal: goal.to_string(),
        }));

        // Reset the cancellation token
        *self.cancel_token.blocking_write() = CancellationToken::new();

        run_id
    }

    pub fn run_id(&self) -> Option<Uuid> {
        *self.run_id.blocking_read()
    }

    pub fn active_run_id(&self) -> Uuid {
        self.run_id.blocking_read().unwrap_or_else(Uuid::new_v4)
    }

    pub fn state(&self) -> RunState {
        self.state.blocking_read().clone()
    }

    pub fn set_state(&self, state: RunState) {
        *self.state.blocking_write() = state;
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.blocking_read().clone()
    }

    pub fn record_partial(&self, delta: &str) {
        self.partial_response.blocking_lock().push_str(delta);
    }

    pub fn take_partial_response(&self) -> String {
        self.partial_response.blocking_lock().drain(..).collect()
    }

    pub fn record_completed_step(&self, step: String) {
        self.completed_steps.blocking_lock().push(step);
    }

    pub fn take_completed_steps(&self) -> Vec<String> {
        self.completed_steps.blocking_lock().drain(..).collect()
    }

    pub fn set_current_tool(&self, tool: Option<ToolCallInfo>) {
        *self.current_tool.blocking_lock() = tool;
    }

    pub fn current_tool(&self) -> Option<ToolCallInfo> {
        self.current_tool.blocking_lock().clone()
    }

    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Called when user submits a message during an active run.
    pub fn interrupt(&self, user_message: &str) -> InterruptionAction {
        // If already interrupted, append message instead of overwriting context.
        if self.interrupted.load(Ordering::SeqCst) {
            if let Some(ref mut ctx) = *self.interruption_ctx.blocking_lock() {
                ctx.user_message = format!("{}\n{}", ctx.user_message, user_message);
                let intent = intent_classify(&ctx.user_message);
                let state = self.state.blocking_read().clone();
                let tool = self.current_tool();
                let action = self.determine_action(intent, &state, tool.as_ref());
                *self.last_action.lock().unwrap() = Some(action.clone());
                return action;
            }
        }

        let rid = self
            .run_id
            .blocking_read()
            .map(|r| r.to_string())
            .unwrap_or_default();

        // Save context before transition
        let partial = self.take_partial_response();
        let steps = self.take_completed_steps();
        let tool = self.current_tool();
        let goal = self.original_goal.blocking_read().clone();
        let state_str = format!("{:?}", self.state.blocking_read());

        // Mark interrupted
        self.interrupted.store(true, Ordering::SeqCst);
        *self.state.blocking_write() = RunState::Interrupted {
            reason: "user_interjection".into(),
        };

        // Cancel current LLM request (best-effort)
        self.cancel_token.blocking_read().cancel();

        // Store interruption context
        *self.interruption_ctx.blocking_lock() = Some(InterruptionCtx {
            original_goal: goal.clone(),
            completed_steps: steps.clone(),
            current_state: state_str.clone(),
            active_tool: tool.clone(),
            partial_response: partial.clone(),
            user_message: user_message.to_string(),
        });

        // Publish event
        self.event_bus.publish(Event::Run(RunEvent::Interrupted {
            run_id: rid,
            reason: "user_interjection".into(),
        }));

        // Classify intent
        let intent = intent_classify(user_message);

        // Determine action
        let state_snapshot = self.state.blocking_read().clone();
        let action = self.determine_action(intent, &state_snapshot, tool.as_ref());
        *self.last_action.lock().unwrap() = Some(action.clone());
        action
    }

    fn determine_action(
        &self,
        intent: InterruptionIntent,
        state: &RunState,
        _tool: Option<&ToolCallInfo>,
    ) -> InterruptionAction {
        match intent {
            InterruptionIntent::Stop => InterruptionAction::CancelAndReplan {
                reason: "用户要求停止".into(),
            },
            InterruptionIntent::Redirect | InterruptionIntent::Constraint => {
                InterruptionAction::CancelAndReplan {
                    reason: match intent {
                        InterruptionIntent::Redirect => "用户要求改变方向".into(),
                        _ => "用户增加新约束".into(),
                    },
                }
            }
            InterruptionIntent::ApprovalCorrection => {
                if matches!(
                    state,
                    RunState::ToolExecuting { .. } | RunState::WaitingApproval { .. }
                ) {
                    InterruptionAction::RequireConfirmation {
                        question: "当前操作被用户禁止，需要重新确认。".into(),
                    }
                } else {
                    InterruptionAction::CancelAndReplan {
                        reason: "用户修改了权限限制".into(),
                    }
                }
            }
            InterruptionIntent::ProgressAsk => InterruptionAction::PauseAndRespond {
                summary: "用户询问当前进度".into(),
            },
            InterruptionIntent::Continue => InterruptionAction::ContinueWithNote {
                note: "用户要求继续".into(),
            },
            InterruptionIntent::Other => {
                // Check if there's an interrupted run to resume
                if self.interrupted.load(Ordering::SeqCst) {
                    InterruptionAction::ContinueWithNote {
                        note: "用户补充了新信息".into(),
                    }
                } else {
                    InterruptionAction::ContinueWithNote {
                        note: String::new(),
                    }
                }
            }
        }
    }

    /// Build recovery context system message for the agent loop.
    pub fn build_recovery_prompt(&self) -> Option<String> {
        let ctx = self.interruption_ctx.blocking_lock().take()?;

        if ctx.partial_response.is_empty()
            && ctx.completed_steps.is_empty()
            && ctx.active_tool.is_none()
        {
            // No meaningful context to recover — let normal flow handle it
            return None;
        }

        let mut prompt = String::from("当前运行被用户插话打断。\n\n");
        prompt.push_str(&format!("原任务：\n{}\n\n", ctx.original_goal));

        if !ctx.completed_steps.is_empty() {
            prompt.push_str("已完成：\n");
            for step in &ctx.completed_steps {
                prompt.push_str(&format!("- {}\n", step));
            }
            prompt.push('\n');
        }

        prompt.push_str(&format!("当前状态：{}\n\n", ctx.current_state));

        if let Some(tool) = &ctx.active_tool {
            prompt.push_str(&format!(
                "当前工具：{}（{}）\n\n",
                tool.tool_name,
                match tool.side_effect {
                    SideEffect::ReadOnly => "只读",
                    SideEffect::LocalWrite => "本地写入",
                    SideEffect::SystemChange => "系统变更",
                    SideEffect::ExternalAction => "外部操作",
                    SideEffect::Irreversible => "不可逆操作",
                }
            ));
        }

        if !ctx.partial_response.is_empty() {
            prompt.push_str("已输出但未完成的回复：\n");
            prompt.push_str(&ctx.partial_response);
            prompt.push_str("\n\n");
        }

        prompt.push_str(&format!("用户插话：\n{}\n\n", ctx.user_message));
        prompt.push_str("请先自然回应用户插话，然后决定：\n");
        prompt.push_str("1. 是否暂停原任务\n");
        prompt.push_str("2. 是否调整约束\n");
        prompt.push_str("3. 是否继续\n");
        prompt.push_str("4. 是否需要用户确认\n");

        Some(prompt)
    }

    pub fn begin_resume(&self) -> Uuid {
        let run_id = self.run_id.blocking_read().unwrap_or_else(Uuid::new_v4);
        *self.cancel_token.blocking_write() = CancellationToken::new();
        self.interrupted.store(false, Ordering::SeqCst);
        *self.state.blocking_write() = RunState::Resuming;
        self.event_bus.publish(Event::Run(RunEvent::Resuming {
            run_id: run_id.to_string(),
        }));
        run_id
    }

    pub async fn finish_run(&self, stop_reason: &str) {
        *self.state.blocking_write() = RunState::Finished {
            stop_reason: stop_reason.into(),
        };
        self.interrupted.store(false, Ordering::SeqCst);
        if let Some(rid) = *self.run_id.blocking_read() {
            self.event_bus.publish(Event::Run(RunEvent::Finished {
                run_id: rid.to_string(),
                stop_reason: stop_reason.into(),
            }));
        }
    }
}

unsafe impl Send for RunController {}
unsafe impl Sync for RunController {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventRx;

    fn make_controller() -> (RunController, EventRx) {
        let eb = Arc::new(EventBus::new(16));
        let rx = eb.subscribe();
        let (tx, _) = mpsc::channel(16);
        (RunController::new(eb, tx), rx)
    }

    #[test]
    fn test_initial_state_idle() {
        let (ctrl, _) = make_controller();
        assert_eq!(ctrl.state(), RunState::Idle);
        assert!(ctrl.run_id().is_none());
    }

    #[test]
    fn test_start_run_generates_run_id() {
        let (ctrl, rx) = make_controller();
        let rid = ctrl.start_run("test goal");
        assert_eq!(ctrl.run_id(), Some(rid));
        assert_eq!(ctrl.state(), RunState::Thinking);

        // Check event published
        let mut r = rx;
        match r.try_recv() {
            Ok(Event::Run(RunEvent::Started { run_id, goal })) => {
                assert_eq!(run_id, rid.to_string());
                assert_eq!(goal, "test goal");
            }
            _ => panic!("expected RunEvent::Started"),
        }
    }

    #[test]
    fn test_interrupt_during_streaming() {
        let (ctrl, rx) = make_controller();
        let _rid = ctrl.start_run("test goal");
        ctrl.set_state(RunState::Streaming);
        ctrl.record_partial("I am generating a long");
        ctrl.record_partial(" response for you");

        let action = ctrl.interrupt("停一下");

        assert!(ctrl.is_interrupted());
        assert!(matches!(ctrl.state(), RunState::Interrupted { .. }));
        assert!(matches!(action, InterruptionAction::CancelAndReplan { .. }));

        // Interruption context should contain partial response
        let ctx_set = ctrl.interruption_ctx.blocking_lock().is_some();
        assert!(ctx_set);

        // RunInterrupted event published
        let mut r = rx;
        loop {
            match r.try_recv() {
                Ok(Event::Run(RunEvent::Interrupted { .. })) => break,
                Ok(_) => continue,
                _ => panic!("expected RunEvent::Interrupted"),
            }
        }
    }

    #[test]
    fn test_intent_stop_maps_to_cancel_and_replan() {
        let (ctrl, _) = make_controller();
        ctrl.start_run("test");

        let action = ctrl.interrupt("停");
        assert!(matches!(action, InterruptionAction::CancelAndReplan { .. }));
    }

    #[test]
    fn test_intent_continue_maps_to_continue_with_note() {
        let (ctrl, _) = make_controller();
        ctrl.start_run("test");
        ctrl.interrupt("等等");

        // Second interjection with "继续"
        let action = ctrl.interrupt("继续吧");
        assert!(matches!(
            action,
            InterruptionAction::ContinueWithNote { .. }
        ));
    }

    #[test]
    fn test_intent_progress_maps_to_pause_and_respond() {
        let (ctrl, _) = make_controller();
        ctrl.start_run("test");
        ctrl.record_completed_step("searched files".into());

        let action = ctrl.interrupt("做到哪了");
        assert!(matches!(action, InterruptionAction::PauseAndRespond { .. }));
    }

    #[test]
    fn test_build_recovery_prompt_includes_context() {
        let (ctrl, _) = make_controller();
        ctrl.start_run("build feature X");
        ctrl.record_partial("I have started working on");
        ctrl.record_completed_step("analyzed requirements".into());
        ctrl.set_current_tool(Some(ToolCallInfo {
            tool_name: "write_file".into(),
            tool_call_id: "call-1".into(),
            side_effect: SideEffect::LocalWrite,
            start_time: std::time::Instant::now(),
        }));

        ctrl.interrupt("不要写文件");
        let prompt = ctrl.build_recovery_prompt();

        assert!(prompt.is_some());
        let p = prompt.unwrap();
        assert!(p.contains("build feature X"));
        assert!(p.contains("analyzed requirements"));
        assert!(p.contains("I have started working on"));
        assert!(p.contains("不要写文件"));
    }
}
