//! Attack-case tests for verification integrity.
//!
//! Each test simulates an adversarial verification pattern and verifies
//! that the harness gate or recovery system catches it.

use zhongshu_core::harness::action::HarnessAction;
use zhongshu_core::harness::recovery;
use zhongshu_core::harness::recovery::fingerprint;
use zhongshu_core::harness::recovery::patch_history::PatchHistory;
use zhongshu_core::harness::state::{RecoveryState, VerificationRecord, VerificationState};
use zhongshu_core::harness::verification::gate;

// ═══════════════════════════════════════════════════════════════════════
// Attack: Fake success after failed verification
//   Agent has a failed test on record but claims success.
//   Harness must block finalize.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn fake_success_after_failed_verify_blocks() {
    let state = VerificationState {
        required: false,
        records: vec![VerificationRecord {
            command: "cargo test".into(),
            command_hash: "abc".into(),
            success: false,
            exit_code: Some(1),
            step: 1,
        }],
        last_success: None,
        last_failure: Some(VerificationRecord {
            command: "cargo test".into(),
            command_hash: "abc".into(),
            success: false,
            exit_code: Some(1),
            step: 1,
        }),
        last_edit_step: 1,
        last_verify_step: 1,
        unavailable_reason: None,
    };
    let output = "修复完成，测试通过。";
    let actions = gate::check(&state, output);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "fake success after failed verify must be blocked"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack: Recovery signal emitted after consecutive failure
//   Agent runs the same failing test 3+ times.
//   is_repeated_failure must return true and recovery check must
//   emit repeated_failure feedback.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn recovery_signal_after_failure() {
    let mut state = RecoveryState {
        failures: Vec::new(),
        last_feedback_step: 0,
        consecutive_no_progress: 0,
        patch_history: PatchHistory::new(),
        pending_signals: Vec::new(),
    };
    for _ in 0..3 {
        fingerprint::record(&mut state, "shell", "cargo test", "test foo failed", 1);
    }
    assert!(
        fingerprint::is_repeated_failure(&state, "cargo test", "test foo failed"),
        "3 identical failures must be recognized as repeated"
    );

    let feedback = recovery::check(&mut state, true, false, false, 10);
    assert!(
        feedback
            .iter()
            .any(|fb| fb.rule_id == "recovery/repeated_failure"),
        "recovery check must emit repeated_failure feedback"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack: Final claim cannot hide failed checks
//   Output says "all tests pass" but ledger has a failure record.
//   gate::check must BlockFinalize regardless of the success claim.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn final_claim_cannot_hide_failure() {
    let state = VerificationState {
        required: false,
        records: vec![VerificationRecord {
            command: "cargo test".into(),
            command_hash: "abc".into(),
            success: false,
            exit_code: Some(1),
            step: 3,
        }],
        last_success: Some(VerificationRecord {
            command: "cargo test".into(),
            command_hash: "abc".into(),
            success: true,
            exit_code: Some(0),
            step: 2,
        }),
        last_failure: Some(VerificationRecord {
            command: "cargo test".into(),
            command_hash: "abc".into(),
            success: false,
            exit_code: Some(1),
            step: 3,
        }),
        last_edit_step: 2,
        last_verify_step: 3,
        unavailable_reason: None,
    };
    let output = "all tests pass";
    let actions = gate::check(&state, output);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "final success claim must not hide a failed check"
    );
}
