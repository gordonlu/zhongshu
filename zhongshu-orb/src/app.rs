use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::VecDeque;
use std::time::Duration;
use zhongshu_core::agent::llm::{Message, OpenAiProvider};
use zhongshu_core::agent::loop_::{AgentBudget, AgentLoop};
use zhongshu_core::event::{
    AgentState,
    Event, AgentEvent, ToolEvent,
    ResponseEvent, ResponseRole, MessageId,
    EventBus, ResponseTx,
};
use zhongshu_core::integration::{ContextConfig, ContextEngine};
use zhongshu_core::tool::ToolRegistry;
use zhongshu_core::task::TaskQueue;
use tokio::sync::RwLock;
use crate::agent::{AgentMemory};

// ── Session persistence ─────────────────────────────────────────────

#[derive(Clone)]
pub struct SessionState {
    pub engine: Arc<tokio::sync::Mutex<Option<Arc<ContextEngine>>>>,
    pub conv_id: Arc<tokio::sync::Mutex<i64>>,
}

impl SessionState {
    pub fn new() -> Self {
        SessionState {
            engine: Arc::new(tokio::sync::Mutex::new(None)),
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
    system_prompt: String,
    state: Arc<RwLock<AgentState>>,
    memory: AgentMemory,
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
            event_bus, response_tx, provider, tools, model, session, system_prompt,
            state: Arc::new(RwLock::new(AgentState::Idle)),
            memory,
        }
    }

    /// Shared state for external consumers (UI, background runner).
    pub fn state(&self) -> Arc<RwLock<AgentState>> {
        self.state.clone()
    }

    pub(crate) fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// Initialise the deeplossless context engine asynchronously.
    pub fn init_engine(&self, api_key: &str) {
        if api_key.is_empty() { return; }
        let ak = api_key.to_string();
        let m = self.model.clone();
        let ea = self.session.engine.clone();
        let ca = self.session.conv_id.clone();
        let sys = self.system_prompt.clone();
        tokio::spawn(async move {
            if let Ok(e) = ContextEngine::new(ContextConfig {
                api_key: ak,
                ..ContextConfig::default()
            }).await {
                let cid = e.find_or_create_conv(&sys, &m).unwrap_or(1);
                *ea.lock().await = Some(Arc::new(e));
                *ca.lock().await = cid;
            }
        });
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
        let e = self.session.engine.clone();
        let c = self.session.conv_id.clone();
        let sys = self.system_prompt.clone();
        let memory = self.memory.clone();
        let state_arc = self.state.clone();

        // Snapshot profile for the prompt — non‑blocking read.
        let profile_ctx = memory.prompt_context();

        tokio::spawn(async move {
            let aid = MessageId::new();
            let _ = tx.send(ResponseEvent::MessageStarted { id: aid, role: ResponseRole::Assistant }).await;

            let eng = e.lock().await.clone();
            let cid = *c.lock().await;
            let mctx = eng.as_ref().map_or(String::new(), |x| x.build_context(cid, 5000, &input).unwrap_or_default());
            let mut msgs = vec![Message::system(&sys)];
            if !profile_ctx.is_empty() {
                msgs.push(Message::system(&profile_ctx));
            }
            if !mctx.is_empty() { msgs.push(Message::user(format!("<context>\n{mctx}\n</context>"))); }
            msgs.push(Message::user(input.clone()));
            let agent = AgentLoop::new(p, t, m).with_budget(AgentBudget::default()).with_messages(msgs);

            let r = agent.run_streaming("",
                {
                    let tx = tx.clone();
                    move |x: &str| {
                        let delta = x.replace("<final_answer>", "").replace("</final_answer>", "");
                        if !delta.is_empty() {
                            let _ = tx.try_send(ResponseEvent::MessageDelta { id: aid, delta });
                        }
                    }
                },
                {
                    let eb = eb.clone();
                    move |name: &str| {
                        eb.publish(Event::Tool(ToolEvent::Started { name: name.to_string() }));
                    }
                },
                {
                    let eb = eb.clone();
                    move |name: &str, ok: bool| {
                        eb.publish(Event::Tool(ToolEvent::Completed { name: name.to_string(), success: ok }));
                    }
                },
            ).await;

            match r {
                Ok(rr) => {
                    let last = rr.messages.last().map(|x| x.content.as_str()).unwrap_or("");
                    if let Some(ref en) = eng {
                        let _ = en.append_turn(cid, &format!("[u]:{}", input), &format!("[a]:{last}"));
                        if en.check_compression(cid).should_compress { let _ = en.trigger_compaction(cid).await; }
                    }
                    // Extract todos and goal completions (independent of engine state).
                    memory.extract_todos(last);
                    memory.extract_goal_completions(last);
                    let _ = tx.send(ResponseEvent::MessageCompleted { id: aid }).await;
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: true },
                    }));
                }
                Err(e) => {
                    let _ = tx.send(ResponseEvent::MessageDelta { id: aid, delta: format!("{e}") }).await;
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
    }
}

// ── Background runner ──────────────────────────────────────────────

pub struct BackgroundRunner {
    interval: Duration,
    prompt: String,
    state: Arc<RwLock<AgentState>>,
}

impl BackgroundRunner {
    pub fn new(interval_secs: u64, prompt: String, state: Arc<RwLock<AgentState>>) -> Self {
        BackgroundRunner {
            interval: Duration::from_secs(interval_secs),
            prompt,
            state,
        }
    }

    pub fn spawn(self, inbox: Arc<AgentInbox>) -> tokio::task::JoinHandle<()> {
        assert!(self.interval > Duration::ZERO, "background interval must be positive");
        tokio::spawn(async move {
            tracing::info!("background runner started (interval {:?})", self.interval);
            let mut tick = tokio::time::interval(self.interval);
            tick.tick().await;
            loop {
                tick.tick().await;
                if matches!(*self.state.read().await, AgentState::Idle) {
                    tracing::debug!("background runner firing");
                    inbox.submit(self.prompt.clone());
                } else {
                    tracing::debug!("background runner skipped (agent busy)");
                }
            }
        })
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

// ── Task → Agent dispatcher ────────────────────────────────────────

/// Consumes from a TaskQueue (shared with TaskScheduler) and routes
/// fired tasks to the AgentInbox as user-visible messages.
pub struct AgentTaskDispatcher;

impl AgentTaskDispatcher {
    pub fn spawn(queue: TaskQueue, inbox: Arc<AgentInbox>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(task) = queue.recv().await {
                let msg = format!("[定时任务: {}] {}", task.source, task.arguments);
                tracing::debug!("dispatching task {} to inbox", task.id);
                inbox.submit(msg);
            }
            tracing::info!("task dispatcher stopped (queue closed)");
        })
    }
}
