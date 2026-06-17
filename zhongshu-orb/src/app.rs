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
use zhongshu_core::agent::llm::{Message, OpenAiProvider};
use zhongshu_core::agent::{
    run_agent, AgentBudget, AgentCallbacks, AgentProfile, AgentRuntime, ModelRouter, Worker,
};
use zhongshu_core::event::{
    AgentEvent, AgentState, Event, EventBus, MessageId, ResponseEvent, ResponseRole, ResponseTx,
    ToolEvent,
};
use zhongshu_core::integration::DeeplosslessProxy;
use zhongshu_core::task::TaskQueue;
use zhongshu_core::tool::ToolRegistry;

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
    provider: Mutex<OpenAiProvider>,
    tools: ToolRegistry,
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
    max_context_tokens: AtomicU32,
    pub auto_evolve_enabled: AtomicBool,
}

impl AgentController {
    pub fn new(
        event_bus: Arc<EventBus>,
        response_tx: ResponseTx,
        provider: OpenAiProvider,
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
    ) -> Self {
        let memory = AgentMemory::load(&profile_path);
        AgentController {
            event_bus,
            response_tx,
            provider: Mutex::new(provider),
            tools,
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
            max_context_tokens: AtomicU32::new(max_context_tokens),
            auto_evolve_enabled: AtomicBool::new(false),
            equipment,
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
            for (_id, prompt) in &reg.skill_prompts() {
                full.push_str("\n\n");
                full.push_str(prompt);
            }
        }
        *self.system_prompt.lock().unwrap() = full;
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

    pub fn provider_snapshot(&self) -> OpenAiProvider {
        self.provider.lock().unwrap().clone()
    }

    pub fn update_llm_runtime(
        &self,
        provider: OpenAiProvider,
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

    pub fn set_chat_history(&self, history: Vec<(String, String)>) {
        *self.history.lock().unwrap() = history;
    }

    /// Cancel the currently running agent task.
    pub fn cancel(&self) {
        let ct = self.current_task.clone();
        let state = self.state.clone();
        let eb = self.event_bus.clone();
        tokio::spawn(async move {
            if let Some(h) = ct.lock().await.take() {
                h.abort();
                tracing::info!("agent task cancelled by user");
                *state.write().await = AgentState::Idle;
                eb.publish(Event::Agent(AgentEvent::StateChanged {
                    from: AgentState::Thinking,
                    to: AgentState::Done { success: false },
                }));
                eb.publish(Event::Agent(AgentEvent::StateChanged {
                    from: AgentState::Done { success: false },
                    to: AgentState::Idle,
                }));
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
        if matches!(
            trimmed.as_str(),
            "yes" | "y" | "可以" | "确认" | "同意" | "好" | "是"
        ) {
            if let Some(req) = zhongshu_core::authority::peek_pending() {
                zhongshu_core::authority::approve_pending(&req.id);
            }
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
        let _ = self.response_tx.try_send(ResponseEvent::MessageStarted {
            id: uid,
            role: ResponseRole::User,
        });
        let _ = self.response_tx.try_send(ResponseEvent::MessageDelta {
            id: uid,
            delta: input.to_string(),
        });
        let _ = self
            .response_tx
            .try_send(ResponseEvent::MessageCompleted { id: uid });

        self.event_bus
            .publish(Event::Agent(AgentEvent::StateChanged {
                from: AgentState::Idle,
                to: AgentState::Thinking,
            }));
    }

    fn spawn_task(&self, input: String) {
        let eb = self.event_bus.clone();
        let tx = self.response_tx.clone();
        let t = self.tools.clone();
        let sys = self.system_prompt.lock().unwrap().clone();
        let history_arc = self.history.clone();
        let memory = self.memory.clone();
        let state_arc = self.state.clone();

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
            provider_snapshot.with_model(&routed_model)
        } else {
            provider_snapshot
        };
        let m = routed_model;
        let max_ctx = self.max_context_tokens.load(Ordering::Relaxed);
        let proxy = self.proxy.clone();

        // Snapshot profile for the prompt — non‑blocking read.
        let profile_ctx = memory.prompt_context();

        let handle = tokio::spawn(async move {
            let aid = MessageId::new();
            let _ = tx
                .send(ResponseEvent::MessageStarted {
                    id: aid,
                    role: ResponseRole::Assistant,
                })
                .await;

            // Context compression: drop oldest history pairs when over 80%.
            if max_ctx > 0 {
                let trigger = (max_ctx as f64 * 0.8) as usize;
                let base_est = (sys.len() / 4)
                    + 1
                    + (input.len() / 4)
                    + 1
                    + if !profile_ctx.is_empty() {
                        (profile_ctx.len() / 4) + 1
                    } else {
                        0
                    };

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

            let mut msgs = vec![Message::system(&sys)];
            if !profile_ctx.is_empty() {
                msgs.push(Message::system(&profile_ctx));
            }
            {
                let history = history_arc.lock().unwrap();
                for (role, content) in history.iter() {
                    match role.as_str() {
                        "user" => msgs.push(Message::user(content)),
                        "assistant" => msgs.push(Message::assistant(content)),
                        _ => {}
                    }
                }
            }
            msgs.push(Message::user(input.clone()));

            let mut runtime = AgentRuntime::new(p, t, m, AgentBudget::default());
            runtime.reasoning_effort = reasoning_str;

            let tool_names = Arc::new(Mutex::new(Vec::<String>::new()));
            let callbacks = {
                let tn = tool_names.clone();
                let eb1 = eb.clone();
                let eb2 = eb.clone();
                AgentCallbacks {
                    on_text: {
                        let tx = tx.clone();
                        Box::new(move |x: &str| {
                            if !x.is_empty() {
                                tracing::debug!(len = x.len(), "on_text");
                                let _ = tx.try_send(ResponseEvent::MessageDelta {
                                    id: aid,
                                    delta: x.to_string(),
                                });
                            } else {
                                tracing::debug!("on_text empty");
                            }
                        })
                    },
                    on_tool_start: Box::new(move |name: &str| {
                        tn.lock().unwrap().push(name.to_string());
                        eb1.publish(Event::Tool(ToolEvent::Started {
                            name: name.to_string(),
                        }));
                    }),
                    on_tool_done: Box::new(move |name: &str, ok: bool| {
                        eb2.publish(Event::Tool(ToolEvent::Completed {
                            name: name.to_string(),
                            success: ok,
                        }));
                    }),
                }
            };

            let r = tokio::time::timeout(
                AGENT_TIMEOUT,
                run_agent(&runtime, msgs, Some(Arc::new(callbacks)), &input),
            )
            .await;

            match r {
                Ok(Ok(rr)) => {
                    let last = rr.messages.last().map(|x| x.content.as_str()).unwrap_or("");
                    // Append to conversation history for next turn.
                    history_arc
                        .lock()
                        .unwrap()
                        .push(("user".to_string(), input.clone()));
                    if !last.is_empty() {
                        let tools_used = tool_names.lock().unwrap();
                        let history_content = if tools_used.is_empty() {
                            last.to_string()
                        } else {
                            let badge = tools_used
                                .iter()
                                .map(|n| format!("✓ {n}"))
                                .collect::<Vec<_>>()
                                .join(" · ");
                            format!("[工具: {badge}]\n\n{last}")
                        };
                        history_arc
                            .lock()
                            .unwrap()
                            .push(("assistant".to_string(), history_content));
                    }
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
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: format!("{e}"),
                        })
                        .await;
                    let _ = tx.send(ResponseEvent::MessageCompleted { id: aid }).await;
                    eb.publish(Event::Agent(AgentEvent::StateChanged {
                        from: AgentState::Thinking,
                        to: AgentState::Done { success: false },
                    }));
                }
                Err(_) => {
                    tracing::warn!("agent task timed out after 300s");
                    let _ = tx
                        .send(ResponseEvent::MessageDelta {
                            id: aid,
                            delta: "[连接超时: 300s 无响应]".into(),
                        })
                        .await;
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

/// Drop oldest history pairs until estimated tokens ≤ trigger.
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
