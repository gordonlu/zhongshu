use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
use crate::harness::state::RecoveryState;

pub fn generate_feedback(state: &RecoveryState) -> Vec<HarnessFeedback> {
    let mut feedback = Vec::new();

    for failure in &state.failures {
        if failure.count >= 3 {
            feedback.push(HarnessFeedback {
                source: FeedbackSource::Recovery,
                severity: Severity::Warning,
                rule_id: "recovery/repeated_failure".into(),
                message: format!("同一个命令已连续失败 {} 次。", failure.count),
                suggestion: "请先总结失败原因，再换一个修复策略。不要重复相同的操作。".into(),
                evidence: Some(format!("失败 fingerprint: {}", failure.error_fingerprint)),
            });
        }
    }

    feedback
}
