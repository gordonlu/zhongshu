use std::sync::atomic::{AtomicU64, Ordering};

use crate::event::{Event, EventBus, SourceEvent};
use crate::task::{Task, TaskQueue};
use tracing::info;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(0);

/// 规则条件（占位设计，当前仅支持 Always）。
///
/// 后续迭代：key-value 条件匹配、数值比较、时间窗口等。
#[derive(Debug, Clone)]
pub enum RuleCondition {
    Always,
}

/// 规则匹配后产生的 Task 描述。
#[derive(Debug, Clone)]
pub struct RuleTask {
    pub source: String,
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// 单条规则。
///
/// 当 `event_pattern` 匹配事件类型名时触发。
/// 示例：`event_pattern = "tick"` 匹配所有 `SourceEvent::Tick`。
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub event_pattern: String,
    pub source: Option<String>,
    pub condition: RuleCondition,
    pub task: RuleTask,
}

impl Rule {
    /// 检查事件是否命中规则。命中返回格式化后的源名字符串。
    pub fn matches(&self, event: &Event) -> Option<String> {
        if event.type_name() != self.event_pattern {
            return None;
        }
        if let Some(ref src) = self.source {
            match event {
                Event::Source(SourceEvent::Tick { name }) => {
                    if name != src {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        match self.condition {
            RuleCondition::Always => Some(format!("rule:{}", self.id)),
        }
    }
}

/// 规则引擎 —— 静态事件匹配 + Task 产出。
///
/// Layer 2 Routing（Layer 1 = Source 检测, Layer 3 = Worker LLM 分析）。
///
/// RuleEngine 订阅 EventBus，匹配规则，命中则向 TaskQueue 提交 Task。
/// 不调用 LLM。
pub struct RuleEngine {
    rules: Vec<Rule>,
    task_queue: TaskQueue,
    eb: EventBus,
}

impl RuleEngine {
    pub fn new(eb: EventBus, task_queue: TaskQueue) -> Self {
        RuleEngine {
            rules: Vec::new(),
            task_queue,
            eb,
        }
    }

    pub fn add_rule(&mut self, rule: Rule) {
        info!(id = %rule.id, pattern = %rule.event_pattern, "rule registered");
        self.rules.push(rule);
    }

    /// 处理单条事件，检查所有规则。
    pub fn process(&self, event: &Event) {
        for rule in &self.rules {
            if let Some(source) = rule.matches(event) {
                info!(rule = %rule.id, event = %event.type_name(), "rule matched");
                self.task_queue.submit(Task {
                    id: format!("{}-{}", rule.id, NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)),
                    source,
                    tool: rule.task.tool.clone(),
                    arguments: rule.task.arguments.clone(),
                });
            }
        }
    }

    /// 在后台订阅 EventBus 并持续处理事件。
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let mut rx = self.eb.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => self.process(&event),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("rule engine lagged: {n}");
                    }
                    Err(_) => {
                        tracing::info!("rule engine stopped (event bus closed)");
                        break;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::SourceEvent;

    fn tick_event(name: &str) -> Event {
        Event::Source(SourceEvent::Tick { name: name.into() })
    }

    #[test]
    fn rule_matches_event_pattern() {
        let rule = Rule {
            id: "test-rule".into(),
            event_pattern: "tick".into(),
            source: None,
            condition: RuleCondition::Always,
            task: RuleTask {
                source: "test".into(),
                tool: "shell".into(),
                arguments: serde_json::json!({"cmd": "echo hello"}),
            },
        };
        assert!(rule.matches(&tick_event("heartbeat")).is_some());
    }

    #[test]
    fn rule_does_not_match_wrong_pattern() {
        let rule = Rule {
            id: "test".into(),
            event_pattern: "worker_report".into(),
            source: None,
            condition: RuleCondition::Always,
            task: RuleTask {
                source: "x".into(),
                tool: "x".into(),
                arguments: serde_json::json!({}),
            },
        };
        assert!(rule.matches(&tick_event("x")).is_none());
    }

    #[test]
    fn rule_engine_processes_matching_event() {
        let eb = EventBus::new(16);
        let queue = TaskQueue::new();

        let mut engine = RuleEngine::new(eb, queue.clone());
        engine.add_rule(Rule {
            id: "on-tick".into(),
            event_pattern: "tick".into(),
            source: None,
            condition: RuleCondition::Always,
            task: RuleTask {
                source: "heartbeat".into(),
                tool: "agent".into(),
                arguments: serde_json::json!({"prompt": "check"}),
            },
        });

        engine.process(&tick_event("hb"));

        let rt = tokio::runtime::Runtime::new().unwrap();
        let task = rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), queue.recv()).await.ok().flatten()
        });
        assert!(task.is_some());
        if let Some(t) = task {
            assert_eq!(t.tool, "agent");
        }
    }

    #[test]
    fn rule_engine_ignores_non_matching_event() {
        let eb = EventBus::new(16);
        let queue = TaskQueue::new();

        let mut engine = RuleEngine::new(eb, queue.clone());
        engine.add_rule(Rule {
            id: "only-ticks".into(),
            event_pattern: "tick".into(),
            source: None,
            condition: RuleCondition::Always,
            task: RuleTask {
                source: "src".into(),
                tool: "x".into(),
                arguments: serde_json::json!({}),
            },
        });

        // Send a non-tick event
        let event = Event::Memory(crate::event::MemoryEvent::Compacted);
        engine.process(&event);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let task = rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_millis(100), queue.recv()).await.ok().flatten()
        });
        assert!(task.is_none());
    }
}
