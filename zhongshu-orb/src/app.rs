use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

const KEEP_COMPLETED_GOALS: usize = 20;
const AGENT_TIMEOUT: Duration = Duration::from_secs(300);
use crate::agent::AgentMemory;
use crate::config;
use tokio::sync::RwLock;
use uuid::Uuid;
use zhongshu_core::agent::llm::LlmProvider;
use zhongshu_core::agent::loop_::ToolCompletionStatus;
use zhongshu_core::agent::run::RunController;
use zhongshu_core::agent::{
    execute_agent_loop, AgentBudget, AgentCallbacks, AgentProfile, AgentRuntime, LoopResult,
    ModelRouter, RunOutcome, Worker,
};
use zhongshu_core::core::checkpoint::CheckpointStore;
use zhongshu_core::core::context::{
    ContextMessage, ContextPack, ContextPackBuilder, ContextRole, RecentUnit,
};
use zhongshu_core::core::{Database, RunbookStore};
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, HarnessUiEvent, MessageId, ResponseEvent,
    ResponseRole, ResponseTx, ToolEvent,
};
use zhongshu_core::harness::trace::runbook::events_to_runbook;
use zhongshu_core::integration::DeeplosslessProxy;
use zhongshu_core::patch::PatchDiffPayload;
use zhongshu_core::runtime::ExecutionProfile;
use zhongshu_core::task::TaskQueue;
use zhongshu_core::tool::ToolRegistry;

pub(crate) fn publish_harness_events(
    eb: &EventBus,
    events: &[zhongshu_core::harness::trace::event::HarnessEvent],
) {
    for event in events {
        match event {
            zhongshu_core::harness::trace::event::HarnessEvent::CodingSessionStarted {
                session_id,
                trace_id,
                intent,
                model,
                deeplossless_conversation_id,
                deeplossless_replay_execution_id,
                ..
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::CodingSessionStarted {
                    session_id: session_id.clone(),
                    trace_id: trace_id.clone(),
                    intent: intent.clone(),
                    model: model.clone(),
                    deeplossless_conversation_id: *deeplossless_conversation_id,
                    deeplossless_replay_execution_id: deeplossless_replay_execution_id.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::CodingPlanCreated {
                session_id,
                step_count,
                risk,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::CodingPlanCreated {
                    session_id: session_id.clone(),
                    step_count: *step_count,
                    risk: risk.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::CodingStepStarted {
                session_id,
                step_id,
                kind,
                title,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::CodingStepStarted {
                    session_id: session_id.clone(),
                    step_id: step_id.clone(),
                    kind: kind.clone(),
                    title: title.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::CodingStepCompleted {
                session_id,
                step_id,
                status,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::CodingStepCompleted {
                    session_id: session_id.clone(),
                    step_id: step_id.clone(),
                    status: status.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::WorkerStarted {
                session_id,
                worker,
                task_id,
                owned_files,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::WorkerStarted {
                    session_id: session_id.clone(),
                    worker: worker.clone(),
                    task_id: task_id.clone(),
                    owned_files: owned_files.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::WorkerCompleted {
                session_id,
                worker,
                task_id,
                success,
                status,
                trace_event_count,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::WorkerCompleted {
                    session_id: session_id.clone(),
                    worker: worker.clone(),
                    task_id: task_id.clone(),
                    success: *success,
                    status: status.clone(),
                    trace_event_count: *trace_event_count,
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::WorkerConflict {
                session_id,
                worker,
                task_id,
                reason,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::WorkerConflict {
                    session_id: session_id.clone(),
                    worker: worker.clone(),
                    task_id: task_id.clone(),
                    reason: reason.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::PatchPreview {
                session_id,
                path,
                operation,
                diff_summary,
                diff,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::PatchPreview {
                    session_id: session_id.clone(),
                    path: path.clone(),
                    operation: operation.clone(),
                    diff_summary: diff_summary.clone(),
                    diff: diff.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::PatchApplied {
                session_id,
                path,
                operation,
                changed,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::PatchApplied {
                    session_id: session_id.clone(),
                    path: path.clone(),
                    operation: operation.clone(),
                    changed: *changed,
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::FileEdit { path, diff, .. } => {
                let display_path = if path.as_os_str().is_empty() {
                    PathBuf::from("workspace")
                } else {
                    path.clone()
                };
                eb.publish(Event::Harness(HarnessUiEvent::PatchPreview {
                    session_id: None,
                    path: display_path,
                    operation: "file_edit".into(),
                    diff_summary: diff
                        .as_deref()
                        .unwrap_or("mutation without captured diff")
                        .lines()
                        .next()
                        .unwrap_or("mutation without captured diff")
                        .to_string(),
                    diff: file_edit_patch_payload(diff.as_ref()),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::ContextIncluded {
                description,
                estimated_tokens,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::ContextIncluded {
                    description: description.clone(),
                    estimated_tokens: *estimated_tokens,
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::ContextPressure {
                pressure_percent,
                dropped_evidence,
                dropped_recent,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::ContextPressure {
                    pressure_percent: *pressure_percent,
                    dropped_evidence: *dropped_evidence,
                    dropped_recent: *dropped_recent,
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::ReplayAvailable {
                conversation_id,
                replay_execution_id,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::ReplayAvailable {
                    conversation_id: *conversation_id,
                    replay_execution_id: replay_execution_id.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::Verification {
                command,
                success,
                exit_code,
                step,
                ..
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::Verification {
                    command: command.clone(),
                    success: *success,
                    exit_code: *exit_code,
                    step: *step,
                    file_locations: None,
                    suggestion: None,
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::RecoveryFeedback {
                rule_id,
                message,
            } => {
                eb.publish(Event::Harness(HarnessUiEvent::RecoveryFeedback {
                    rule_id: rule_id.clone(),
                    message: message.clone(),
                }));
            }
            zhongshu_core::harness::trace::event::HarnessEvent::PhaseTransition { from, to } => {
                eb.publish(Event::Harness(HarnessUiEvent::PhaseTransition {
                    from: from.clone(),
                    to: to.clone(),
                }));
            }
            _ => {}
        }
    }
}

fn file_edit_patch_payload(diff: Option<&String>) -> Option<PatchDiffPayload> {
    diff.map(|diff| {
        if diff.starts_with('<') && diff.ends_with('>') {
            PatchDiffPayload::from_summary(diff.clone())
        } else {
            PatchDiffPayload::from_unified_diff(diff.clone())
        }
    })
}

// ── Session persistence ─────────────────────────────────────────────

#[derive(Clone)]
pub struct SessionState {
    #[allow(dead_code)]
    pub conv_id: Arc<tokio::sync::Mutex<i64>>,
}

impl SessionState {
    pub fn new() -> Self {
        SessionState {
            conv_id: Arc::new(tokio::sync::Mutex::new(1)),
        }
    }
}

// ── Agent controller ───────────────────────────────────────────────

pub struct AgentController {
    event_bus: Arc<EventBus>,
    response_tx: ResponseTx,
    provider: Mutex<Arc<dyn LlmProvider>>,
    base_tools: ToolRegistry,
    tools: Mutex<ToolRegistry>,
    model: Mutex<String>,
    #[allow(dead_code)]
    session: SessionState,
    base_system_prompt: Mutex<String>,
    system_prompt: Mutex<String>,
    history: Arc<Mutex<Vec<(String, String)>>>,
    state: Arc<RwLock<AgentState>>,
    memory: AgentMemory,
    current_task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    proxy: Arc<tokio::sync::Mutex<DeeplosslessProxy>>,
    router: Mutex<ModelRouter>,
    reasoning_complex: Mutex<String>,
    reasoning_agent: Mutex<String>,
    equipment: Arc<Mutex<zhongshu_core::equipment::EquipmentRegistry>>,
    core_db_path: PathBuf,
    max_context_tokens: AtomicU32,
    pub auto_evolve_enabled: AtomicBool,
    pub mode: Mutex<String>,
    pub run_controller: Arc<RunController>,
}

impl AgentController {
    pub fn new(
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        provider: Arc<dyn LlmProvider>,
        base_tools: ToolRegistry,
        tools: ToolRegistry,
        model: String,
        #[allow(dead_code)] session: SessionState,
        base_system_prompt: String,
        system_prompt: String,
        profile_path: PathBuf,
        proxy: Arc<tokio::sync::Mutex<DeeplosslessProxy>>,
        router: ModelRouter,
        reasoning_complex: String,
        reasoning_agent: String,
        max_context_tokens: u32,
        equipment: Arc<Mutex<zhongshu_core::equipment::EquipmentRegistry>>,
        core_db_path: PathBuf,
    ) -> Self {
        let memory = AgentMemory::load(&profile_path);
        let run_controller = Arc::new(RunController::new(event_bus.clone(), response_tx.clone()));
        AgentController {
            event_bus,
            response_tx,
            provider: Mutex::new(provider),
            base_tools,
            tools: Mutex::new(tools),
            model: Mutex::new(model),
            session,
            base_system_prompt: Mutex::new(base_system_prompt),
            system_prompt: Mutex::new(system_prompt),
            history: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(RwLock::new(AgentState::Idle)),
            memory,
            current_task: Arc::new(tokio::sync::Mutex::new(None)),
            proxy,
            router: Mutex::new(router),
            reasoning_complex: Mutex::new(reasoning_complex),
            reasoning_agent: Mutex::new(reasoning_agent),
            core_db_path,
            max_context_tokens: AtomicU32::new(max_context_tokens),
            auto_evolve_enabled: AtomicBool::new(false),
            mode: Mutex::new("assistant".into()),
            equipment,
            run_controller,
        }
    }

    /// Shared state for external consumers (UI, background runner).
    #[allow(dead_code)]
    pub fn state(&self) -> Arc<RwLock<AgentState>> {
        self.state.clone()
    }

    /// Update the base system prompt (e.g. after personality change).
    /// Skill prompts are automatically re-applied.
    pub fn set_system_prompt(&self, prompt: String) {
        *self.base_system_prompt.lock().unwrap() = prompt;
        self.refresh_skill_prompts();
    }

    /// Rebuild system prompt by appending current skill prompts.
    pub fn refresh_skill_prompts(&self) {
        let base = self.base_system_prompt.lock().unwrap().clone();
        let mut full = base;
        if let Ok(reg) = self.equipment.lock() {
            let current_mode = self.mode.lock().unwrap().clone();
            for (_id, prompt) in &reg.skill_prompts() {
                // Simple mode filter: skip skills tagged for other mode.
                let is_coding = prompt.contains("[coding]");
                let is_assistant = prompt.contains("[assistant]");
                if (current_mode == "coding" && is_assistant)
                    || (current_mode != "coding" && is_coding)
                {
                    continue;
                }
                full.push_str("\n\n");
                full.push_str(prompt);
            }
        }
        *self.system_prompt.lock().unwrap() = full;
    }

    pub fn set_mode(&self, mode: String) {
        *self.mode.lock().unwrap() = mode;
        self.refresh_skill_prompts();
    }

    pub fn set_max_context_tokens(&self, val: u32) {
        self.max_context_tokens.store(val, Ordering::Relaxed);
        tracing::info!("max_context_tokens updated to {val}");
    }

    pub fn set_auto_evolve(&self, enabled: bool) {
        self.auto_evolve_enabled.store(enabled, Ordering::Relaxed);
        tracing::info!(
            "auto_evolve {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    pub fn model_name(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    pub fn is_idle(&self) -> bool {
        self.state
            .try_read()
            .map(|state| matches!(*state, AgentState::Idle))
            .unwrap_or(false)
    }

    pub fn provider_snapshot(&self) -> Arc<dyn LlmProvider> {
        self.provider.lock().unwrap().clone()
    }

    pub fn update_llm_runtime(
        &self,
        provider: Arc<dyn LlmProvider>,
        model: String,
        router: ModelRouter,
        reasoning_complex: String,
        reasoning_agent: String,
    ) {
        *self.provider.lock().unwrap() = provider;
        *self.model.lock().unwrap() = model;
        *self.router.lock().unwrap() = router;
        *self.reasoning_complex.lock().unwrap() = reasoning_complex;
        *self.reasoning_agent.lock().unwrap() = reasoning_agent;
        tracing::info!("chat LLM runtime updated");
    }

    pub async fn rebuild_equipment_tools_with_mcp(&self) {
        let mut tools = self.base_tools.clone();
        let reports = if let Ok(equipment) = self.equipment.lock() {
            equipment.register_tools(&mut tools);
            equipment.register_mcp_tools(&mut tools).await
        } else {
            Vec::new()
        };
        *self.tools.lock().unwrap() = tools;
        for report in reports {
            if let Some(error) = report.error {
                tracing::warn!("MCP server '{}' skipped: {}", report.server_id, error);
            }
        }
        tracing::info!("chat tool registry rebuilt from active equipment and MCP servers");
    }

    pub fn set_chat_history(&self, history: Vec<(String, String)>) {
        *self.history.lock().unwrap() = history;
    }

    /// Cancel the currently running agent task.
    /// Uses the unified graceful cancel path so in-flight tools with
    /// side effects are reconciled before the run finishes.
    pub fn cancel(&self) {
        self.run_controller
            .request_cancel(zhongshu_core::runtime::cancellation::CancelMode::Graceful);
    }

    pub(crate) fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// Run an agent turn for the given input.  Non‑blocking — spawns
    /// the actual work on the tokio runtime and returns immediately.
    /// If the agent is already busy, the input is treated as an interruption.
    pub fn run(&self, input: String) {
        if !self.try_claim() {
            tracing::debug!("agent busy, interrupting current run");
            self.run_controller.interrupt(&input);
            return;
        }

        // User approval keywords → approve pending authority requests.
        let trimmed = input.trim().to_lowercase();
        if matches!(
            trimmed.as_str(),
            "yes" | "y" | "可以" | "确认" | "同意" | "好" | "是"
        ) {
            if let Some(req) = zhongshu_core::authority::peek_pending() {
                self.run_controller.record_approval(&req.tool, "approved");
                zhongshu_core::authority::approve_pending(&req.id);
            }
        }

        if self.run_controller.has_startup_recovery() {
            self.run_controller.begin_resume();
        } else {
            self.run_controller.start_run(&input);
        }
        self.emit_start(&input);
        self.spawn_task(input);
    }

    // ── internal helpers ───────────────────────────────────────────

    fn try_claim(&self) -> bool {
        self.state
            .try_write()
            .map(|mut s| {
                if matches!(*s, AgentState::Idle) {
                    *s = AgentState::from(zhongshu_core::runtime::RunStatus::Running);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    fn emit_start(&self, input: &str) {
        let uid = MessageId::new();
        let rid = self.run_controller.active_run_id();
        let _ = self.response_tx.try_send(ResponseEvent::MessageStarted {
            id: uid,
            role: ResponseRole::User,
            run_id: rid,
        });
        let _ = self.response_tx.try_send(ResponseEvent::MessageDelta {
            id: uid,
            delta: input.to_string(),
            run_id: rid,
        });
        let _ = self.response_tx.try_send(ResponseEvent::MessageCompleted {
            id: uid,
            run_id: rid,
        });

        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Idle,
                to: self.run_controller.agent_state(),
            }));
    }

    fn spawn_task(&self, input: String) {
        let rc = self.run_controller.clone();
        let eb = self.event_bus.clone();
        let tx = self.response_tx.clone();
        let t = self.tools.lock().unwrap().clone();
        let sys = self.system_prompt.lock().unwrap().clone();
        let history_arc = self.history.clone();
        let memory = self.memory.clone();
        let state_arc = self.state.clone();
        let core_db_path = self.core_db_path.clone();

        // Determine routed model + reasoning effort.
        let provider_snapshot = self.provider.lock().unwrap().clone();
        let model_snapshot = self.model.lock().unwrap().clone();
        let (routed_model, routed_effort) = {
            let router = self.router.lock().unwrap();
            let (model, effort) = router.route(&input);
            (model, effort.map(str::to_string))
        };
        let reasoning_str: Option<String> = match routed_effort.as_deref() {
            Some("high") => Some(self.reasoning_complex.lock().unwrap().clone()),
            Some("max") => Some(self.reasoning_agent.lock().unwrap().clone()),
            _ => None,
        };
        let p = if routed_model != model_snapshot {
            provider_snapshot.change_model(&routed_model)
        } else {
            provider_snapshot
        };
        let m = routed_model;
        let max_ctx = self.max_context_tokens.load(Ordering::Relaxed);
        let proxy = self.proxy.clone();

        // Snapshot profile for the prompt — non‑blocking read.
        let state_block = memory.to_state_block();

        // Select budget by mode.
        let mode_str = self.mode.lock().unwrap().clone();
        let budget = match mode_str.as_str() {
            "coding" => AgentBudget::coding_default(),
            _ => AgentBudget::assistant_default(),
        };
        let budget = AgentBudget {
            token_limit: (max_ctx as usize).min(budget.token_limit),
            ..budget
        };

        let handle = tokio::spawn(async move {
            let aid = MessageId::new();
            let run_id = rc.active_run_id();
            let _ = tx
                .send(ResponseEvent::MessageStarted {
                    id: aid,
                    role: ResponseRole::Assistant,
                    run_id,
                })
                .await;

            // Context compression: drop oldest history pairs when over 80%.
            if max_ctx > 0 {
                let trigger = (max_ctx as f64 * 0.8) as usize;
                let base_est = (sys.len() / 4) + 1 + (input.len() / 4) + 1;

                // Compute how many to drop — history lock is scoped so it's
                // released before the async proxy lock below.
                let dropped = {
                    let mut history = history_arc.lock().unwrap();
                    compress_history(&mut history, base_est, trigger)
                };
                if dropped > 0 {
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: format!("\n——压缩中(已归档{dropped}条)——\n\n"),
                            run_id,
                        })
                        .await;
                    // Best-effort deeplossless DAG compression before discarding.
                    let proxy_guard = proxy.lock().await;
                    let compressed = proxy_guard
                        .compress_oldest_leaves(dropped)
                        .await
                        .unwrap_or(0);
                    if compressed > 0 {
                        tracing::info!(
                            "deeplossless compressed {compressed} leaves, dropped {dropped} from history"
                        );
                    } else {
                        tracing::info!("compressed context: dropped {dropped} messages (deeplossless unavailable)");
                    }
                }
            }

            let recent: Vec<RecentUnit> = {
                let history = history_arc.lock().unwrap();
                let mut units = Vec::new();
                let mut i = 0;
                while i < history.len() {
                    let (role, content) = &history[i];
                    if role == "user" {
                        if i + 1 < history.len() && history[i + 1].0 == "assistant" {
                            let assistant_content = history[i + 1].1.clone();
                            units.push(RecentUnit::UserAssistant {
                                user: ContextMessage {
                                    role: ContextRole::User,
                                    content: content.clone(),
                                    tool_call_id: None,
                                    tool_calls: vec![],
                                },
                                assistant: Some(ContextMessage {
                                    role: ContextRole::Assistant,
                                    content: assistant_content,
                                    tool_call_id: None,
                                    tool_calls: vec![],
                                }),
                            });
                            i += 2;
                        } else {
                            units.push(RecentUnit::Single(ContextMessage {
                                role: ContextRole::User,
                                content: content.clone(),
                                tool_call_id: None,
                                tool_calls: vec![],
                            }));
                            i += 1;
                        }
                    } else {
                        units.push(RecentUnit::Single(ContextMessage {
                            role: ContextRole::Assistant,
                            content: content.clone(),
                            tool_call_id: None,
                            tool_calls: vec![],
                        }));
                        i += 1;
                    }
                }
                units
            };

            // Clone state_block and recent for potential recovery re-run
            let recovery_state = state_block.clone();
            let recovery_recent = recent.clone();

            let (context_pack, report) = match ContextPackBuilder::new()
                .stable_system(sys.clone())
                .state(state_block)
                .with_evidence(Vec::new())
                .with_recent(recent)
                .input(input.clone())
                .build(max_ctx as usize)
            {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("ContextPack build error: {}", e);
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: format!("context build error: {e}"),
                            run_id,
                        })
                        .await;
                    let _ = tx
                        .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                        .await;
                    return;
                }
            };

            tracing::debug!(
                "ContextPack: sys={} state={} ev={} recent={} input={} total={} hash={}",
                report.stable_system_tokens,
                report.state_tokens,
                report.evidence_tokens,
                report.recent_tokens,
                report.input_tokens,
                report.total_tokens,
                report.stable_prefix_hash,
            );

            // Clone provider, model, toolset, budget, reasoning for potential recovery re-run
            let recovery_provider = p.clone();
            let recovery_model = m.clone();
            let recovery_tools = t.clone();
            let recovery_budget = budget.clone();
            let recovery_reasoning = reasoning_str.clone();

            // ── Execute initial attempt via shared kernel ──────────
            let attempt_svc = RunAttemptServices {
                event_bus: eb.clone(),
                response_tx: tx.clone(),
                core_db_path: core_db_path.clone(),
                run_controller: rc.clone(),
            };
            #[allow(unused_assignments)]
            let mut stop_reason = String::new();
            #[allow(unused_assignments)]
            let mut overall_success = false;

            let model_name = m.clone();
            let attempt_budget = budget.clone();
            match run_attempt(
                RunAttemptRequest {
                    run_id,
                    input: input.clone(),
                    context_pack,
                    provider: p,
                    model: m,
                    tools: t,
                    budget,
                    reasoning_effort: reasoning_str,
                    message_id: aid,
                    profile: ExecutionProfile::Resumable,
                },
                attempt_svc,
            )
            .await
            {
                Ok(attempt_result) => {
                    let rr = &attempt_result.loop_result;
                    if !rc.is_interrupted() {
                        let conversation_id = proxy.lock().await.current_conv_id().await;
                        persist_trace_runbook(
                            core_db_path.clone(),
                            &input,
                            &rr.trace_events,
                            conversation_id,
                        );
                        publish_harness_events(&eb, &rr.trace_events);
                        let last = rr.messages.last().map(|x| x.content.as_str()).unwrap_or("");
                        history_arc
                            .lock()
                            .unwrap()
                            .push(("user".to_string(), input.clone()));
                        if !last.is_empty() {
                            let tools_used = &attempt_result.tool_names;
                            let history_content = if tools_used.is_empty() {
                                last.to_string()
                            } else {
                                let mut deduped: Vec<(&str, u32)> = Vec::new();
                                for name in tools_used.iter().map(|s| s.as_str()) {
                                    if let Some(last) = deduped.last_mut() {
                                        if last.0 == name {
                                            last.1 += 1;
                                            continue;
                                        }
                                    }
                                    deduped.push((name, 1));
                                }
                                let badge = deduped
                                    .iter()
                                    .map(|(n, c)| {
                                        if *c > 1 {
                                            format!("✓ {n} ×{c}")
                                        } else {
                                            format!("✓ {n}")
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                                format!("[工具: {badge}]\n\n{last}")
                            };
                            history_arc
                                .lock()
                                .unwrap()
                                .push(("assistant".to_string(), history_content));
                        }
                        memory.extract_todos(last);
                        if rr.outcome == RunOutcome::CompletedVerified {
                            memory.extract_goal_completions(last);
                            memory.archive_completed_goals(KEEP_COMPLETED_GOALS);
                        }
                        stop_reason = format!("{:?}", rr.stop_reason);
                        overall_success = rr.outcome == RunOutcome::CompletedVerified;
                        let receipt = zhongshu_core::core::receipt::RunReceipt::from_loop_result(
                            rr,
                            &run_id.to_string(),
                            &model_name,
                            &attempt_budget,
                            0,
                            vec![],
                            vec![],
                            false,
                        );
                        tracing::info!(
                            run_id = %receipt.run_id,
                            outcome = %receipt.stop_reason,
                            tools = receipt.tool_calls_made,
                            tokens = receipt.estimated_tokens,
                            "run receipt"
                        );
                    } else {
                        stop_reason = "interrupted".to_string();
                        overall_success = false;
                    }
                    let _ = tx
                        .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                        .await;
                    *state_arc.write().await = AgentState::Done {
                        success: overall_success,
                    };
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done {
                            success: overall_success,
                        },
                    }));
                }
                Err(RunAttemptError::AgentError(e)) => {
                    tracing::error!("agent run failed: {e:#}");
                    stop_reason = "error".to_string();
                    overall_success = false;
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: format!("{e:#}"),
                            run_id,
                        })
                        .await;
                    let _ = tx
                        .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                        .await;
                    *state_arc.write().await = AgentState::Done { success: false };
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: false },
                    }));
                }
                Err(RunAttemptError::Timeout) => {
                    tracing::warn!("agent task timed out after 300s");
                    stop_reason = "timeout".to_string();
                    overall_success = false;
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: "[连接超时: 300s 无响应]".into(),
                            run_id,
                        })
                        .await;
                    let _ = tx
                        .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                        .await;
                    *state_arc.write().await = AgentState::Done { success: false };
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: false },
                    }));
                }
            }

            // ── Recovery after interruption ──────────────────────────
            if rc.is_interrupted() {
                let action = rc.take_last_action();
                match action {
                    Some(zhongshu_core::agent::run::InterruptionAction::ContinueWithNote {
                        ..
                    }) => {
                        if let Some(prompt) = rc.build_recovery_prompt() {
                            tracing::info!("agent interrupted, performing recovery re-run");
                            let recovery_input = format!(
                                "[恢复]\n用户插话。请你先自然回应新消息，然后根据当前状态决定是继续还是调整方案。\n\n{prompt}"
                            );
                            if let Ok((recovery_pack, _)) = ContextPackBuilder::new()
                                .stable_system(sys)
                                .state(recovery_state)
                                .with_evidence(Vec::new())
                                .with_recent(recovery_recent)
                                .input(recovery_input)
                                .build(max_ctx as usize)
                            {
                                rc.set_state(zhongshu_core::agent::run::RunState::Resuming);
                                let _run_id = rc.begin_resume();
                                let recovery_svc = RunAttemptServices {
                                    event_bus: eb.clone(),
                                    response_tx: tx.clone(),
                                    core_db_path: core_db_path.clone(),
                                    run_controller: rc.clone(),
                                };
                                match run_attempt(
                                    RunAttemptRequest {
                                        run_id,
                                        input: input.clone(),
                                        context_pack: recovery_pack,
                                        provider: recovery_provider,
                                        model: recovery_model,
                                        tools: recovery_tools,
                                        budget: recovery_budget,
                                        reasoning_effort: recovery_reasoning,
                                        message_id: aid,
                                        profile: ExecutionProfile::Resumable,
                                    },
                                    recovery_svc,
                                )
                                .await
                                {
                                    Ok(attempt_result) => {
                                        let rr = &attempt_result.loop_result;
                                        let conversation_id =
                                            proxy.lock().await.current_conv_id().await;
                                        persist_trace_runbook(
                                            core_db_path.clone(),
                                            &input,
                                            &rr.trace_events,
                                            conversation_id,
                                        );
                                        publish_harness_events(&eb, &rr.trace_events);
                                        let last = rr
                                            .messages
                                            .last()
                                            .map(|x| x.content.as_str())
                                            .unwrap_or("");
                                        history_arc
                                            .lock()
                                            .unwrap()
                                            .push(("user".to_string(), input.clone()));
                                        if !last.is_empty() {
                                            let tools_used = &attempt_result.tool_names;
                                            let history_content = if tools_used.is_empty() {
                                                last.to_string()
                                            } else {
                                                let mut deduped: Vec<(&str, u32)> = Vec::new();
                                                for name in tools_used.iter().map(|s| s.as_str()) {
                                                    if let Some(last) = deduped.last_mut() {
                                                        if last.0 == name {
                                                            last.1 += 1;
                                                            continue;
                                                        }
                                                    }
                                                    deduped.push((name, 1));
                                                }
                                                let badge = deduped
                                                    .iter()
                                                    .map(|(n, c)| {
                                                        if *c > 1 {
                                                            format!("✓ {n} ×{c}")
                                                        } else {
                                                            format!("✓ {n}")
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(" · ");
                                                format!("[工具: {badge}]\n\n{last}")
                                            };
                                            history_arc
                                                .lock()
                                                .unwrap()
                                                .push(("assistant".to_string(), history_content));
                                        }
                                        memory.extract_todos(last);
                                        if rr.outcome == RunOutcome::CompletedVerified {
                                            memory.extract_goal_completions(last);
                                            memory.archive_completed_goals(KEEP_COMPLETED_GOALS);
                                        }
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        stop_reason = format!("{:?}", rr.stop_reason);
                                        overall_success =
                                            rr.outcome == RunOutcome::CompletedVerified;
                                        let outcome_state = match rr.outcome {
                                            RunOutcome::CompletedVerified => {
                                                AgentState::Done { success: true }
                                            }
                                            RunOutcome::CompletedUnverified => {
                                                AgentState::Submitted
                                            }
                                            _ => AgentState::Done { success: false },
                                        };
                                        *state_arc.write().await = outcome_state;
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: outcome_state,
                                        }));
                                    }
                                    Err(RunAttemptError::AgentError(e)) => {
                                        tracing::error!("recovery agent run failed: {e:#}");
                                        stop_reason = "recovery_failed".to_string();
                                        overall_success = false;
                                        let _ = tx
                                            .send(ResponseEvent::MessageDelta {
                                                id: aid,
                                                delta: format!("{e:#}"),
                                                run_id,
                                            })
                                            .await;
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        *state_arc.write().await =
                                            AgentState::Done { success: false };
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: AgentState::Done { success: false },
                                        }));
                                    }
                                    Err(RunAttemptError::Timeout) => {
                                        tracing::warn!("recovery agent task timed out");
                                        stop_reason = "recovery_timeout".to_string();
                                        overall_success = false;
                                        let _ = tx
                                            .send(ResponseEvent::MessageDelta {
                                                id: aid,
                                                delta: "[恢复超时]".into(),
                                                run_id,
                                            })
                                            .await;
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        *state_arc.write().await =
                                            AgentState::Done { success: false };
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: AgentState::Done { success: false },
                                        }));
                                    }
                                }
                            }
                        }
                    }
                    Some(zhongshu_core::agent::run::InterruptionAction::Stop) => {
                        tracing::info!("interruption stopped");
                        stop_reason = "cancelled".to_string();
                        overall_success = false;
                        let _ = tx
                            .send(ResponseEvent::MessageDelta {
                                id: aid,
                                delta: "[已停止]".into(),
                                run_id,
                            })
                            .await;
                        let _ = tx
                            .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                            .await;
                        *state_arc.write().await = AgentState::Done { success: false };
                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                            from: AgentState::Thinking,
                            to: AgentState::Done { success: false },
                        }));
                    }
                    Some(zhongshu_core::agent::run::InterruptionAction::CancelAndReplan {
                        reason,
                    }) => {
                        tracing::info!("interruption: {reason} — replanning");
                        if let Some(prompt) = rc.build_recovery_prompt() {
                            let recovery_input = format!(
                                "[重新规划]\n用户改变了方向。请优先处理用户的新消息，根据新方向重新规划任务。\n\n{prompt}"
                            );
                            if let Ok((recovery_pack, _)) = ContextPackBuilder::new()
                                .stable_system(sys)
                                .state(recovery_state)
                                .with_evidence(Vec::new())
                                .with_recent(recovery_recent)
                                .input(recovery_input)
                                .build(max_ctx as usize)
                            {
                                rc.set_state(zhongshu_core::agent::run::RunState::Resuming);
                                let _run_id = rc.begin_resume();
                                let replan_svc = RunAttemptServices {
                                    event_bus: eb.clone(),
                                    response_tx: tx.clone(),
                                    core_db_path: core_db_path.clone(),
                                    run_controller: rc.clone(),
                                };
                                match run_attempt(
                                    RunAttemptRequest {
                                        run_id,
                                        input: input.clone(),
                                        context_pack: recovery_pack,
                                        provider: recovery_provider,
                                        model: recovery_model,
                                        tools: recovery_tools,
                                        budget: recovery_budget,
                                        reasoning_effort: recovery_reasoning,
                                        message_id: aid,
                                        profile: ExecutionProfile::Resumable,
                                    },
                                    replan_svc,
                                )
                                .await
                                {
                                    Ok(attempt_result) => {
                                        let rr = &attempt_result.loop_result;
                                        let conversation_id =
                                            proxy.lock().await.current_conv_id().await;
                                        persist_trace_runbook(
                                            core_db_path.clone(),
                                            &input,
                                            &rr.trace_events,
                                            conversation_id,
                                        );
                                        publish_harness_events(&eb, &rr.trace_events);
                                        let last = rr
                                            .messages
                                            .last()
                                            .map(|x| x.content.as_str())
                                            .unwrap_or("");
                                        history_arc
                                            .lock()
                                            .unwrap()
                                            .push(("user".to_string(), input.clone()));
                                        if !last.is_empty() {
                                            let tools_used = &attempt_result.tool_names;
                                            let history_content = if tools_used.is_empty() {
                                                last.to_string()
                                            } else {
                                                let mut deduped: Vec<(&str, u32)> = Vec::new();
                                                for name in tools_used.iter().map(|s| s.as_str()) {
                                                    if let Some(last) = deduped.last_mut() {
                                                        if last.0 == name {
                                                            last.1 += 1;
                                                            continue;
                                                        }
                                                    }
                                                    deduped.push((name, 1));
                                                }
                                                let badge = deduped
                                                    .iter()
                                                    .map(|(n, c)| {
                                                        if *c > 1 {
                                                            format!("✓ {n} ×{c}")
                                                        } else {
                                                            format!("✓ {n}")
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(" · ");
                                                format!("[工具: {badge}]\n\n{last}")
                                            };
                                            history_arc
                                                .lock()
                                                .unwrap()
                                                .push(("assistant".to_string(), history_content));
                                        }
                                        memory.extract_todos(last);
                                        if rr.outcome == RunOutcome::CompletedVerified {
                                            memory.extract_goal_completions(last);
                                            memory.archive_completed_goals(KEEP_COMPLETED_GOALS);
                                        }
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        stop_reason = format!("{:?}", rr.stop_reason);
                                        overall_success =
                                            rr.outcome == RunOutcome::CompletedVerified;
                                        let outcome_state = match rr.outcome {
                                            RunOutcome::CompletedVerified => {
                                                AgentState::Done { success: true }
                                            }
                                            RunOutcome::CompletedUnverified => {
                                                AgentState::Submitted
                                            }
                                            _ => AgentState::Done { success: false },
                                        };
                                        *state_arc.write().await = outcome_state;
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: outcome_state,
                                        }));
                                    }
                                    Err(RunAttemptError::AgentError(e)) => {
                                        tracing::error!("replan agent run failed: {e:#}");
                                        stop_reason = "replan_failed".to_string();
                                        overall_success = false;
                                        let _ = tx
                                            .send(ResponseEvent::MessageDelta {
                                                id: aid,
                                                delta: format!("{e:#}"),
                                                run_id,
                                            })
                                            .await;
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        *state_arc.write().await =
                                            AgentState::Done { success: false };
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: AgentState::Done { success: false },
                                        }));
                                    }
                                    Err(RunAttemptError::Timeout) => {
                                        tracing::warn!("replan agent task timed out");
                                        stop_reason = "replan_timeout".to_string();
                                        overall_success = false;
                                        let _ = tx
                                            .send(ResponseEvent::MessageDelta {
                                                id: aid,
                                                delta: "[重新规划超时]".into(),
                                                run_id,
                                            })
                                            .await;
                                        let _ = tx
                                            .send(ResponseEvent::MessageCompleted {
                                                id: aid,
                                                run_id,
                                            })
                                            .await;
                                        *state_arc.write().await =
                                            AgentState::Done { success: false };
                                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                                            from: AgentState::Thinking,
                                            to: AgentState::Done { success: false },
                                        }));
                                    }
                                }
                            }
                        } else {
                            // No context to recover — fall back to a clean stop
                            tracing::info!("no recovery context for CancelAndReplan — stopping");
                            stop_reason = "cancelled".to_string();
                            overall_success = false;
                            let _ = tx
                                .send(ResponseEvent::MessageDelta {
                                    id: aid,
                                    delta: format!("[已停止: {reason}]"),
                                    run_id,
                                })
                                .await;
                            let _ = tx
                                .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                                .await;
                            *state_arc.write().await = AgentState::Done { success: false };
                            eb.publish(Event::Agent(AgentEvent::StateChanged {
                                from: AgentState::Thinking,
                                to: AgentState::Done { success: false },
                            }));
                        }
                    }
                    Some(zhongshu_core::agent::run::InterruptionAction::PauseAndRespond {
                        summary,
                    }) => {
                        tracing::info!("interruption paused: {summary}");
                        stop_reason = "paused".to_string();
                        overall_success = false;
                        let _ = tx
                            .send(ResponseEvent::MessageDelta {
                                id: aid,
                                delta: format!("[已暂停: {summary}]"),
                                run_id,
                            })
                            .await;
                        let _ = tx
                            .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                            .await;
                        *state_arc.write().await = AgentState::Done { success: false };
                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                            from: AgentState::Thinking,
                            to: AgentState::Done { success: false },
                        }));
                    }
                    Some(zhongshu_core::agent::run::InterruptionAction::RequireConfirmation {
                        question,
                    }) => {
                        tracing::info!("interruption requires confirmation: {question}");
                        stop_reason = "awaiting_confirmation".to_string();
                        overall_success = false;
                        let _ = tx
                            .send(ResponseEvent::MessageDelta {
                                id: aid,
                                delta: format!("[需要确认: {question}]"),
                                run_id,
                            })
                            .await;
                        let _ = tx
                            .send(ResponseEvent::MessageCompleted { id: aid, run_id })
                            .await;
                        *state_arc.write().await = AgentState::Done { success: false };
                        eb.publish(Event::Agent(AgentEvent::StateChanged {
                            from: AgentState::Thinking,
                            to: AgentState::Done { success: false },
                        }));
                    }
                    None => {
                        tracing::warn!(
                            "interrupted but no action stored — assuming CancelAndReplan"
                        );
                        stop_reason = "cancelled".to_string();
                        overall_success = false;
                    }
                }
            }

            // Notify run controller of completion (handles state cleanup and events)
            let run_outcome_label = if overall_success {
                "CompletedVerified"
            } else if stop_reason.contains("interrupted") || stop_reason == "stopped" {
                "Interrupted"
            } else if stop_reason.contains("timeout") {
                "BudgetExhausted"
            } else if stop_reason.contains("error") || stop_reason.contains("failed") {
                "Failed"
            } else {
                "CompletedUnverified"
            };
            rc.finish_run(&stop_reason, Some(run_outcome_label)).await;

            // Derive terminal UI state from canonical RunStatus
            let terminal_state = rc.agent_state();
            *state_arc.write().await = terminal_state;

            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            *state_arc.write().await = AgentState::Idle;
            eb.publish(Event::Agent(AgentEvent::StateChanged {
                from: terminal_state,
                to: AgentState::Idle,
            }));
        });
        let ct = self.current_task.clone();
        tokio::spawn(async move {
            *ct.lock().await = Some(handle);
        });
    }
}

// ── Shared attempt types ────────────────────────────────────────────

/// Everything that varies per-attempt.
pub struct RunAttemptRequest {
    pub run_id: Uuid,
    pub input: String,
    pub context_pack: ContextPack,
    pub provider: Arc<dyn LlmProvider>,
    pub model: String,
    pub tools: ToolRegistry,
    pub budget: AgentBudget,
    pub reasoning_effort: Option<String>,
    pub message_id: MessageId,
    pub profile: ExecutionProfile,
}

/// Stable infrastructure an attempt needs to run.
pub struct RunAttemptServices {
    pub event_bus: Arc<EventBus>,
    pub response_tx: ResponseTx,
    pub core_db_path: PathBuf,
    pub run_controller: Arc<RunController>,
}

/// Normal return from a successful attempt.
pub struct RunAttemptResult {
    pub loop_result: LoopResult,
    pub tool_names: Vec<String>,
}

#[derive(Debug)]
pub enum RunAttemptError {
    AgentError(anyhow::Error),
    Timeout,
}

/// Execute a single agent attempt — the shared execution kernel
/// for normal runs, recovery runs, and replan runs.
///
/// Owns:
///   1. AgentRuntime construction (checkpoint, ledger, event_bus, idempotency)
///   2. Callback construction (streaming text, tool lifecycle)
///   3. run_agent_with_context() with timeout
///   4. Normalized result extraction
pub(crate) async fn run_attempt(
    req: RunAttemptRequest,
    svc: RunAttemptServices,
) -> Result<RunAttemptResult, RunAttemptError> {
    let tool_names = Arc::new(Mutex::new(Vec::<String>::new()));

    let mut runtime = AgentRuntime::with_llm(req.provider, req.model, req.tools, req.budget);
    runtime.reasoning_effort = req.reasoning_effort;
    runtime.profile = req.profile;

    // Checkpoint store: saves state before each tool call for crash recovery.
    // Only enabled for profiles that need cross-crash recovery.
    if req.profile.saves_checkpoint() {
        runtime.checkpoint_store = Some(CheckpointStore::new(Database::new(
            svc.core_db_path.clone(),
        )));
    }

    // Ledger: needed for reconciling in-flight tools.
    runtime.ledger = svc.run_controller.get_ledger();

    // Event bus: publish events to the UI layer.
    runtime.event_bus = Some((*svc.event_bus).clone());

    // Idempotency checker: skip already-completed tool calls.
    {
        let rc = svc.run_controller.clone();
        runtime.idempotency_checker = Some(Arc::new(move |name: &str, args: &str| {
            rc.is_tool_completed(name, args)
        }));
    }

    // Build callbacks
    let tn = tool_names.clone();
    let callbacks = AgentCallbacks {
        on_text: {
            let tx = svc.response_tx.clone();
            Box::new(move |x: &str| {
                if !x.is_empty() {
                    let _ = tx.try_send(ResponseEvent::MessageDelta {
                        id: req.message_id,
                        delta: x.to_string(),
                        run_id: req.run_id,
                    });
                }
            })
        },
        on_tool_start: {
            let run_id = req.run_id.to_string();
            let eb = svc.event_bus.clone();
            let rc = svc.run_controller.clone();
            Box::new(move |name: &str, args: &str| {
                tn.lock().unwrap().push(name.to_string());
                rc.record_tool_call_start(name, args);
                eb.publish(Event::Tool(ToolEvent::Started {
                    name: name.to_string(),
                    run_id: run_id.clone(),
                }));
            })
        },
        on_tool_done: {
            let run_id = req.run_id.to_string();
            let eb = svc.event_bus.clone();
            let rc = svc.run_controller.clone();
            Box::new(move |name: &str, args: &str, status| {
                rc.record_tool_call_end(name, args, status.as_ledger_status(), None);
                if status == ToolCompletionStatus::UnknownEffect {
                    eb.publish(Event::Tool(ToolEvent::Interrupted {
                        name: name.to_string(),
                        run_id: run_id.clone(),
                        tool_call_id: String::new(),
                    }));
                } else {
                    eb.publish(Event::Tool(ToolEvent::Completed {
                        name: name.to_string(),
                        success: status.is_success(),
                        run_id: run_id.clone(),
                    }));
                }
            })
        },
        run_id: req.run_id,
    };

    let cancel_token = svc.run_controller.cancel_token();
    let r = tokio::time::timeout(
        AGENT_TIMEOUT,
        execute_agent_loop(
            &mut runtime,
            req.context_pack,
            Some(Arc::new(callbacks)),
            &req.input,
            cancel_token,
            req.profile,
        ),
    )
    .await;

    let tools = tool_names.lock().unwrap().clone();
    match r {
        Ok(Ok(loop_result)) => Ok(RunAttemptResult {
            loop_result,
            tool_names: tools,
        }),
        Ok(Err(e)) => Err(RunAttemptError::AgentError(e)),
        Err(_) => Err(RunAttemptError::Timeout),
    }
}

// ── Agent inbox ─────────────────────────────────────────────────────

pub struct AgentInbox {
    controller: Arc<AgentController>,
    queue: Arc<Mutex<VecDeque<String>>>,
    listener_spawned: Mutex<bool>,
}

impl AgentInbox {
    pub fn new(controller: Arc<AgentController>) -> Self {
        AgentInbox {
            controller,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            listener_spawned: Mutex::new(false),
        }
    }

    /// Must be called after the tokio runtime is active.
    pub fn start(&self) {
        let mut spawned = self.listener_spawned.lock().unwrap();
        if *spawned {
            return;
        }
        *spawned = true;
        drop(spawned);
        Self::spawn_listener(self.controller.clone(), self.queue.clone());
    }

    pub fn submit(&self, message: String) {
        self.queue.lock().unwrap().push_back(message);
        self.try_flush();
    }

    /// Queue a message without flushing immediately.
    /// The message will be picked up by the inbox listener when the agent
    /// transitions to Idle. Used for supplement input during busy state.
    pub fn push(&self, message: String) {
        self.queue.lock().unwrap().push_back(message);
    }

    fn try_flush(&self) {
        loop {
            let msg = self.queue.lock().unwrap().pop_front();
            if let Some(msg) = msg {
                self.controller.run(msg);
                // controller.run() returns immediately; if agent was
                // Idle it is now Thinking, so subsequent dequeues will
                // hit the busy guard.
            } else {
                break;
            }
        }
    }

    fn spawn_listener(
        controller: Arc<AgentController>,
        queue: Arc<Mutex<VecDeque<String>>>,
    ) -> tokio::task::JoinHandle<()> {
        let mut rx = controller.event_bus().subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Event::Agent(AgentEvent::StateChanged {
                        to: AgentState::Idle,
                        ..
                    })) => {
                        while let Some(msg) = queue.lock().unwrap().pop_front() {
                            controller.run(msg);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("inbox listener lagged: {n}");
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        })
    }
}

/// Append a worker report to ~/.config/zhongshu/check_log.jsonl.
/// Automatically truncates when the file exceeds 10 MB (keeping the last 5000 lines).
fn log_check(report: &zhongshu_core::agent::report::Report) {
    let path = config::config_dir().join("check_log.jsonl");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let line = serde_json::json!({
            "ts": ts,
            "task_id": report.task_id,
            "worker": report.worker,
            "summary": report.summary,
            "findings": report.findings,
            "attention": format!("{:?}", report.attention),
        });
        let _ = writeln!(f, "{line}");
    }
    truncate_jsonl(&path, 10 * 1024 * 1024, 5000);
}

/// Keep a JSONL file under `max_bytes` by keeping only the last `keep_lines` lines.
fn truncate_jsonl(path: &std::path::Path, max_bytes: u64, keep_lines: usize) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return,
    };
    if meta.len() <= max_bytes {
        return;
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= keep_lines {
        return;
    }
    if let Ok(mut f) = std::fs::File::create(path) {
        for line in lines.iter().rev().take(keep_lines).rev() {
            let _ = writeln!(f, "{line}");
        }
    }
}

// ── Task → Worker dispatcher ─────────────────────────────────────────

/// Consumes from a TaskQueue (shared with TaskScheduler) and routes
/// fired tasks to a Worker for LLM analysis, producing a Report.
pub struct TaskWorkerDispatcher;

impl TaskWorkerDispatcher {
    pub fn spawn(
        queue: TaskQueue,
        runtime: Arc<RwLock<AgentRuntime>>,
        profile: AgentProfile,
        eb: Arc<EventBus>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(task) = queue.recv().await {
                tracing::info!(task = %task.id, source = %task.source, "dispatching to worker");
                let runtime_snapshot = { runtime.read().await.clone() };
                match Worker::execute(&runtime_snapshot, &profile, task, None).await {
                    Ok(report) => {
                        log_check(&report);
                        tracing::debug!(worker = %report.worker, attention = ?report.attention, "worker report");
                        eb.publish(Event::Agent(AgentEvent::WorkerReport(report)));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "worker execution failed");
                    }
                }
            }
            tracing::info!("worker dispatcher stopped (queue closed)");
        })
    }
}

fn persist_trace_runbook(
    core_db_path: PathBuf,
    goal: &str,
    events: &[zhongshu_core::harness::trace::event::HarnessEvent],
    conversation_id: Option<i64>,
) {
    let Some(mut runbook) = events_to_runbook(events, goal) else {
        return;
    };
    runbook.conversation_id = conversation_id;

    let handle = tokio::task::spawn_blocking(move || {
        let store = RunbookStore::new(Database::new(core_db_path));
        if let Err(e) = store.migrate().and_then(|_| store.save(&runbook)) {
            tracing::warn!(error = %e, runbook_id = %runbook.id, "failed to persist trace runbook");
        }
    });
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            tracing::warn!("trace runbook persistence task failed: {e}");
        }
    });
}

/// Drop oldest history pairs until estimated tokens <= trigger.
/// Returns number of messages dropped.
pub(crate) fn compress_history(
    history: &mut Vec<(String, String)>,
    base_est: usize,
    trigger: usize,
) -> usize {
    if history.is_empty() || history.len() < 2 {
        return 0;
    }
    let costs: Vec<usize> = history.iter().map(|(_, c)| (c.len() / 4) + 1).collect();
    let total: usize = costs.iter().sum::<usize>() + base_est;
    if total <= trigger {
        return 0;
    }
    let mut running = total;
    let mut to_drop = 0;
    while running > trigger && to_drop + 2 <= history.len() {
        running -= costs[to_drop] + costs[to_drop + 1];
        to_drop += 2;
    }
    if to_drop > 0 {
        history.drain(..to_drop);
    }
    to_drop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_edit_patch_payload_preserves_unified_diff() {
        let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string();
        let payload = file_edit_patch_payload(Some(&diff)).expect("payload");

        assert_eq!(payload.removed_lines, 1);
        assert_eq!(payload.added_lines, 1);
        assert!(payload.unified_diff.contains("+new"));
    }

    #[test]
    fn file_edit_patch_payload_keeps_placeholder_explicit() {
        let diff = "<binary>".to_string();
        let payload = file_edit_patch_payload(Some(&diff)).expect("payload");

        assert_eq!(payload.summary, "<binary>");
        assert!(payload.unified_diff.is_empty());
    }

    #[test]
    fn compress_empty_history() {
        let mut h = vec![];
        assert_eq!(compress_history(&mut h, 100, 80), 0);
        assert!(h.is_empty());
    }

    #[test]
    fn compress_under_threshold_no_op() {
        let mut h = vec![
            ("user".into(), "hi".into()),
            ("assistant".into(), "hello".into()),
        ];
        // base_est=1, trigger=1000: 1 + (2/4+1)+(5/4+1) = 1+1+2 = 4 << 1000
        assert_eq!(compress_history(&mut h, 1, 1000), 0);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn compress_drops_oldest_pair() {
        let mut h = vec![
            ("user".into(), "old msg".into()),
            ("assistant".into(), "old reply".into()),
            ("user".into(), "new msg".into()),
            ("assistant".into(), "new reply".into()),
        ];
        // Costs: old msg=(8/4+1)=3, old reply=(9/4+1)=3, new msg=(7/4+1)=2, new reply=(9/4+1)=3
        // base_est=1 → total = 1+3+3+2+3 = 12
        // trigger=8 → total>trigger, drop first pair: running=12-3-3=6 ≤ 8 → drop 2
        let dropped = compress_history(&mut h, 1, 8);
        assert_eq!(dropped, 2, "should drop the oldest pair");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].1, "new msg");
        assert_eq!(h[1].1, "new reply");
    }

    #[test]
    fn compress_drops_multiple_pairs_when_needed() {
        let mut h = vec![
            ("user".into(), "a".repeat(100)),      // 100/4+1 = 26
            ("assistant".into(), "b".repeat(100)), // 26
            ("user".into(), "c".repeat(100)),      // 26
            ("assistant".into(), "d".repeat(100)), // 26
            ("user".into(), "e".repeat(50)),       // 50/4+1 = 13
            ("assistant".into(), "f".repeat(50)),  // 13
        ];
        // total = base + 26+26+26+26+13+13 = base + 130
        let base = 10;
        let trigger = 70;
        // total = 140, need running ≤ 70
        // drop pair 1: 140-26-26=88 > 70
        // drop pair 2: 88-26-26=36 ≤ 70 → drop 4 messages (2 pairs)
        let dropped = compress_history(&mut h, base, trigger);
        assert_eq!(dropped, 4);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].1, "e".repeat(50));
        assert_eq!(h[1].1, "f".repeat(50));
    }

    /// Smoke test: realistic message sizes (~100–1000 chars) with a
    /// typical system prompt base estimate (800 tokens).
    #[test]
    fn compress_smoke_realistic_sizes() {
        let n_pairs = 50; // 100 messages
        let mut h: Vec<(String, String)> = (0..n_pairs)
            .flat_map(|i| {
                let user = format!(
                    "用户第{}条消息：{}",
                    i,
                    "请帮我分析一下这个数据，看看有什么值得注意的趋势和模式。我们需要重点关注异常值。".repeat(6)
                );
                let assistant = format!(
                    "这是第{}次回复：{}",
                    i,
                    "好的，我来分析这些数据。从整体趋势来看，数据呈现出明显的周期性波动。\
                     具体来说，第1-3周处于上升期，第4周达到峰值后开始回落，\
                     第5-8周处于低位盘整阶段。建议关注以下几个关键指标：\
                     日均活跃用户数、转化率、留存率和平均会话时长。\
                     异常值出现在第4周周三，可能是由于促销活动导致的短期波动。"
                        .repeat(4)
                );
                vec![(user, assistant)]
            })
            .collect();
        // base ~800 tokens for system prompt, trigger ~3000
        let base = 800;
        let trigger = 3000;
        let total_before = h.len();
        let dropped = compress_history(&mut h, base, trigger);
        assert!(dropped > 0, "should drop some messages when over trigger");
        assert!(
            dropped % 2 == 0,
            "should only drop complete user/assistant pairs"
        );
        assert_eq!(h.len() + dropped, total_before, "total messages conserved");
        // Verify the most recent pair is always preserved
        assert_eq!(
            h.last().unwrap().0,
            format!(
                "用户第{}条消息：{}",
                n_pairs - 1,
                "请帮我分析一下这个数据，看看有什么值得注意的趋势和模式。我们需要重点关注异常值。"
                    .repeat(6)
            )
        );
        // Token estimate must be below (or very close to) trigger after compression
        let costs: Vec<usize> = h.iter().map(|(_, c)| (c.len() / 4) + 1).collect();
        let remain_est: usize = costs.iter().sum::<usize>() + base;
        assert!(
            remain_est <= trigger + 50,
            "after compression estimated tokens {remain_est} should be near trigger {trigger}"
        );
    }

    #[test]
    fn compress_odd_history_does_not_drop_last_single() {
        let mut h = vec![
            ("user".into(), "x".repeat(200)),      // 51
            ("assistant".into(), "y".repeat(200)), // 51
            ("user".into(), "z".repeat(50)),       // 13
        ];
        // base=5, total = 5+51+51+13 = 120, trigger=10
        // drop pair 1: 120-51-51=18 > 10
        // remaining = 1 entry (not a pair), stop
        let dropped = compress_history(&mut h, 5, 10);
        assert_eq!(dropped, 2, "drops the complete pair, leaves lone user");
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].1, "z".repeat(50));
    }
}
