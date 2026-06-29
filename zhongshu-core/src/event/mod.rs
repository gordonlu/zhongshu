use crate::agent::report::Report;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, mpsc};

// ── Message identity ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MessageId {
    id: u64,
    parent: Option<u64>,
}

impl MessageId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        MessageId {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            parent: None,
        }
    }

    pub fn with_parent(parent: MessageId) -> Self {
        MessageId {
            id: Self::new().id,
            parent: Some(parent.id),
        }
    }

    #[allow(dead_code)]
    pub fn parent(&self) -> Option<MessageId> {
        self.parent.map(|p| MessageId {
            id: p,
            parent: None,
        })
    }
}

// ── Agent state ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentState {
    Idle,
    Thinking,
    Executing,
    Done { success: bool },
}

// ── Hierarchical Event (broadcast — allowed to drop) ────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    Agent(AgentEvent),
    Tool(ToolEvent),
    Harness(HarnessUiEvent),
    Task(TaskEvent),
    Memory(MemoryEvent),
    Goal(GoalEvent),
    Suggestion(SuggestionEvent),
    Authority(AuthorityEvent),
    Attention(AttentionEvent),
    Source(SourceEvent),
}

impl Event {
    /// 人类可读的事件类型名，用于 RuleEngine 规则匹配。
    pub fn type_name(&self) -> &'static str {
        match self {
            Event::Agent(e) => match e {
                AgentEvent::StateChanged { .. } => "state_changed",
                AgentEvent::WorkerReport(..) => "worker_report",
            },
            Event::Tool(..) => "tool",
            Event::Harness(e) => match e {
                HarnessUiEvent::Verification { .. } => "verification",
                HarnessUiEvent::RecoveryFeedback { .. } => "recovery",
                HarnessUiEvent::PhaseTransition { .. } => "phase",
                HarnessUiEvent::CodingSessionStarted { .. } => "coding_session_started",
                HarnessUiEvent::CodingPlanCreated { .. } => "coding_plan_created",
                HarnessUiEvent::CodingStepStarted { .. } => "coding_step_started",
                HarnessUiEvent::CodingStepCompleted { .. } => "coding_step_completed",
                HarnessUiEvent::WorkerStarted { .. } => "worker_started",
                HarnessUiEvent::WorkerCompleted { .. } => "worker_completed",
                HarnessUiEvent::WorkerConflict { .. } => "worker_conflict",
                HarnessUiEvent::PatchPreview { .. } => "patch_preview",
                HarnessUiEvent::PatchApplied { .. } => "patch_applied",
                HarnessUiEvent::ContextIncluded { .. } => "context_included",
                HarnessUiEvent::ContextPressure { .. } => "context_pressure",
                HarnessUiEvent::ReplayAvailable { .. } => "replay_available",
            },
            Event::Task(e) => match e {
                TaskEvent::Triggered { .. } => "task_triggered",
                TaskEvent::Completed { .. } => "task_completed",
            },
            Event::Memory(..) => "memory",
            Event::Goal(e) => match e {
                GoalEvent::Created { .. } => "goal_created",
                GoalEvent::Completed { .. } => "goal_completed",
            },
            Event::Suggestion(e) => match e {
                SuggestionEvent::Accepted { .. } => "suggestion_accepted",
                SuggestionEvent::Rejected { .. } => "suggestion_rejected",
            },
            Event::Authority(..) => "authority",
            Event::Attention(e) => match e {
                AttentionEvent::Interrupt { .. } => "attention_interrupt",
                AttentionEvent::Notify { .. } => "attention_notify",
                AttentionEvent::Digest { .. } => "attention_digest",
            },
            Event::Source(e) => match e {
                SourceEvent::Tick { .. } => "tick",
                SourceEvent::DiskUsage { .. } => "disk_usage",
                SourceEvent::BatteryLow { .. } => "battery_low",
            },
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MemoryEvent {
    Compacted,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GoalEvent {
    Created { goal_id: String, title: String },
    Completed { goal_id: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SuggestionEvent {
    Accepted {
        suggestion_id: String,
        content: String,
    },
    Rejected {
        suggestion_id: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AuthorityEvent {
    ApprovalRequired {
        id: u64,
        tool: String,
        program: String,
        command: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AgentEvent {
    StateChanged { from: AgentState, to: AgentState },
    WorkerReport(Report),
}

/// AttentionManager 产出的通知路由事件。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AttentionEvent {
    /// 需要立即打断用户（P0）
    Interrupt { report: Report },
    /// 桌面通知即可（P1）
    Notify { report: Report },
    /// 归入日/周报（P3）
    Digest { report: Report },
}

/// Source 系统产生的事件。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SourceEvent {
    /// 定期心跳事件。
    Tick { name: String },
    /// 磁盘使用率告警。
    DiskUsage { path: String, usage_pct: f64 },
    /// 电池电量低。
    BatteryLow { level: u8 },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ToolEvent {
    Started { name: String },
    Completed { name: String, success: bool },
}

/// UI-facing events from the harness layer (verification, recovery, phase).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum HarnessUiEvent {
    CodingSessionStarted {
        session_id: String,
        trace_id: String,
        intent: String,
        model: String,
        deeplossless_conversation_id: Option<i64>,
        deeplossless_replay_execution_id: Option<String>,
    },
    CodingPlanCreated {
        session_id: String,
        step_count: usize,
        risk: String,
    },
    CodingStepStarted {
        session_id: String,
        step_id: String,
        kind: String,
        title: String,
    },
    CodingStepCompleted {
        session_id: String,
        step_id: String,
        status: String,
    },
    WorkerStarted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        owned_files: Vec<std::path::PathBuf>,
    },
    WorkerCompleted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        success: bool,
        trace_event_count: usize,
    },
    WorkerConflict {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        reason: String,
    },
    PatchPreview {
        session_id: Option<String>,
        path: std::path::PathBuf,
        operation: String,
        diff_summary: String,
        diff: Option<crate::patch::PatchDiffPayload>,
    },
    PatchApplied {
        session_id: Option<String>,
        path: std::path::PathBuf,
        operation: String,
        changed: bool,
    },
    ContextIncluded {
        description: String,
        estimated_tokens: usize,
    },
    ContextPressure {
        pressure_percent: u8,
        dropped_evidence: usize,
        dropped_recent: usize,
    },
    ReplayAvailable {
        conversation_id: Option<i64>,
        replay_execution_id: Option<String>,
    },
    Verification {
        command: String,
        success: bool,
        exit_code: Option<i32>,
        step: u32,
    },
    RecoveryFeedback {
        rule_id: String,
        message: String,
    },
    PhaseTransition {
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TaskEvent {
    Triggered {
        task_id: String,
        title: String,
    },
    Completed {
        task_id: String,
        title: String,
        output: String,
    },
}

// ── EventBus ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        EventBus { tx }
    }

    pub fn publish(&self, event: Event) {
        if self.tx.receiver_count() == 0 {
            tracing::warn!("event published with no receivers");
        }
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

pub type EventRx = broadcast::Receiver<Event>;

// ── Event persistence (append-only JSONL) ───────────────────────────

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const EVENT_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024; // 10 MB
const EVENT_LOG_KEEP_LINES: usize = 10_000;
const EVENT_LOG_CHECK_INTERVAL: u64 = 100;

/// Append-only event log for debugging and replay.
/// Automatically truncates to the last 10k lines when the file exceeds 10 MB.
pub struct EventLogger {
    file: Mutex<std::fs::File>,
    path: PathBuf,
    write_count: AtomicU64,
}

fn truncate_jsonl(path: &Path, max_bytes: u64, keep_lines: usize) {
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

impl EventLogger {
    /// Open or create the JSONL log file at `path`.
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(EventLogger {
            file: Mutex::new(file),
            path,
            write_count: AtomicU64::new(0),
        })
    }

    /// Spawn a background task that writes every EventBus event to the log.
    pub fn spawn(self, eb: &EventBus) -> tokio::task::JoinHandle<()> {
        let mut rx = eb.subscribe();
        let path = self.path.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Ok(line) = serde_json::to_string(&event) {
                            if let Ok(mut f) = self.file.lock() {
                                let _ = writeln!(f, "{line}");
                                let _ = f.flush();
                            }
                            let count = self.write_count.fetch_add(1, Ordering::Relaxed);
                            if count % EVENT_LOG_CHECK_INTERVAL == 0 {
                                truncate_jsonl(&path, EVENT_LOG_MAX_BYTES, EVENT_LOG_KEEP_LINES);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("event logger lagged: {n}");
                    }
                    Err(_) => break,
                }
            }
        })
    }

    /// Replay events from a JSONL log file into the EventBus.
    /// Returns the number of replayed events.
    pub fn replay(path: &Path, eb: &EventBus) -> usize {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("cannot replay event log: {e}");
                return 0;
            }
        };
        let mut count = 0;
        for line in content.lines() {
            if let Ok(event) = serde_json::from_str::<Event>(line) {
                // Skip stale state-affecting events from past sessions.
                if matches!(event, Event::Agent(AgentEvent::StateChanged { .. })) {
                    continue;
                }
                if matches!(event, Event::Source(SourceEvent::Tick { .. })) {
                    continue;
                }
                if matches!(event, Event::Tool(_)) {
                    continue;
                }
                eb.publish(event);
                count += 1;
            }
        }
        tracing::info!(count, "replayed events from log");
        count
    }
}

// ── Response stream (mpsc bounded — backpressure safe) ──────────────

#[derive(Debug, Clone)]
pub enum ResponseRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub enum ResponseEvent {
    MessageStarted { id: MessageId, role: ResponseRole },
    MessageDelta { id: MessageId, delta: String },
    MessageCompleted { id: MessageId },
}

/// Bounded sender for response events.  `try_send` drops when full;
/// the call site should log a warning when this happens.
pub type ResponseTx = mpsc::Sender<ResponseEvent>;
pub type ResponseRx = mpsc::Receiver<ResponseEvent>;

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── sync tests ──────────────────────────────────────────────────

    #[test]
    fn message_id_is_unique() {
        let a = MessageId::new();
        let b = MessageId::new();
        let c = MessageId::new();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn message_id_parent_chain() {
        let root = MessageId::new();
        let child = MessageId::with_parent(root);
        assert_eq!(child.parent().unwrap(), root);
    }

    #[test]
    fn event_bus_publish_subscribe() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));
        let ev = rx.try_recv().unwrap();
        match ev {
            Event::Agent(AgentEvent::StateChanged { from, to }) => {
                assert_eq!(from, AgentState::Idle);
                assert_eq!(to, AgentState::Thinking);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn event_bus_multiple_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.publish(Event::Tool(ToolEvent::Started {
            name: "search".into(),
        }));
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn event_bus_late_subscriber_does_not_get_past_events() {
        let bus = EventBus::new(16);
        bus.publish(Event::Memory(MemoryEvent::Compacted));
        let mut rx = bus.subscribe();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn response_channel_bounded_send_recv() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(16);
        let id = MessageId::new();
        let _ = tx.try_send(ResponseEvent::MessageStarted {
            id,
            role: ResponseRole::Assistant,
        });
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "hello".into(),
        });
        let _ = tx.try_send(ResponseEvent::MessageCompleted { id });
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    // ── smoke tests (async) ─────────────────────────────────────────

    #[tokio::test]
    async fn smoke_event_flow_timeout() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        bus.publish(Event::Agent(AgentEvent::StateChanged {
            from: AgentState::Idle,
            to: AgentState::Thinking,
        }));
        let ev = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(ev, Event::Agent(_)));
    }

    #[tokio::test]
    async fn smoke_response_stream_timeout() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(8);
        let id = MessageId::new();
        tx.send(ResponseEvent::MessageDelta {
            id,
            delta: "ok".into(),
        })
        .await
        .unwrap();
        let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap();
        assert!(matches!(msg, Some(ResponseEvent::MessageDelta { .. })));
    }

    #[tokio::test]
    async fn smoke_event_bus_no_hang() {
        let bus = EventBus::new(4);
        let mut rx = bus.subscribe();
        // Fill the channel to near capacity, verify no hang.
        for i in 0..3 {
            bus.publish(Event::Memory(MemoryEvent::Compacted));
        }
        for _ in 0..3 {
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.recv())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn smoke_backpressure_no_deadlock() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(2);
        let id = MessageId::new();
        // Fill to capacity, sender should not panic.
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "a".into(),
        });
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "b".into(),
        });
        // Third send should fail (full), not block or panic.
        assert!(tx
            .try_send(ResponseEvent::MessageDelta {
                id,
                delta: "c".into()
            })
            .is_err());
        // Drain.
        while rx.try_recv().is_ok() {}
    }
}
