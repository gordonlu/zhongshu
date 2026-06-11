use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::VecDeque;
use std::io::Write;
use std::time::Duration;

const KEEP_COMPLETED_GOALS: usize = 20;
const AGENT_TIMEOUT: Duration = Duration::from_secs(300);
use zhongshu_core::agent::llm::{Message, OpenAiProvider};
use zhongshu_core::agent::{
    AgentBudget, AgentRuntime, AgentCallbacks, AgentProfile, Worker, run_agent,
};
use zhongshu_core::event::{
    AgentState,
    Event, AgentEvent, ToolEvent,
    ResponseEvent, ResponseRole, MessageId,
    EventBus, ResponseTx,
};
use zhongshu_core::tool::ToolRegistry;
use zhongshu_core::task::TaskQueue;
use tokio::sync::RwLock;
use crate::agent::{AgentMemory};
use crate::config;

// ── Session persistence ─────────────────────────────────────────────

#[derive(Clone)]
pub struct SessionState {
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
    provider: OpenAiProvider,
    tools: ToolRegistry,
    model: String,
    session: SessionState,
    system_prompt: Mutex<String>,
    state: Arc<RwLock<AgentState>>,
    memory: AgentMemory,
    current_task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl AgentController {
    pub fn new(
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        provider: OpenAiProvider,
        tools: ToolRegistry,
        model: String,
        session: SessionState,
        system_prompt: String,
        profile_path: PathBuf,
    ) -> Self {
        let memory = AgentMemory::load(&profile_path);
        AgentController {
            event_bus, response_tx, provider, tools, model, session,
            system_prompt: Mutex::new(system_prompt),
            state: Arc::new(RwLock::new(AgentState::Idle)),
            memory,
            current_task: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Shared state for external consumers (UI, background runner).
    pub fn state(&self) -> Arc<RwLock<AgentState>> {
        self.state.clone()
    }

    pub fn set_system_prompt(&self, prompt: String) {
        *self.system_prompt.lock().unwrap() = prompt;
    }

    /// Cancel the currently running agent task.
    pub fn cancel(&self) {
        let ct = self.current_task.clone();
        tokio::spawn(async move {
            if let Some(h) = ct.lock().await.take() {
                h.abort();
                tracing::info!("agent task cancelled by user");
            }
        });
    }

    pub(crate) fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// Run an agent turn for the given input.  Non‑blocking — spawns
    /// the actual work on the tokio runtime and returns immediately.
    /// Does nothing if the agent is already busy.
    pub fn run(&self, input: String) {
        if !self.try_claim() {
            tracing::debug!("agent busy, skipping run");
            return;
        }

        // User approval keywords → approve pending authority requests.
        let trimmed = input.trim().to_lowercase();
        if matches!(trimmed.as_str(), "yes" | "y" | "可以" | "确认" | "同意" | "好" | "是") {
            zhongshu_core::authority::approve_pending();
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
                    *s = AgentState::Thinking;
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    fn emit_start(&self, input: &str) {
        let uid = MessageId::new();
        let _ = self.response_tx.try_send(ResponseEvent::MessageStarted { id: uid, role: ResponseRole::User });
        let _ = self.response_tx.try_send(ResponseEvent::MessageDelta { id: uid, delta: input.to_string() });
        let _ = self.response_tx.try_send(ResponseEvent::MessageCompleted { id: uid });

        self.event_bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));
    }

    fn spawn_task(&self, input: String) {
        let eb = self.event_bus.clone();
        let tx = self.response_tx.clone();
        let p = self.provider.clone();
        let t = self.tools.clone();
        let m = self.model.clone();
        let sys = self.system_prompt.lock().unwrap().clone();
        let memory = self.memory.clone();
        let state_arc = self.state.clone();

        // Snapshot profile for the prompt — non‑blocking read.
        let profile_ctx = memory.prompt_context();

        let handle = tokio::spawn(async move {
            let aid = MessageId::new();
            let _ = tx.send(ResponseEvent::MessageStarted { id: aid, role: ResponseRole::Assistant }).await;

            let mut msgs = vec![Message::system(&sys)];
            if !profile_ctx.is_empty() {
                msgs.push(Message::system(&profile_ctx));
            }
            msgs.push(Message::user(input.clone()));
            let runtime = AgentRuntime::new(p, t, m, AgentBudget::default());

            let callbacks = AgentCallbacks {
                on_text: {
                    let tx = tx.clone();
                    Box::new(move |x: &str| {
                        let delta = super::overlay::strip_final_answer(x).trim().to_string();
                        if !delta.is_empty() {
                            let _ = tx.try_send(ResponseEvent::MessageDelta { id: aid, delta });
                        }
                    })
                },
                on_tool_start: {
                    let eb = eb.clone();
                    Box::new(move |name: &str| {
                        eb.publish(Event::Tool(ToolEvent::Started { name: name.to_string() }));
                    })
                },
                on_tool_done: {
                    let eb = eb.clone();
                    Box::new(move |name: &str, ok: bool| {
                        eb.publish(Event::Tool(ToolEvent::Completed { name: name.to_string(), success: ok }));
                    })
                },
            };

            let r = tokio::time::timeout(AGENT_TIMEOUT, run_agent(&runtime, msgs, Some(Arc::new(callbacks)), &input)).await;

            match r {
                Ok(Ok(rr)) => {
                    let last = rr.messages.last().map(|x| x.content.as_str()).unwrap_or("");
                    // Extract todos and goal completions.
                    memory.extract_todos(last);
                    memory.extract_goal_completions(last);
                    // Archive old completed goals to keep the list bounded.
                    memory.archive_completed_goals(KEEP_COMPLETED_GOALS);
                    let _ = tx.send(ResponseEvent::MessageCompleted { id: aid }).await;
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: true },
                    }));
                }
                Ok(Err(e)) => {
                    let _ = tx.send(ResponseEvent::MessageDelta { id: aid, delta: format!("{e}") }).await;
                    let _ = tx.send(ResponseEvent::MessageCompleted { id: aid }).await;
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: false },
                    }));
                }
                Err(_) => {
                    tracing::warn!("agent task timed out after 300s");
                    let _ = tx.send(ResponseEvent::MessageDelta { id: aid, delta: "[连接超时: 300s 无响应]".into() }).await;
                    let _ = tx.send(ResponseEvent::MessageCompleted { id: aid }).await;
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: false },
                    }));
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            *state_arc.write().await = AgentState::Idle;
            eb.publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Done { success: true },
                to: AgentState::Idle,
            }));
        });
        let ct = self.current_task.clone();
        tokio::spawn(async move {
            *ct.lock().await = Some(handle);
        });
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
        if *spawned { return; }
        *spawned = true;
        drop(spawned);
        Self::spawn_listener(self.controller.clone(), self.queue.clone());
    }

    pub fn submit(&self, message: String) {
        self.queue.lock().unwrap().push_back(message);
        self.try_flush();
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
                    Ok(Event::Agent(AgentEvent::StateChanged { to: AgentState::Idle, .. })) => {
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
fn log_check(report: &zhongshu_core::agent::report::Report) {
    let path = config::config_dir().join("check_log.jsonl");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
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
}

// ── Task → Worker dispatcher ─────────────────────────────────────────

/// Consumes from a TaskQueue (shared with TaskScheduler) and routes
/// fired tasks to a Worker for LLM analysis, producing a Report.
pub struct TaskWorkerDispatcher;

impl TaskWorkerDispatcher {
    pub fn spawn(
        queue: TaskQueue,
        runtime: Arc<AgentRuntime>,
        profile: AgentProfile,
        eb: Arc<EventBus>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(task) = queue.recv().await {
                tracing::info!(task = %task.id, source = %task.source, "dispatching to worker");
                match Worker::execute(&runtime, &profile, task, None).await {
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
