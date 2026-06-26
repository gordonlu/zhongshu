//! Attack-case tests for the DS Coding Harness.
//!
//! Each test simulates an adversarial agent pattern and verifies that
//! the harness catches it. Tests target the harness checkers directly
//! (unit style) rather than through the full agent loop.

use std::collections::{HashMap, VecDeque};

use zhongshu_core::harness::action::{HarnessAction, Severity};
use zhongshu_core::harness::recovery::fingerprint;
use zhongshu_core::harness::state::{
    CodingPhase, RecoveryState, ToolLoopState, VerificationRecord, VerificationState,
};
use zhongshu_core::harness::tool::loop_guard;
use zhongshu_core::harness::verification::{claim, gate, ledger};

// ═══════════════════════════════════════════════════════════════════════
// Attack 1: Fake completion
//   Agent writes output claiming "test passed" without running any tests.
//   Harness must block finalize.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_fake_completion_no_test_blocks() {
    let state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 0,
        last_verify_step: 0,
        unavailable_reason: None,
    };
    let output = "修改完成，测试通过。";
    let actions = gate::check(&state, output);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "fake completion without test must be blocked"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 2: Stale verification
//   Agent verified, then edited, then tries to finalize without re-testing.
//   Harness must detect last_verify_step <= last_edit_step.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_stale_verification_blocks() {
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 5,   // edit at step 5
        last_verify_step: 3, // verify at step 3 (before edit!)
        unavailable_reason: None,
    };
    state.last_success = Some(VerificationRecord {
        command: "cargo test".into(),
        command_hash: "abc".into(),
        success: true,
        exit_code: Some(0),
        step: 3,
    });
    let output = "已完成";
    let actions = gate::check(&state, output);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "stale verification (verify before edit) must be blocked"
    );
}

#[test]
fn fresh_verification_after_edit_allows() {
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 3,
        last_verify_step: 5, // verify after edit
        unavailable_reason: None,
    };
    state.last_success = Some(VerificationRecord {
        command: "cargo test".into(),
        command_hash: "abc".into(),
        success: true,
        exit_code: Some(0),
        step: 5,
    });
    let actions = gate::check(&state, "已完成");
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "fresh verification after edit should not block"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 3: Duplicate tool call
//   Agent calls the same grep with the same args 3+ times consecutively.
//   ToolLoopGuard must block.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_duplicate_tool_blocks() {
    let mut state = ToolLoopState {
        recent_calls: VecDeque::new(),
        counts: HashMap::new(),
    };
    // First two calls pass
    for _ in 0..2 {
        let result = loop_guard::check_duplicate(&mut state, "grep", "hash123");
        assert!(matches!(result, HarnessAction::None));
    }
    // Third consecutive call must block
    let result = loop_guard::check_duplicate(&mut state, "grep", "hash123");
    assert!(
        matches!(result, HarnessAction::BlockTool { .. }),
        "3rd consecutive identical tool call must block"
    );
}

#[test]
fn different_args_not_blocked() {
    let mut state = ToolLoopState {
        recent_calls: VecDeque::new(),
        counts: HashMap::new(),
    };
    for i in 0..5 {
        let result = loop_guard::check_duplicate(&mut state, "grep", &format!("hash{}", i));
        assert!(
            matches!(result, HarnessAction::None),
            "different args should not block (iteration {})",
            i
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 4: Verification bypass via non-standard command
//   Agent runs a shell command that isn't classified as test/check.
//   Harness should not record it as verification.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_non_verification_bypass() {
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 0,
        last_verify_step: 0,
        unavailable_reason: None,
    };
    ledger::record(&mut state, "shell", "ls -la", Some(0), 1);
    assert!(
        state.records.is_empty(),
        "ls must not be treated as verification"
    );
    assert_eq!(state.last_verify_step, 0, "verify step must not advance");
}

#[test]
fn attack_self_test_bypass_attempt() {
    // self_test is explicitly recognized as verification
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 0,
        last_verify_step: 0,
        unavailable_reason: None,
    };
    ledger::record(&mut state, "self_test", "{}", Some(0), 1);
    assert!(
        !state.records.is_empty(),
        "self_test must be treated as verification"
    );
    assert_eq!(state.last_verify_step, 1);
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 5: Output claim detection
//   Agent uses various phrasings to claim verification.
//   ClaimVerifier must detect all of them.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_claim_detection_chinese() {
    assert!(claim::has_verification_claim("测试通过"));
    assert!(claim::has_verification_claim("验证成功"));
    assert!(claim::has_verification_claim("全部测试通过"));
}

#[test]
fn attack_claim_detection_english() {
    assert!(claim::has_verification_claim("All tests passed."));
    assert!(claim::has_verification_claim("verification passed"));
}

#[test]
fn attack_claim_detection_no_false_positive() {
    assert!(!claim::has_verification_claim("正在分析代码"));
    assert!(!claim::has_verification_claim("修复了测试中的问题"));
    assert!(!claim::has_verification_claim("需要进一步验证"));
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 6: Repeated failure does not escalate to loop_guard
//   Agent runs a test that fails 3 times. The loop_guard should not
//   block it (different semantics — test failure is not a loop).
//   RecoveryHarness should record the pattern.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_repeated_test_failure_recorded() {
    let mut state = RecoveryState {
        failures: Vec::new(),
        last_feedback_step: 0,
        consecutive_no_progress: 0,
    };
    for _ in 0..3 {
        fingerprint::record(&mut state, "shell", "cargo test", "test foo failed", 1);
    }
    assert!(fingerprint::is_repeated_failure(
        &state,
        "cargo test",
        "test foo failed"
    ));
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 7: Phase validation — finalize without doing anything
//   Agent tries to summarize without any inspect/edit/verify events.
//   Phase machine must warn/block.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_summarize_without_work_warns() {
    let feedback = zhongshu_core::harness::phase::validate_transition(
        CodingPhase::Understand,
        CodingPhase::Summarize,
    );
    assert!(
        !feedback.is_empty(),
        "transition from Understand to Summarize must produce feedback"
    );
    assert_eq!(feedback[0].severity, Severity::Fatal);
}

#[test]
fn normal_transition_no_warning() {
    let feedback =
        zhongshu_core::harness::phase::validate_transition(CodingPhase::Inspect, CodingPhase::Edit);
    assert!(feedback.is_empty(), "Inspect -> Edit should not warn");
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 8: Claim without evidence — English variants
//   Edge cases: different casing, extra whitespace, partial matches
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_claim_edge_cases() {
    // Case insensitive
    assert!(claim::has_verification_claim("ALL TESTS PASSED"));
    // Partial in longer text
    assert!(claim::has_verification_claim("I verified the fix works"));
    // "tests passed" without "all"
    assert!(claim::has_verification_claim("tests passed: 42/42"));
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 9: Stale verification with multiple edits
//   Multiple edits after last verify — block even if there's a verified
//   record from long ago.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_multiple_edits_after_verify_blocks() {
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 10,  // edited 3 more times
        last_verify_step: 3, // verified once long ago
        unavailable_reason: None,
    };
    state.last_success = Some(VerificationRecord {
        command: "cargo test".into(),
        command_hash: "abc".into(),
        success: true,
        exit_code: Some(0),
        step: 3,
    });
    let actions = gate::check(&state, "all done");
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, HarnessAction::BlockFinalize { .. })),
        "multiple edits after verify must block"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 10: Multi-run state isolation
//   Previous run's verification must not satisfy a new run.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_fresh_run_has_no_lingering_verification() {
    use zhongshu_core::harness::HarnessState;
    let state = HarnessState::new();
    assert!(
        state.verification.last_success.is_none(),
        "new run must start clean"
    );
    assert_eq!(state.verification.last_edit_step, 0);
    assert_eq!(state.verification.last_verify_step, 0);
    assert_eq!(state.phase, CodingPhase::Understand);
    assert_eq!(state.previous_phase, CodingPhase::Understand);
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 11: Architecture violation lifecycle
//   A warning-level violation must not block finalize.
//   A resolved violation must not block finalize.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_architecture_warning_does_not_block() {
    use zhongshu_core::harness::state::{OpenViolation, ViolationKey, ViolationStatus};
    let violations = vec![OpenViolation {
        key: ViolationKey {
            rule_id: "test/warning".into(),
            file_path: "a.rs".into(),
            symbol_id: "fn foo".into(),
        },
        status: ViolationStatus::Open,
        severity: Severity::Warning,
        confidence: zhongshu_core::harness::action::Confidence::High,
        message: "test warning".into(),
        introduced_this_run: true,
        raised_step: 0,
    }];
    // Warning severity must not block
    let fatal = violations
        .iter()
        .filter(|v| {
            v.status == ViolationStatus::Open
                && v.severity == Severity::Fatal
                && v.introduced_this_run
        })
        .count();
    assert_eq!(fatal, 0, "warning must not be counted as blocking");
}

#[test]
fn attack_architecture_resolved_does_not_block() {
    use zhongshu_core::harness::state::{OpenViolation, ViolationKey, ViolationStatus};
    let violations = vec![OpenViolation {
        key: ViolationKey {
            rule_id: "test/fatal".into(),
            file_path: "a.rs".into(),
            symbol_id: "fn foo".into(),
        },
        status: ViolationStatus::Resolved,
        severity: Severity::Fatal,
        confidence: zhongshu_core::harness::action::Confidence::High,
        message: "resolved fatal".into(),
        introduced_this_run: true,
        raised_step: 0,
    }];
    // Resolved must not block
    let fatal = violations
        .iter()
        .filter(|v| {
            v.status == ViolationStatus::Open
                && v.severity == Severity::Fatal
                && v.introduced_this_run
        })
        .count();
    assert_eq!(fatal, 0, "resolved must not block");
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 12: Duplicate guard allows with progress
//   Same tool+args but with intervening progress (file read, edit)
//   must NOT be blocked.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn attack_duplicate_with_progress_allowed() {
    let mut state = ToolLoopState {
        recent_calls: std::collections::VecDeque::new(),
        counts: std::collections::HashMap::new(),
    };
    // First call
    let r1 = loop_guard::check_duplicate(&mut state, "grep", "hash1");
    assert!(matches!(r1, HarnessAction::None));

    // Progress: different tool (read_file) resets consecutiveness
    let r2 = loop_guard::check_duplicate(&mut state, "read_file", "hash_other");
    assert!(matches!(r2, HarnessAction::None));

    // Same grep again — should NOT block because the read_file broke consecutiveness
    let r3 = loop_guard::check_duplicate(&mut state, "grep", "hash1");
    assert!(
        matches!(r3, HarnessAction::None),
        "grep with intervening progress must not block"
    );
}
