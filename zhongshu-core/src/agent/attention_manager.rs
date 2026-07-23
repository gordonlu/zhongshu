use std::sync::{Arc, Mutex};

use crate::agent::attention::AttentionLevel;
use crate::agent::report::Report;
use crate::event::{AttentionEvent, Event, EventBus};

/// 系统通知总控。
///
/// 所有 Report 必须经过 AttentionManager。
/// 它决定每条 Report 的通知层级，并发布对应的 AttentionEvent。
///
/// 生命周期：
/// 1. 订阅 EventBus 上的 WorkerReport 事件
/// 2. 根据 AttentionLevel 路由：
///    - Immediate → EventBus.Attention.Interrupt
///    - Notify    → EventBus.Attention.Notify
///    - Digest    → 暂存队列（可通过 digest_queue 共享访问）
///    - Ignore    → 丢弃
/// 3. digest_queue 供 Daily Digest 消费
pub struct AttentionManager {
    eb: EventBus,
    digest_queue: Arc<Mutex<Vec<Report>>>,
    capacity: usize,
}

impl AttentionManager {
    /// 创建 AttentionManager，绑定到 EventBus。
    pub fn new(eb: EventBus) -> Self {
        AttentionManager {
            eb,
            digest_queue: Arc::new(Mutex::new(Vec::new())),
            capacity: 100,
        }
    }

    /// 返回 digest queue 的共享引用，供外部（如 DigestBuilder）安全 drain。
    pub fn digest_queue(&self) -> Arc<Mutex<Vec<Report>>> {
        self.digest_queue.clone()
    }

    /// 取出所有待汇总的 Digest Report。
    pub fn drain_digest(&mut self) -> Vec<Report> {
        let mut dq = self.digest_queue.lock().unwrap();
        std::mem::take(&mut *dq)
    }

    /// 通过 Arc<Mutex<Vec<Report>>> 取出所有待汇总的 Digest Report。
    pub fn drain_queue(queue: &Arc<Mutex<Vec<Report>>>) -> Vec<Report> {
        let mut guard = queue.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    /// 处理单个 Report，根据 AttentionLevel 路由。
    pub fn process(&mut self, report: Report) {
        tracing::trace!(worker = %report.worker, level = ?report.attention, "attention manager processing report");
        match report.attention {
            AttentionLevel::Immediate => {
                self.eb
                    .publish(Event::Attention(AttentionEvent::Interrupt { report }));
            }
            AttentionLevel::Notify => {
                self.eb
                    .publish(Event::Attention(AttentionEvent::Notify { report }));
            }
            AttentionLevel::Digest => {
                let mut dq = self.digest_queue.lock().unwrap();
                if dq.len() >= self.capacity {
                    tracing::warn!(
                        dropped_task = %dq[0].task_id,
                        dropped_worker = %dq[0].worker,
                        "digest queue full, dropping oldest report"
                    );
                    dq.remove(0);
                }
                dq.push(report);
            }
            AttentionLevel::Ignore => {
                tracing::debug!("ignoring report");
            }
        }
    }

    /// 在后台订阅 EventBus，持续处理 WorkerReport。
    ///
    /// 返回 (digest_queue 共享引用, JoinHandle)。
    pub fn spawn(self) -> (Arc<Mutex<Vec<Report>>>, tokio::task::JoinHandle<()>) {
        let queue = self.digest_queue.clone();
        let q = queue.clone();
        let capacity = self.capacity;
        let eb = self.eb.clone();
        let mut rx = self.eb.subscribe();
        let handle = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(Event::Agent(crate::event::AgentEvent::WorkerReport(report))) => {
                        tracing::trace!(worker = %report.worker, level = ?report.attention, "attention manager processing report");
                        match report.attention {
                            AttentionLevel::Immediate => {
                                eb.publish(Event::Attention(AttentionEvent::Interrupt { report }));
                            }
                            AttentionLevel::Notify => {
                                eb.publish(Event::Attention(AttentionEvent::Notify { report }));
                            }
                            AttentionLevel::Digest => {
                                let mut dq = q.lock().unwrap();
                                if dq.len() >= capacity {
                                    tracing::warn!(
                                        dropped_task = %dq[0].task_id,
                                        dropped_worker = %dq[0].worker,
                                        "digest queue full, dropping oldest report"
                                    );
                                    dq.remove(0);
                                }
                                dq.push(report);
                            }
                            AttentionLevel::Ignore => {
                                tracing::debug!("ignoring report");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("attention manager lagged: {n}");
                    }
                    Err(_) => {
                        tracing::info!("attention manager stopped (event bus closed)");
                        break;
                    }
                }
            }
        });
        (queue, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::attention::AttentionLevel;

    fn dummy_report(level: AttentionLevel) -> Report {
        Report {
            task_id: "t1".into(),
            worker: "test".into(),
            run_id: "test-run".into(),
            summary: "test".into(),
            findings: "test".into(),
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            confidence: 0.5,
            attention: level,
            trace_events: vec![],
        }
    }

    #[test]
    fn process_immediate_publishes_interrupt() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let mut mgr = AttentionManager::new(eb);

        mgr.process(dummy_report(AttentionLevel::Immediate));
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event,
            Event::Attention(AttentionEvent::Interrupt { .. })
        ));
    }

    #[test]
    fn process_notify_publishes_notify() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let mut mgr = AttentionManager::new(eb);

        mgr.process(dummy_report(AttentionLevel::Notify));
        let event = rx.try_recv().unwrap();
        assert!(matches!(
            event,
            Event::Attention(AttentionEvent::Notify { .. })
        ));
    }

    #[test]
    fn process_digest_queues_report() {
        let eb = EventBus::new(16);
        let mut mgr = AttentionManager::new(eb);

        mgr.process(dummy_report(AttentionLevel::Digest));
        mgr.process(dummy_report(AttentionLevel::Digest));

        let drained = mgr.drain_digest();
        assert_eq!(drained.len(), 2);
    }

    #[test]
    fn process_ignore_discards() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let mut mgr = AttentionManager::new(eb);

        mgr.process(dummy_report(AttentionLevel::Ignore));
        assert!(rx.try_recv().is_err());
        assert!(mgr.drain_digest().is_empty());
    }

    #[test]
    fn digest_queue_respects_capacity() {
        let eb = EventBus::new(16);
        let mut mgr = AttentionManager::new(eb);
        mgr.capacity = 2;

        mgr.process(dummy_report(AttentionLevel::Digest));
        mgr.process(dummy_report(AttentionLevel::Digest));
        mgr.process(dummy_report(AttentionLevel::Digest)); // should drop oldest

        assert_eq!(mgr.digest_queue.lock().unwrap().len(), 2);
    }
}
