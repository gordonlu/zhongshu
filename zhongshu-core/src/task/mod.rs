use crate::tool::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub source: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone)]
pub struct TaskQueue {
    tx: mpsc::UnboundedSender<Task>,
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Task>>>,
}

impl TaskQueue {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        TaskQueue {
            tx,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
        }
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<Task> {
        self.tx.clone()
    }

    pub fn submit(&self, task: Task) {
        let _ = self.tx.send(task);
    }

    /// Block until a task is available, then return it.
    /// Returns `None` if the channel is closed and empty.
    pub async fn recv(&self) -> Option<Task> {
        loop {
            let task = {
                let mut guard = self.rx.lock().await;
                guard.try_recv().ok()
            };
            if let Some(task) = task {
                return Some(task);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

pub struct TaskWorker {
    queue: TaskQueue,
    registry: Arc<ToolRegistry>,
}

impl TaskWorker {
    pub fn new(queue: TaskQueue, registry: Arc<ToolRegistry>) -> Self {
        TaskWorker { queue, registry }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let rx = self.queue.rx.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            let mut rx = rx.lock().await;
            while let Some(task) = rx.recv().await {
                info!(task = %task.id, tool = %task.tool, "worker executing task");
                let output = registry
                    .execute(
                        &task.tool,
                        &serde_json::to_string(&task.arguments).unwrap_or_default(),
                    )
                    .await;
                match output.status {
                    crate::tool::ToolStatus::Success => {
                        debug!(task = %task.id, "task succeeded");
                    }
                    crate::tool::ToolStatus::Error => {
                        tracing::warn!(task = %task.id, error = ?output.error, "task failed");
                    }
                    crate::tool::ToolStatus::AuthRequired => {
                        tracing::warn!(task = %task.id, "task requires authorization");
                    }
                }
            }
        })
    }
}

pub struct TaskScheduler {
    triggers: Vec<Box<dyn Trigger>>,
    queue: TaskQueue,
    interval: Duration,
}

impl TaskScheduler {
    pub fn new(interval: Duration) -> Self {
        TaskScheduler {
            triggers: Vec::new(),
            queue: TaskQueue::new(),
            interval,
        }
    }

    pub fn queue(&self) -> &TaskQueue {
        &self.queue
    }

    pub fn register(&mut self, trigger: impl Trigger + 'static) {
        self.triggers.push(Box::new(trigger));
    }

    pub fn spawn(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(self.interval);
            loop {
                tick.tick().await;
                for trigger in &mut self.triggers {
                    if let Some(task) = trigger.poll() {
                        info!(trigger = %task.source, task = %task.id, "trigger fired");
                        self.queue.submit(task);
                    }
                }
            }
        })
    }
}

pub trait Trigger: Send + Sync {
    fn poll(&mut self) -> Option<Task>;
}

pub struct ReminderTrigger {
    id: String,
    message: String,
    at: chrono::DateTime<chrono::Utc>,
    fired: bool,
}

impl ReminderTrigger {
    pub fn new(
        id: impl Into<String>,
        message: impl Into<String>,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        ReminderTrigger {
            id: id.into(),
            message: message.into(),
            at,
            fired: false,
        }
    }

    /// Create a reminder from an RFC 3339 / ISO 8601 timestamp string.
    /// Returns `None` if the string cannot be parsed.
    pub fn from_rfc3339(
        id: impl Into<String>,
        message: impl Into<String>,
        at: &str,
    ) -> Option<Self> {
        let dt = chrono::DateTime::parse_from_rfc3339(at).ok()?;
        Some(ReminderTrigger::new(
            id,
            message,
            dt.with_timezone(&chrono::Utc),
        ))
    }
}

impl Trigger for ReminderTrigger {
    fn poll(&mut self) -> Option<Task> {
        if self.fired {
            return None;
        }
        if chrono::Utc::now() >= self.at {
            self.fired = true;
            Some(Task {
                id: self.id.clone(),
                source: "reminder".into(),
                tool: "desktop".into(),
                arguments: serde_json::json!({
                    "action": "type",
                    "text": format!("⏰ 提醒: {}", self.message),
                }),
            })
        } else {
            None
        }
    }
}

pub struct IntervalTrigger {
    id: String,
    tool: String,
    args: serde_json::Value,
    interval: Duration,
    last_fired: Option<tokio::time::Instant>,
}

impl IntervalTrigger {
    pub fn new(
        id: impl Into<String>,
        tool: impl Into<String>,
        args: serde_json::Value,
        interval: Duration,
    ) -> Self {
        IntervalTrigger {
            id: id.into(),
            tool: tool.into(),
            args,
            interval,
            last_fired: Some(tokio::time::Instant::now()),
        }
    }
}

impl Trigger for IntervalTrigger {
    fn poll(&mut self) -> Option<Task> {
        let now = tokio::time::Instant::now();
        if self
            .last_fired
            .map_or(true, |last| now.duration_since(last) >= self.interval)
        {
            self.last_fired = Some(now);
            Some(Task {
                id: self.id.clone(),
                source: "interval".into(),
                tool: self.tool.clone(),
                arguments: self.args.clone(),
            })
        } else {
            None
        }
    }
}

pub struct FileWatchTrigger {
    id: String,
    path: PathBuf,
    last_modified: Option<std::time::SystemTime>,
}

impl FileWatchTrigger {
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        FileWatchTrigger {
            id: id.into(),
            path: path.into(),
            last_modified: None,
        }
    }

    fn check_modified(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.path).ok()?.modified().ok()
    }
}

impl Trigger for FileWatchTrigger {
    fn poll(&mut self) -> Option<Task> {
        let current = self.check_modified()?;
        let changed = self.last_modified.map_or(true, |prev| current > prev);
        self.last_modified = Some(current);
        if changed {
            Some(Task {
                id: self.id.clone(),
                source: "file_watch".into(),
                tool: "read_file".into(),
                arguments: serde_json::json!({ "path": self.path.display().to_string() }),
            })
        } else {
            None
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_from_rfc3339_parses_valid() {
        let trigger = ReminderTrigger::from_rfc3339("test-1", "hello", "2027-01-01T00:00:00Z");
        let t = trigger.expect("valid RFC 3339 string");
        assert_eq!(t.id, "test-1");
        assert_eq!(t.message, "hello");
        assert!(!t.fired);
    }

    #[test]
    fn reminder_rejects_bad_date() {
        assert!(ReminderTrigger::from_rfc3339("bad", "x", "not-a-date").is_none());
        assert!(ReminderTrigger::from_rfc3339("bad", "x", "").is_none());
    }

    #[test]
    fn reminder_fires_once_then_stops() {
        let past = chrono::TimeDelta::seconds(1);
        let mut t = ReminderTrigger::new("ding", "stand up", chrono::Utc::now() - past);
        assert!(t.poll().is_some(), "should fire when time is past");
        assert!(t.poll().is_none(), "should not fire twice");
    }

    #[test]
    fn interval_fires_at_expected_rate() {
        let mut t = IntervalTrigger::new(
            "tick",
            "agent",
            serde_json::json!({"k": "v"}),
            Duration::from_millis(1),
        );
        std::thread::sleep(Duration::from_millis(50));
        let task = t.poll().expect("should fire after interval elapses");
        assert_eq!(task.id, "tick");
        assert_eq!(task.tool, "agent");
        // Second immediate poll must NOT fire (interval hasn't elapsed).
        assert!(t.poll().is_none(), "should not fire two ticks in a row");
    }

    #[test]
    fn file_watch_detects_change() {
        let tmp = std::env::temp_dir().join("zhongshu_test_watch.txt");
        let _ = std::fs::write(&tmp, "v1");
        let mut t = FileWatchTrigger::new("watch-1", &tmp);
        let task = t.poll().expect("should fire on first poll");
        assert_eq!(task.source, "file_watch");
        // No change → should NOT fire.
        assert!(t.poll().is_none(), "no change should not fire");
        // Modify file → should fire again.
        let _ = std::fs::write(&tmp, "v2");
        std::thread::sleep(Duration::from_millis(50)); // let filesystem settle
        assert!(t.poll().is_some(), "should fire after modification");
        let _ = std::fs::remove_file(&tmp);
    }
}
