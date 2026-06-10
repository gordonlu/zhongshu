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
        MessageId { id: NEXT.fetch_add(1, Ordering::Relaxed), parent: None }
    }

    pub fn with_parent(parent: MessageId) -> Self {
        MessageId { id: Self::new().id, parent: Some(parent.id) }
    }

    #[allow(dead_code)]
    pub fn parent(&self) -> Option<MessageId> {
        self.parent.map(|p| MessageId { id: p, parent: None })
    }
}

// ── Agent state ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Thinking,
    Executing,
    Done { success: bool },
}

// ── Hierarchical Event (broadcast — allowed to drop) ────────────────

#[derive(Debug, Clone)]
pub enum Event {
    Agent(AgentEvent),
    Tool(ToolEvent),
    Task(TaskEvent),
    Memory(MemoryEvent),
    Authority(AuthorityEvent),
}

#[derive(Debug, Clone)]
pub enum MemoryEvent {
    Compacted,
}

#[derive(Debug, Clone)]
pub enum AuthorityEvent {
    ApprovalRequired {
        id: u64,
        tool: String,
        program: String,
        command: String,
    },
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    StateChanged { from: AgentState, to: AgentState },
}

#[derive(Debug, Clone)]
pub enum ToolEvent {
    Started { name: String },
    Completed { name: String, success: bool },
}

#[derive(Debug, Clone)]
pub enum TaskEvent {
    Triggered { name: String },
    Completed { name: String },
}

// ── EventBus ────────────────────────────────────────────────────────

pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        EventBus { tx }
    }

    pub fn publish(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

pub type EventRx = broadcast::Receiver<Event>;

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
        bus.publish(Event::Agent(AgentEvent::StateChanged { from: AgentState::Idle, to: AgentState::Thinking }));
        let ev = rx.try_recv().unwrap();
        match ev {
            Event::Agent(AgentEvent::StateChanged { from, to }) => {
                assert_eq!(from, AgentState::Idle); assert_eq!(to, AgentState::Thinking);
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn event_bus_multiple_subscribers() {
        let bus = EventBus::new(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        bus.publish(Event::Tool(ToolEvent::Started { name: "search".into() }));
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
        let _ = tx.try_send(ResponseEvent::MessageStarted { id, role: ResponseRole::Assistant });
        let _ = tx.try_send(ResponseEvent::MessageDelta { id, delta: "hello".into() });
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
        bus.publish(Event::Agent(AgentEvent::StateChanged { from: AgentState::Idle, to: AgentState::Thinking }));
        let ev = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(ev, Event::Agent(_)));
    }

    #[tokio::test]
    async fn smoke_response_stream_timeout() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(8);
        let id = MessageId::new();
        tx.send(ResponseEvent::MessageDelta { id, delta: "ok".into() }).await.unwrap();
        let msg = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await.unwrap();
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
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await.unwrap();
        }
    }

    #[tokio::test]
    async fn smoke_backpressure_no_deadlock() {
        let (tx, mut rx) = mpsc::channel::<ResponseEvent>(2);
        let id = MessageId::new();
        // Fill to capacity, sender should not panic.
        let _ = tx.try_send(ResponseEvent::MessageDelta { id, delta: "a".into() });
        let _ = tx.try_send(ResponseEvent::MessageDelta { id, delta: "b".into() });
        // Third send should fail (full), not block or panic.
        assert!(tx.try_send(ResponseEvent::MessageDelta { id, delta: "c".into() }).is_err());
        // Drain.
        while rx.try_recv().is_ok() {}
    }
}
