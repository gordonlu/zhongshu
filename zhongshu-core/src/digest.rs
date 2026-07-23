use tracing::info;

use crate::agent::attention::AttentionLevel;
use crate::agent::report::Report;
use crate::event::{AttentionEvent, Event, EventBus};

/// Digest 构建器。
///
/// 从 AttentionManager 的 digest 队列收集 Report，汇总为单条摘要 Report，
/// 通过 AttentionManager 路由到用户（通常为 Notify 层级）。
///
/// 当前阶段：
/// - 手动触发 `build_and_send()`
/// - 后续接入定时触发器（每日/每周）
pub struct DigestBuilder {
    eb: EventBus,
}

impl DigestBuilder {
    pub fn new(eb: EventBus) -> Self {
        DigestBuilder { eb }
    }

    /// 从一批 Report 构建摘要 Report，并通过 AttentionManager 发出。
    ///
    /// 如果 `reports` 为空，跳过。
    /// 生成的摘要 Report 的 attention 为 `Notify`。
    pub fn build_and_send(&self, reports: Vec<Report>) {
        if reports.is_empty() {
            info!("digest: no reports to summarize");
            return;
        }

        let worker_names: Vec<&str> = {
            let mut seen = std::collections::BTreeSet::new();
            reports
                .iter()
                .filter(|r| seen.insert(r.worker.as_str()))
                .map(|r| r.worker.as_str())
                .collect()
        };
        let total = reports.len();

        let summary = format!(
            "## 每日摘要\n\n共 {} 条来自 {} 的报告\n",
            total,
            worker_names.join(", ")
        );

        let findings: String = reports
            .iter()
            .map(|r| {
                let truncated: String = r.findings.chars().take(1000).collect();
                let findings_display = if r.findings.chars().count() > 1000 {
                    format!("{}...", truncated)
                } else {
                    truncated
                };
                format!("### [{}] {}\n{}\n", r.worker, r.summary, findings_display)
            })
            .collect::<Vec<_>>()
            .join("\n---\n");

        let ts = chrono::Utc::now().timestamp();
        let digest_report = Report {
            task_id: format!("digest-{ts}"),
            worker: "digest".into(),
            run_id: format!("digest-{ts}"),
            summary,
            findings,
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            confidence: 0.8,
            attention: AttentionLevel::Notify,
            trace_events: vec![],
        };

        info!(worker = "digest", reports = total, "sending digest report");

        self.eb.publish(Event::Attention(AttentionEvent::Notify {
            report: digest_report,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::attention::AttentionLevel;

    fn dummy_report(worker: &str, summary: &str) -> Report {
        Report {
            task_id: "t1".into(),
            worker: worker.into(),
            run_id: "unknown".into(),
            summary: summary.into(),
            findings: "detailed findings".into(),
            success: true,
            outcome: crate::agent::RunOutcome::CompletedVerified,
            confidence: 0.7,
            attention: AttentionLevel::Digest,
            trace_events: vec![],
        }
    }

    #[test]
    fn digest_empty_reports_skips() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let builder = DigestBuilder::new(eb);

        builder.build_and_send(vec![]);

        // No event should be published for empty digest
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn digest_sends_notify_event() {
        let eb = EventBus::new(16);
        let mut rx = eb.subscribe();
        let builder = DigestBuilder::new(eb);

        builder.build_and_send(vec![
            dummy_report("qintianjian", "天气晴"),
            dummy_report("shumiyuan", "3 条新通知"),
        ]);

        let event = rx.try_recv().expect("should publish event");
        match event {
            Event::Attention(AttentionEvent::Notify { report }) => {
                assert!(report.summary.contains("2 条"));
                assert!(report.summary.contains("qintianjian"));
                assert!(report.summary.contains("shumiyuan"));
                assert_eq!(report.attention, AttentionLevel::Notify);
            }
            _ => panic!("expected Notify event"),
        }
    }
}
