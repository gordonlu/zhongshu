use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::agent::intent::{intent_classify, InterruptionIntent};
use crate::core::RunLedger;
use crate::event::{Event, EventBus, ResponseEvent, RunEvent};
use crate::runtime::cancellation::{CancelMode, CancelOutcome};
use crate::runtime::RunStatus;
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
    Stop,
    ContinueWithNote { note: String },
    PauseAndRespond { summary: String },
    CancelAndReplan { reason: String },
    RequireConfirmation { question: String },
}

pub struct RunController {
    run_id: RwLock<Option<Uuid>>,
    state: RwLock<RunState>,
    cancel_token: Mutex<CancellationToken>,
    partial_response: Mutex<String>,
    completed_steps: Mutex<Vec<String>>,
    current_tool: Mutex<Option<ToolCallInfo>>,
    interruption_ctx: Mutex<Option<InterruptionCtx>>,
    original_goal: RwLock<String>,
    event_bus: Arc<EventBus>,
    last_action: std::sync::Mutex<Option<InterruptionAction>>,
    interrupted: AtomicBool,
    ledger: RwLock<Option<RunLedger>>,
    /// Canonical status projected from `state`.
    /// Kept in sync via `set_state()` for migration; new code reads this.
    canonical_status: RwLock<crate::runtime::RunStatus>,
}

impl RunController {
    pub fn new(event_bus: Arc<EventBus>, _response_tx: mpsc::Sender<ResponseEvent>) -> Self {
        Self {
            run_id: RwLock::new(None),
            state: RwLock::new(RunState::Idle),
            cancel_token: Mutex::new(CancellationToken::new()),
            partial_response: Mutex::new(String::new()),
            completed_steps: Mutex::new(Vec::new()),
            current_tool: Mutex::new(None),
            interruption_ctx: Mutex::new(None),
            original_goal: RwLock::new(String::new()),
            event_bus,
            last_action: std::sync::Mutex::new(None),
            interrupted: AtomicBool::new(false),
            ledger: RwLock::new(None),
            canonical_status: RwLock::new(crate::runtime::RunStatus::Created),
        }
    }

    pub fn set_ledger(&self, ledger: RunLedger) {
        *self.ledger.write().unwrap() = Some(ledger);
    }

    pub fn get_ledger(&self) -> Option<RunLedger> {
        self.ledger.read().unwrap().clone()
    }

    pub fn interruption_ctx(&self) -> Option<InterruptionCtx> {
        self.interruption_ctx.lock().unwrap().clone()
    }

    pub fn take_last_action(&self) -> Option<InterruptionAction> {
        self.last_action.lock().unwrap().take()
    }

    pub fn start_run(&self, goal: &str) -> Uuid {
        let run_id = Uuid::new_v4();
        *self.run_id.write().unwrap() = Some(run_id);
        self.set_state(RunState::Thinking);
        *self.original_goal.write().unwrap() = goal.to_string();
        self.interrupted.store(false, Ordering::SeqCst);
        self.partial_response.lock().unwrap().clear();
        self.completed_steps.lock().unwrap().clear();
        self.interruption_ctx.lock().unwrap().take();

        self.event_bus.publish(Event::Run(RunEvent::Started {
            run_id: run_id.to_string(),
            goal: goal.to_string(),
        }));

        // Reset the cancellation token
        *self.cancel_token.lock().unwrap() = CancellationToken::new();

        if let Some(ref ledger) = *self.ledger.read().unwrap() {
            let _ = ledger.record_run_started(&run_id.to_string(), goal);
        }

        run_id
    }

    /// Restore an unfinished run discovered during process startup. The next
    /// user turn resumes this run ID instead of allocating a new one, allowing
    /// the agent loop to load its durable checkpoint and reconcile tool state.
    pub fn restore_interrupted_run(&self, run_id: Uuid, goal: &str) {
        *self.run_id.write().unwrap() = Some(run_id);
        self.set_state(RunState::Interrupted {
            reason: "process_restart_recovery".into(),
        });
        *self.original_goal.write().unwrap() = goal.to_string();
        self.interrupted.store(true, Ordering::SeqCst);
        *self.cancel_token.lock().unwrap() = CancellationToken::new();
        self.event_bus.publish(Event::Run(RunEvent::Interrupted {
            run_id: run_id.to_string(),
            reason: "检测到上次进程退出时未完成的任务；下一条指令将恢复并对账".into(),
        }));
    }

    pub fn has_startup_recovery(&self) -> bool {
        matches!(
            &*self.state.read().unwrap(),
            RunState::Interrupted { reason } if reason == "process_restart_recovery"
        )
    }

    pub fn run_id(&self) -> Option<Uuid> {
        *self.run_id.read().unwrap()
    }

    pub fn active_run_id(&self) -> Uuid {
        self.run_id.read().unwrap().unwrap_or_else(Uuid::new_v4)
    }

    pub fn state(&self) -> RunState {
        self.state.read().unwrap().clone()
    }

    pub fn set_state(&self, state: RunState) {
        *self.canonical_status.write().unwrap() = crate::runtime::RunStatus::from_run_state(&state);
        *self.state.write().unwrap() = state;
    }

    /// Canonical run status (projected from RunState).
    /// New code should prefer this over `state()`.
    pub fn canonical_status(&self) -> crate::runtime::RunStatus {
        *self.canonical_status.read().unwrap()
    }

    /// Derive UI-facing state from canonical status.
    /// This is the single source of truth for AgentState transitions.
    pub fn agent_state(&self) -> crate::event::AgentState {
        crate::event::AgentState::from(self.canonical_status())
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.lock().unwrap().clone()
    }

    pub fn record_partial(&self, delta: &str) {
        self.partial_response.lock().unwrap().push_str(delta);
    }

    pub fn take_partial_response(&self) -> String {
        self.partial_response.lock().unwrap().drain(..).collect()
    }

    pub fn record_completed_step(&self, step: String) {
        self.completed_steps.lock().unwrap().push(step);
    }

    pub fn take_completed_steps(&self) -> Vec<String> {
        self.completed_steps.lock().unwrap().drain(..).collect()
    }

    pub fn set_current_tool(&self, tool: Option<ToolCallInfo>) {
        *self.current_tool.lock().unwrap() = tool;
    }

    pub fn current_tool(&self) -> Option<ToolCallInfo> {
        self.current_tool.lock().unwrap().clone()
    }

    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Called when user submits a message during an active run.
    pub fn interrupt(&self, user_message: &str) -> InterruptionAction {
        // If already interrupted, append message instead of overwriting context.
        if self.interrupted.load(Ordering::SeqCst) {
            if let Some(ref mut ctx) = *self.interruption_ctx.lock().unwrap() {
                ctx.user_message = format!("{}\n{}", ctx.user_message, user_message);
                let intent = intent_classify(&ctx.user_message);
                let state = self.state.read().unwrap().clone();
                let tool = self.current_tool();
                let action = self.determine_action(intent, &state, tool.as_ref());
                *self.last_action.lock().unwrap() = Some(action.clone());
                return action;
            }
        }

        let rid = self
            .run_id
            .read()
            .unwrap()
            .map(|r| r.to_string())
            .unwrap_or_default();

        // Save context before transition
        let partial = self.take_partial_response();
        let steps = self.take_completed_steps();
        let tool = self.current_tool();
        let goal = self.original_goal.read().unwrap().clone();
        let state_str = format!("{:?}", self.state.read().unwrap());
        let original_state = self.state.read().unwrap().clone(); // snapshot before overwrite

        // Mark interrupted
        self.interrupted.store(true, Ordering::SeqCst);
        self.set_state(RunState::Interrupted {
            reason: "user_interjection".into(),
        });

        // Cancel current LLM request (best-effort)
        self.cancel_token.lock().unwrap().cancel();

        // Store interruption context
        *self.interruption_ctx.lock().unwrap() = Some(InterruptionCtx {
            original_goal: goal.clone(),
            completed_steps: steps.clone(),
            current_state: state_str.clone(),
            active_tool: tool.clone(),
            partial_response: partial.clone(),
            user_message: user_message.to_string(),
        });

        // Publish event
        self.event_bus.publish(Event::Run(RunEvent::Interrupted {
            run_id: rid.clone(),
            reason: "user_interjection".into(),
        }));

        if let Some(ref ledger) = *self.ledger.read().unwrap() {
            let _ = ledger.record_run_interrupted(&rid, "user_interjection");
        }

        // Classify intent
        let intent = intent_classify(user_message);

        // Determine action using original state, not the overwritten Interrupted
        let action = self.determine_action(intent, &original_state, tool.as_ref());
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
            InterruptionIntent::Stop => InterruptionAction::Stop,
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
        let ctx = self.interruption_ctx.lock().unwrap().take()?;

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
        let run_id = self.run_id.read().unwrap().unwrap_or_else(Uuid::new_v4);
        *self.cancel_token.lock().unwrap() = CancellationToken::new();
        self.interrupted.store(false, Ordering::SeqCst);
        self.set_state(RunState::Resuming);
        self.event_bus.publish(Event::Run(RunEvent::Resuming {
            run_id: run_id.to_string(),
        }));
        if let Some(ref ledger) = *self.ledger.read().unwrap() {
            let _ = ledger.record_run_resumed(&run_id.to_string());
        }
        run_id
    }

    pub async fn finish_run(&self, stop_reason: &str, outcome: Option<&str>) {
        self.set_state(RunState::Finished {
            stop_reason: stop_reason.into(),
        });
        self.interrupted.store(false, Ordering::SeqCst);
        if let Some(rid) = *self.run_id.read().unwrap() {
            self.event_bus.publish(Event::Run(RunEvent::Finished {
                run_id: rid.to_string(),
                stop_reason: stop_reason.into(),
            }));
            if let Some(ref ledger) = *self.ledger.read().unwrap() {
                let _ = ledger.record_run_finished(
                    &rid.to_string(),
                    stop_reason,
                    outcome.unwrap_or(""),
                );
            }
        }
    }

    pub fn idempotency_key(tool_name: &str, args: &str) -> String {
        let arguments = if args.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(args)
                .unwrap_or_else(|_| serde_json::json!({"__parse_error_raw_arguments": args}))
        };
        let observable = crate::tool::ObservableToolInput::new(tool_name, arguments);
        let key = crate::tool::ToolReplayKey::from_observable(&observable);
        format!("{}:{}", key.tool_name, key.arguments_hash)
    }

    pub fn record_tool_call_start(&self, tool_name: &str, args: &str) {
        if let Some(rid) = *self.run_id.read().unwrap() {
            if let Some(ref ledger) = *self.ledger.read().unwrap() {
                let key = Self::idempotency_key(tool_name, args);
                let _ = ledger.record_tool_call(
                    &rid.to_string(),
                    tool_name,
                    args,
                    "started",
                    None,
                    Some(&key),
                );
            }
        }
    }

    pub fn record_tool_call_end(
        &self,
        tool_name: &str,
        args: &str,
        status: &str,
        error: Option<&str>,
    ) {
        if let Some(rid) = *self.run_id.read().unwrap() {
            if let Some(ref ledger) = *self.ledger.read().unwrap() {
                let key = Self::idempotency_key(tool_name, args);
                let _ = ledger.record_tool_call(
                    &rid.to_string(),
                    tool_name,
                    args,
                    status,
                    error,
                    Some(&key),
                );
            }
        }
    }

    /// Check if a tool call (identified by name + canonical args) was
    /// already completed in this run.  Uses the ledger if available.
    /// Returns `false` if the ledger is not set or query fails
    /// (optimistically allows the tool to run).
    pub fn is_tool_completed(&self, tool_name: &str, args: &str) -> bool {
        let rid = match *self.run_id.read().unwrap() {
            Some(rid) => rid.to_string(),
            None => return false,
        };
        let ledger = match *self.ledger.read().unwrap() {
            Some(ref l) => l.clone(),
            None => return false,
        };
        let key = Self::idempotency_key(tool_name, args);
        ledger.is_tool_completed(&rid, &key).unwrap_or(false)
    }

    /// Unified cancel entry point.
    ///
    /// - `Graceful`: cancels the CancellationToken and transitions state.
    /// - `ForceAfterTimeout`: same as Graceful; the caller is responsible
    ///   for the force-abort fallback (e.g. JoinHandle::abort()).
    ///
    /// Returns `CancelOutcome::NoActiveRun` if there is no run to cancel.
    pub fn request_cancel(&self, mode: CancelMode) -> CancelOutcome {
        let rid = match *self.run_id.read().unwrap() {
            Some(rid) => rid,
            None => return CancelOutcome::NoActiveRun,
        };
        let rid_str = rid.to_string();

        // Stop the CancellationToken so the current Attempt sees it.
        self.cancel_token.lock().unwrap().cancel();

        // Canonical status.
        *self.canonical_status.write().unwrap() = RunStatus::Cancelled;
        *self.state.write().unwrap() = RunState::Interrupted {
            reason: match mode {
                CancelMode::Graceful => "user_cancelled".into(),
                CancelMode::ForceAfterTimeout { .. } => "timeout".into(),
            },
        };
        self.interrupted.store(true, Ordering::SeqCst);

        self.event_bus.publish(Event::Run(RunEvent::Interrupted {
            run_id: rid_str.clone(),
            reason: "user_cancelled".into(),
        }));

        if let Some(ref ledger) = *self.ledger.read().unwrap() {
            let _ = ledger.record_run_interrupted(&rid_str, "user_cancelled");
        }

        CancelOutcome::Accepted
    }

    pub fn record_approval(&self, tool: &str, decision: &str) {
        if let Some(rid) = *self.run_id.read().unwrap() {
            if let Some(ref ledger) = *self.ledger.read().unwrap() {
                let _ = ledger.record_approval(&rid.to_string(), tool, decision);
            }
        }
    }
}

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
        assert!(matches!(action, InterruptionAction::Stop));

        // Interruption context should contain partial response
        let ctx_set = ctrl.interruption_ctx.lock().unwrap().is_some();
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
    fn test_intent_stop_maps_to_stop_action() {
        let (ctrl, _) = make_controller();
        ctrl.start_run("test");

        let action = ctrl.interrupt("停");
        assert!(matches!(action, InterruptionAction::Stop));
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

    #[test]
    fn idempotency_key_canonicalizes_json_object_order() {
        assert_eq!(
            RunController::idempotency_key("tool", r#"{"a":1,"b":2}"#),
            RunController::idempotency_key("tool", r#"{"b":2,"a":1}"#),
        );
    }

    #[test]
    fn startup_recovery_preserves_run_id() {
        let (ctrl, _) = make_controller();
        let run_id = Uuid::new_v4();
        ctrl.restore_interrupted_run(run_id, "recover me");

        assert!(ctrl.has_startup_recovery());
        assert_eq!(ctrl.begin_resume(), run_id);
        assert_eq!(ctrl.run_id(), Some(run_id));
    }
}
