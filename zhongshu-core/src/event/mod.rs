use crate::agent::report::Report;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

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
    Submitted,
    Done { success: bool },
}

// ── Hierarchical Event (broadcast — allowed to drop) ────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    Agent(AgentEvent),
    Tool(ToolEvent),
    Harness(HarnessUiEvent),
    Organization(OrganizationEvent),
    Task(TaskEvent),
    Memory(MemoryEvent),
    Goal(GoalEvent),
    Run(RunEvent),
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
            Event::Organization(e) => match e {
                OrganizationEvent::RoutingDecided { .. } => "organization_routing_decided",
                OrganizationEvent::TaskStarted { .. } => "organization_task_started",
                OrganizationEvent::EmployeeAssigned { .. } => "organization_employee_assigned",
                OrganizationEvent::EmployeeWorking { .. } => "organization_employee_working",
                OrganizationEvent::EmployeeReported { .. } => "organization_employee_reported",
                OrganizationEvent::Handoff { .. } => "organization_handoff",
                OrganizationEvent::ManagerReviewing { .. } => "organization_manager_reviewing",
                OrganizationEvent::TaskFinished { .. } => "organization_task_finished",
            },
            Event::Task(e) => match e {
                TaskEvent::Triggered { .. } => "task_triggered",
                TaskEvent::Claimed { .. } => "task_claimed",
                TaskEvent::ClaimFailed { .. } => "task_claim_failed",
                TaskEvent::Completed { .. } => "task_completed",
                TaskEvent::Failed { .. } => "task_failed",
                TaskEvent::Cancelled { .. } => "task_cancelled",
                TaskEvent::RetryScheduled { .. } => "task_retry_scheduled",
                TaskEvent::RetriesExhausted { .. } => "task_retries_exhausted",
            },
            Event::Memory(..) => "memory",
            Event::Goal(e) => match e {
                GoalEvent::Created { .. } => "goal_created",
                GoalEvent::Completed { .. } => "goal_completed",
            },
            Event::Run(e) => match e {
                RunEvent::Started { .. } => "run_started",
                RunEvent::Interrupted { .. } => "run_interrupted",
                RunEvent::Resuming { .. } => "run_resuming",
                RunEvent::Paused { .. } => "run_paused",
                RunEvent::Finished { .. } => "run_finished",
                RunEvent::Cancelled { .. } => "run_cancelled",
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

/// Observable organization lifecycle emitted by real orchestration work.
///
/// These events describe completed state transitions; presentation layers may
/// animate between them but must not synthesize additional progress states.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrganizationEvent {
    /// Observable result of the optional automatic routing gate. This is a
    /// proposal/admission outcome, not evidence that worker execution began.
    RoutingDecided {
        routing_id: String,
        strategy: String,
        reason: String,
        worker_count: usize,
    },
    TaskStarted {
        task_id: String,
        manager: String,
        collaboration: String,
    },
    EmployeeAssigned {
        task_id: String,
        employee: String,
        role: String,
        responsibility: String,
        reports_to: String,
    },
    EmployeeWorking {
        task_id: String,
        employee: String,
        role: String,
    },
    EmployeeReported {
        task_id: String,
        employee: String,
        role: String,
        outcome: String,
        success: bool,
    },
    Handoff {
        task_id: String,
        from_employee: String,
        to_employee: String,
    },
    ManagerReviewing {
        task_id: String,
        manager: String,
    },
    TaskFinished {
        task_id: String,
        status: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum MemoryEvent {
    Compacted,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TaskEvent {
    Triggered {
        task_id: String,
        title: String,
    },
    Claimed {
        task_id: String,
        worker_id: String,
    },
    ClaimFailed {
        task_id: String,
        reason: String,
    },
    Completed {
        task_id: String,
        title: String,
        output: String,
    },
    Failed {
        task_id: String,
        title: String,
        error: String,
    },
    Cancelled {
        task_id: String,
        title: String,
        reason: String,
    },
    RetryScheduled {
        task_id: String,
        retry_count: i32,
        max_retries: i32,
    },
    RetriesExhausted {
        task_id: String,
        retry_count: i32,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GoalEvent {
    Created { goal_id: String, title: String },
    Completed { goal_id: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RunEvent {
    Started { run_id: String, goal: String },
    Interrupted { run_id: String, reason: String },
    Resuming { run_id: String },
    Paused { run_id: String },
    Finished { run_id: String, stop_reason: String },
    Cancelled { run_id: String },
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
    Started {
        name: String,
        run_id: String,
    },
    Completed {
        name: String,
        success: bool,
        run_id: String,
    },
    Interrupted {
        name: String,
        run_id: String,
        tool_call_id: String,
    },
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
        status: String,
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

// ── SnapshotStore (ring buffer with sequence numbers) ───────────────

const SNAPSHOT_CAPACITY: usize = 500;

/// An in-memory ring buffer that assigns a monotonic sequence number to every
/// published event.  Lagged subscribers (or an overlay that was closed) can
/// call `recent_since(cursor)` to recover missed events without relying on the
/// broadcast channel's capacity.
#[derive(Clone)]
pub struct SnapshotStore {
    recent: Arc<Mutex<VecDeque<(u64, Event)>>>,
    seq: Arc<AtomicU64>,
    capacity: usize,
}

impl SnapshotStore {
    pub fn new(capacity: usize) -> Self {
        SnapshotStore {
            recent: Arc::new(Mutex::new(VecDeque::with_capacity(capacity + 1))),
            seq: Arc::new(AtomicU64::new(0)),
            capacity,
        }
    }

    /// Record an event and return its sequence number.
    pub fn push(&self, event: Event) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut guard = self.recent.lock().unwrap();
        guard.push_back((seq, event));
        if guard.len() > self.capacity {
            guard.pop_front();
        }
        seq
    }

    /// Current cursor (next sequence number that will be assigned).
    pub fn current_cursor(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Return all buffered events whose sequence number >= `cursor`.
    /// Returns an empty vec when `cursor >= current_cursor()`.
    pub fn recent_since(&self, cursor: u64) -> Vec<(u64, Event)> {
        let guard = self.recent.lock().unwrap();
        if cursor >= self.seq.load(Ordering::Relaxed) {
            return Vec::new();
        }
        // Binary search because VecDeque is not slice-friendly; linear scan
        // over a small buffer is fine (<= 500 entries).
        let mut out = Vec::with_capacity(guard.len());
        for entry in guard.iter() {
            if entry.0 >= cursor {
                out.push(entry.clone());
            }
        }
        out
    }

    /// Resolve the sequence number for a broadcast event starting at a
    /// subscriber cursor. Broadcast carries the event payload for backward
    /// compatibility; this lookup lets cursor-aware consumers acknowledge the
    /// exact buffered occurrence and suppress replay duplicates.
    pub fn sequence_for_event_since(&self, cursor: u64, event: &Event) -> Option<u64> {
        let encoded = serde_json::to_vec(event).ok()?;
        self.recent
            .lock()
            .unwrap()
            .iter()
            .find(|(sequence, candidate)| {
                *sequence >= cursor
                    && serde_json::to_vec(candidate)
                        .map(|candidate| candidate == encoded)
                        .unwrap_or(false)
            })
            .map(|(sequence, _)| *sequence)
    }
}

// ── EventBus ────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
    snapshot: SnapshotStore,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        EventBus {
            tx,
            snapshot: SnapshotStore::new(SNAPSHOT_CAPACITY),
        }
    }

    pub fn publish(&self, event: Event) {
        self.snapshot.push(event.clone());
        if self.tx.receiver_count() == 0 {
            tracing::warn!("event published with no receivers");
        }
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn snapshot_store(&self) -> &SnapshotStore {
        &self.snapshot
    }

    /// Convenience: return events since `cursor` for UI catch-up.
    pub fn recent_since_cursor(&self, cursor: u64) -> Vec<(u64, Event)> {
        self.snapshot.recent_since(cursor)
    }

    /// Current cursor value (next sequence number).
    pub fn current_cursor(&self) -> u64 {
        self.snapshot.current_cursor()
    }

    pub fn sequence_for_event_since(&self, cursor: u64, event: &Event) -> Option<u64> {
        self.snapshot.sequence_for_event_since(cursor, event)
    }
}

pub type EventRx = broadcast::Receiver<Event>;

// ── Event persistence (append-only JSONL) ───────────────────────────

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    MessageStarted {
        id: MessageId,
        role: ResponseRole,
        run_id: Uuid,
    },
    MessageDelta {
        id: MessageId,
        delta: String,
        run_id: Uuid,
    },
    MessageCompleted {
        id: MessageId,
        run_id: Uuid,
    },
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
            run_id: "test".into(),
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
            run_id: Uuid::default(),
        });
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "hello".into(),
            run_id: Uuid::default(),
        });
        let _ = tx.try_send(ResponseEvent::MessageCompleted {
            id,
            run_id: Uuid::default(),
        });
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
            run_id: Uuid::default(),
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
        for _ in 0..3 {
            bus.publish(Event::Memory(MemoryEvent::Compacted));
        }
        for _ in 0..3 {
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.recv())
                .await
                .unwrap();
        }
    }

    #[test]
    fn event_cursor_distinguishes_identical_occurrences() {
        let store = SnapshotStore::new(8);
        let event = Event::Memory(MemoryEvent::Compacted);
        let first = store.push(event.clone());
        let second = store.push(event.clone());

        assert_eq!(store.sequence_for_event_since(first, &event), Some(first));
        assert_eq!(
            store.sequence_for_event_since(first + 1, &event),
            Some(second)
        );
        assert_eq!(store.sequence_for_event_since(second + 1, &event), None);
    }

    #[tokio::test]
    async fn smoke_backpressure_no_deadlock() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(2);
        let id = MessageId::new();
        // Fill to capacity, sender should not panic.
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "a".into(),
            run_id: Uuid::default(),
        });
        let _ = tx.try_send(ResponseEvent::MessageDelta {
            id,
            delta: "b".into(),
            run_id: Uuid::default(),
        });
        // Third send should fail (full), not block or panic.
        assert!(tx
            .try_send(ResponseEvent::MessageDelta {
                id,
                delta: "c".into(),
                run_id: Uuid::default(),
            })
            .is_err());
        // Drain.
        while rx.try_recv().is_ok() {}
    }
}
