use serde::{Deserialize, Serialize};

use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    pub feedback_cooldown_steps: u32,
    pub stop_after_no_progress_count: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            feedback_cooldown_steps: 3,
            stop_after_no_progress_count: 8,
        }
    }
}

impl RecoveryPolicy {
    pub fn evaluate(&self, input: RecoveryPolicyInput) -> RecoveryDecision {
        if input.signals.is_empty() {
            return RecoveryDecision::empty();
        }

        if input.last_feedback_step > 0
            && input.current_step < input.last_feedback_step + self.feedback_cooldown_steps
        {
            return RecoveryDecision {
                suppressed: true,
                ..RecoveryDecision::empty()
            };
        }

        let mut actions = Vec::new();
        let mut feedback = Vec::new();

        let has_repeated_patch = input.has_signal(RecoverySignalKind::RepeatedPatch);
        let has_repeated_failure = input.has_signal(RecoverySignalKind::RepeatedFailure);
        let has_no_progress = input.has_signal(RecoverySignalKind::NoProgress);

        if has_repeated_patch && has_repeated_failure {
            actions.push(RecoveryAction::AnalyzeRootCause);
            actions.push(RecoveryAction::GenerateSmallerPatch);
            actions.push(RecoveryAction::SpawnReadOnlyDiagnosticWorker);
            feedback.push(policy_feedback(
                "recovery/root_cause_before_more_patches",
                Severity::Warning,
                "同一个错误反复出现且 patch 高度相似，建议：先分析错误的根本原因，而不是继续修改同一个函数。",
                "Stop editing the same area until the failure signature has been explained.",
                input.evidence_for(&[
                    RecoverySignalKind::RepeatedPatch,
                    RecoverySignalKind::RepeatedFailure,
                ]),
            ));
        } else if has_repeated_failure {
            actions.push(RecoveryAction::AnalyzeRootCause);
            actions.push(RecoveryAction::NarrowVerificationCommand);
            feedback.push(policy_feedback(
                "recovery/repeated_failure",
                Severity::Warning,
                "同一个测试或命令持续失败，建议：先检查失败签名和测试环境，再继续修改代码。",
                "Run a narrower diagnostic command or inspect the failing assertion before patching again.",
                input.evidence_for(&[RecoverySignalKind::RepeatedFailure]),
            ));
        } else if has_repeated_patch {
            actions.push(RecoveryAction::GenerateSmallerPatch);
            actions.push(RecoveryAction::RereadCurrentFiles);
            feedback.push(policy_feedback(
                "recovery/repeated_patch",
                Severity::Warning,
                "连续 patch 高度相似，建议：重新读取当前文件并生成更小的修改。",
                "Re-read the target file and avoid repeating the same patch.",
                input.evidence_for(&[RecoverySignalKind::RepeatedPatch]),
            ));
        }

        if has_no_progress {
            actions.push(RecoveryAction::RereadCurrentFiles);
            let no_progress_count = input
                .signals
                .iter()
                .find(|signal| signal.kind == RecoverySignalKind::NoProgress)
                .map(|signal| signal.count)
                .unwrap_or_default();
            if no_progress_count >= self.stop_after_no_progress_count {
                actions.push(RecoveryAction::StopWithClearBlocker);
            }
            feedback.push(policy_feedback(
                "recovery/no_progress",
                Severity::Warning,
                "连续多轮没有取得进展，建议：重新梳理任务目标，确认当前的修改方向是否正确。",
                "Re-read the task and current files before taking another action.",
                input.evidence_for(&[RecoverySignalKind::NoProgress]),
            ));
        }

        for signal in &input.signals {
            match signal.kind {
                RecoverySignalKind::ToolTimeout => {
                    actions.push(RecoveryAction::NarrowVerificationCommand);
                    feedback.push(policy_feedback(
                        "recovery/tool_timeout",
                        Severity::Warning,
                        "工具执行超时，建议：改用更小范围的命令或检查是否被外部依赖阻塞。",
                        "Use a narrower command or split the tool call into smaller steps.",
                        signal.evidence.clone(),
                    ));
                }
                RecoverySignalKind::PermissionBlocked => {
                    actions.push(RecoveryAction::RequestApprovalOrAlternative);
                    feedback.push(policy_feedback(
                        "recovery/permission_blocked",
                        Severity::Warning,
                        "操作被权限策略阻止，建议：请求明确授权或选择不需要该权限的方案。",
                        "Ask for approval only when the operation is necessary.",
                        signal.evidence.clone(),
                    ));
                }
                RecoverySignalKind::ContextPressure => {
                    actions.push(RecoveryAction::CompactContext);
                    feedback.push(policy_feedback(
                        "recovery/context_pressure",
                        Severity::Warning,
                        "上下文压力过高，建议：压缩旧工具结果并保留当前工作集证据。",
                        "Compact old evidence before continuing.",
                        signal.evidence.clone(),
                    ));
                }
                RecoverySignalKind::PatchRejected => {
                    actions.push(RecoveryAction::RereadCurrentFiles);
                    actions.push(RecoveryAction::GenerateSmallerPatch);
                    feedback.push(policy_feedback(
                        "recovery/patch_rejected",
                        Severity::Warning,
                        "patch 被拒绝，建议：重新读取文件并缩小修改范围。",
                        "Use the patch failure evidence to regenerate a smaller patch.",
                        signal.evidence.clone(),
                    ));
                }
                RecoverySignalKind::VerificationFailed => {
                    actions.push(RecoveryAction::AnalyzeRootCause);
                    actions.push(RecoveryAction::NarrowVerificationCommand);
                    feedback.push(policy_feedback(
                        "recovery/verification_failed",
                        Severity::Warning,
                        "验证失败，建议：先定位失败签名，再决定是否继续修改。",
                        "Do not claim success until a later verification run passes.",
                        signal.evidence.clone(),
                    ));
                }
                RecoverySignalKind::RepeatedPatch
                | RecoverySignalKind::RepeatedFailure
                | RecoverySignalKind::NoProgress => {}
            }
        }

        actions.sort();
        actions.dedup();
        feedback.dedup_by(|a, b| a.rule_id == b.rule_id);

        RecoveryDecision {
            triggered: !actions.is_empty() || !feedback.is_empty(),
            suppressed: false,
            actions,
            feedback,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPolicyInput {
    pub signals: Vec<RecoverySignal>,
    pub current_step: u32,
    pub last_feedback_step: u32,
}

impl RecoveryPolicyInput {
    fn has_signal(&self, kind: RecoverySignalKind) -> bool {
        self.signals.iter().any(|signal| signal.kind == kind)
    }

    fn evidence_for(&self, kinds: &[RecoverySignalKind]) -> Option<String> {
        self.signals
            .iter()
            .find(|signal| kinds.contains(&signal.kind))
            .and_then(|signal| signal.evidence.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySignal {
    pub kind: RecoverySignalKind,
    pub count: u32,
    pub evidence: Option<String>,
}

impl RecoverySignal {
    pub fn new(kind: RecoverySignalKind) -> Self {
        Self {
            kind,
            count: 1,
            evidence: None,
        }
    }

    pub fn with_count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence = Some(evidence.into());
        self
    }

    pub fn patch_rejected(evidence: &crate::patch::PatchFailureEvidence) -> Self {
        Self::new(RecoverySignalKind::PatchRejected)
            .with_evidence(format!("{}: {}", evidence.error_code, evidence.message))
    }

    pub fn verification_failed(summary: impl Into<String>) -> Self {
        Self::new(RecoverySignalKind::VerificationFailed).with_evidence(summary)
    }

    pub fn tool_timeout(tool_name: impl AsRef<str>, timeout: std::time::Duration) -> Self {
        Self::new(RecoverySignalKind::ToolTimeout).with_evidence(format!(
            "{} timed out after {:?}",
            tool_name.as_ref(),
            timeout
        ))
    }

    pub fn permission_blocked(reason: impl Into<String>) -> Self {
        Self::new(RecoverySignalKind::PermissionBlocked).with_evidence(reason)
    }

    pub fn context_pressure(reason: impl Into<String>) -> Self {
        Self::new(RecoverySignalKind::ContextPressure).with_evidence(reason)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySignalKind {
    NoProgress,
    RepeatedFailure,
    RepeatedPatch,
    PatchRejected,
    VerificationFailed,
    ToolTimeout,
    PermissionBlocked,
    ContextPressure,
}

pub struct RecoveryDecision {
    pub triggered: bool,
    pub suppressed: bool,
    pub actions: Vec<RecoveryAction>,
    pub feedback: Vec<HarnessFeedback>,
}

impl RecoveryDecision {
    pub fn empty() -> Self {
        Self {
            triggered: false,
            suppressed: false,
            actions: Vec::new(),
            feedback: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    RereadCurrentFiles,
    AnalyzeRootCause,
    NarrowVerificationCommand,
    GenerateSmallerPatch,
    SpawnReadOnlyDiagnosticWorker,
    RequestApprovalOrAlternative,
    CompactContext,
    StopWithClearBlocker,
}

fn policy_feedback(
    rule_id: impl Into<String>,
    severity: Severity,
    message: impl Into<String>,
    suggestion: impl Into<String>,
    evidence: Option<String>,
) -> HarnessFeedback {
    HarnessFeedback {
        source: FeedbackSource::Recovery,
        severity,
        rule_id: rule_id.into(),
        message: message.into(),
        suggestion: suggestion.into(),
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(signals: Vec<RecoverySignal>) -> RecoveryPolicyInput {
        RecoveryPolicyInput {
            signals,
            current_step: 10,
            last_feedback_step: 0,
        }
    }

    #[test]
    fn empty_input_does_not_trigger() {
        let decision = RecoveryPolicy::default().evaluate(input(Vec::new()));

        assert!(!decision.triggered);
        assert!(decision.actions.is_empty());
    }

    #[test]
    fn repeated_failure_and_patch_prioritizes_root_cause() {
        let decision = RecoveryPolicy::default().evaluate(input(vec![
            RecoverySignal::new(RecoverySignalKind::RepeatedFailure)
                .with_count(3)
                .with_evidence("failure fp"),
            RecoverySignal::new(RecoverySignalKind::RepeatedPatch).with_count(3),
        ]));

        assert!(decision.triggered);
        assert!(decision.actions.contains(&RecoveryAction::AnalyzeRootCause));
        assert!(decision
            .actions
            .contains(&RecoveryAction::SpawnReadOnlyDiagnosticWorker));
        assert!(decision
            .feedback
            .iter()
            .any(|fb| fb.rule_id == "recovery/root_cause_before_more_patches"));
    }

    #[test]
    fn no_progress_can_stop_after_threshold() {
        let decision = RecoveryPolicy::default().evaluate(input(vec![RecoverySignal::new(
            RecoverySignalKind::NoProgress,
        )
        .with_count(8)]));

        assert!(decision
            .actions
            .contains(&RecoveryAction::RereadCurrentFiles));
        assert!(decision
            .actions
            .contains(&RecoveryAction::StopWithClearBlocker));
        assert!(decision
            .feedback
            .iter()
            .any(|fb| fb.message.contains("没有取得进展")));
    }

    #[test]
    fn cooldown_suppresses_feedback() {
        let decision = RecoveryPolicy::default().evaluate(RecoveryPolicyInput {
            signals: vec![RecoverySignal::new(RecoverySignalKind::NoProgress).with_count(5)],
            current_step: 4,
            last_feedback_step: 2,
        });

        assert!(decision.suppressed);
        assert!(!decision.triggered);
        assert!(decision.feedback.is_empty());
    }

    #[test]
    fn permission_block_requests_approval_or_alternative() {
        let decision = RecoveryPolicy::default().evaluate(input(vec![RecoverySignal::new(
            RecoverySignalKind::PermissionBlocked,
        )
        .with_evidence("auth required")]));

        assert!(decision
            .actions
            .contains(&RecoveryAction::RequestApprovalOrAlternative));
        assert_eq!(decision.feedback[0].rule_id, "recovery/permission_blocked");
    }

    #[test]
    fn patch_failure_signal_requests_reread_and_smaller_patch() {
        let evidence = crate::patch::PatchFailureEvidence {
            operation: crate::patch::PatchOperationKind::Replace,
            path: Some(std::path::PathBuf::from("src/lib.rs")),
            error_code: "stale_read".into(),
            message: "file changed since read".into(),
            recoverable: true,
            suggested_action: "re-read the file".into(),
        };

        let decision = RecoveryPolicy::default()
            .evaluate(input(vec![RecoverySignal::patch_rejected(&evidence)]));

        assert!(decision
            .actions
            .contains(&RecoveryAction::RereadCurrentFiles));
        assert!(decision
            .actions
            .contains(&RecoveryAction::GenerateSmallerPatch));
        assert!(decision
            .feedback
            .iter()
            .any(|fb| fb.rule_id == "recovery/patch_rejected"));
    }

    #[test]
    fn timeout_and_context_pressure_have_distinct_actions() {
        let decision = RecoveryPolicy::default().evaluate(input(vec![
            RecoverySignal::tool_timeout("shell", std::time::Duration::from_secs(120)),
            RecoverySignal::context_pressure("context over budget"),
        ]));

        assert!(decision
            .actions
            .contains(&RecoveryAction::NarrowVerificationCommand));
        assert!(decision.actions.contains(&RecoveryAction::CompactContext));
    }
}
