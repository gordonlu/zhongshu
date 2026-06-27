pub mod feedback;
pub mod fingerprint;
pub mod no_progress;
pub mod patch_history;
pub mod strategy;

use crate::harness::action::{FeedbackSource, HarnessFeedback, Severity};
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
    let repeated_failure = state.failures.iter().any(|f| f.count >= 3);

    if !no_progress && !repeated_patch && !repeated_failure {
        return Vec::new();
    }

    // Dedup: only emit feedback at most once per 3 steps
    if current_step < state.last_feedback_step + 3 {
        return Vec::new();
    }

    let mut feedback = feedback::generate_feedback(state);
    if let Some(hint) = strategy::hint(repeated_patch, repeated_failure, no_progress) {
        feedback.push(HarnessFeedback {
            source: FeedbackSource::Recovery,
            severity: Severity::Warning,
            rule_id: "recovery/strategy_hint".into(),
            message: hint,
            suggestion: String::new(),
            evidence: None,
        });
    }
    if !feedback.is_empty() {
        state.last_feedback_step = current_step;
    }
    feedback
}
