pub mod feedback;
pub mod fingerprint;
pub mod no_progress;
pub mod patch_history;
pub mod policy;
pub mod strategy;

use crate::harness::action::HarnessFeedback;
use crate::harness::recovery::policy::{
    RecoveryPolicy, RecoveryPolicyInput, RecoverySignal, RecoverySignalKind,
};
use crate::harness::state::RecoveryState;

pub fn check(
    state: &mut RecoveryState,
    had_file_read: bool,
    had_successful_edit: bool,
    had_successful_test: bool,
    current_step: u32,
) -> Vec<HarnessFeedback> {
    // Always track no-progress state (before dedup guard so counter remains accurate)
    let no_progress = no_progress::check_no_progress(
        state,
        had_file_read,
        had_successful_edit,
        had_successful_test,
    );
    let repeated_patch = state.patch_history.is_repeated();
    let repeated_failures: Vec<_> = state
        .failures
        .iter()
        .filter(|failure| failure.count >= 3)
        .collect();

    if !no_progress
        && !repeated_patch
        && repeated_failures.is_empty()
        && state.pending_signals.is_empty()
    {
        return Vec::new();
    }

    let mut signals = std::mem::take(&mut state.pending_signals);
    if no_progress {
        signals.push(
            RecoverySignal::new(RecoverySignalKind::NoProgress)
                .with_count(state.consecutive_no_progress),
        );
    }
    if repeated_patch {
        signals.push(RecoverySignal::new(RecoverySignalKind::RepeatedPatch).with_count(3));
    }
    for failure in repeated_failures {
        signals.push(
            RecoverySignal::new(RecoverySignalKind::RepeatedFailure)
                .with_count(failure.count)
                .with_evidence(format!(
                    "failure fingerprint: {}",
                    failure.error_fingerprint
                )),
        );
    }

    let decision = RecoveryPolicy::default().evaluate(RecoveryPolicyInput {
        signals,
        current_step,
        last_feedback_step: state.last_feedback_step,
    });
    if decision.triggered {
        state.last_feedback_step = current_step;
    }
    decision.feedback
}

pub fn record_signal(state: &mut RecoveryState, signal: RecoverySignal) {
    state.pending_signals.push(signal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::recovery::patch_history::PatchHistory;
    use crate::harness::state::{FailureSignature, RecoveryState};

    fn state() -> RecoveryState {
        RecoveryState {
            failures: Vec::new(),
            last_feedback_step: 0,
            consecutive_no_progress: 0,
            patch_history: PatchHistory::new(),
            pending_signals: Vec::new(),
        }
    }

    #[test]
    fn check_emits_no_progress_feedback_through_policy() {
        let mut state = state();
        let mut feedback = Vec::new();
        for step in 1..=5 {
            feedback = check(&mut state, false, false, false, step);
        }

        assert!(feedback
            .iter()
            .any(|fb| fb.rule_id == "recovery/no_progress"));
        assert!(feedback
            .iter()
            .any(|fb| fb.message.contains("没有取得进展")));
    }

    #[test]
    fn check_respects_feedback_cooldown() {
        let mut state = state();
        state.consecutive_no_progress = 4;
        state.last_feedback_step = 4;

        let feedback = check(&mut state, false, false, false, 5);

        assert!(feedback.is_empty());
    }

    #[test]
    fn check_combines_repeated_failure_and_patch() {
        let mut state = state();
        state.failures.push(FailureSignature {
            command_hash: "abc".into(),
            error_fingerprint: "abc".into(),
            count: 3,
            first_seen_step: 1,
        });
        state.patch_history.record("same patch");
        state.patch_history.record("same patch");
        state.patch_history.record("same patch");

        let feedback = check(&mut state, true, false, false, 10);

        assert!(feedback
            .iter()
            .any(|fb| { fb.rule_id == "recovery/root_cause_before_more_patches" }));
    }
}
