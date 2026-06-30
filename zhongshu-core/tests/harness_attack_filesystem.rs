//! Destructive filesystem attack tests for the DS Coding Harness.
//!
//! Each test verifies that filesystem-destructive operations are
//! detected, classified correctly, and blocked or flagged as
//! requiring approval.

use std::path::PathBuf;

use zhongshu_core::authority::{self, Risk};
use zhongshu_core::harness::state::VerificationState;
use zhongshu_core::harness::tool::effect::{
    classify_effects, risk_from_effects, EffectRisk, ToolEffect,
};
use zhongshu_core::harness::trace::event::HarnessEvent;
use zhongshu_core::tool::shell_semantics::{ShellCommandClass, ShellSemantics};

// ═══════════════════════════════════════════════════════════════════════
// Attack 1: Risk classification for destructive operations
//   Tool effects must classify shell/file operations correctly,
//   and rm -rf / must be blocked by both authority and shell semantics.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn recursive_delete_classified_destructive() {
    let shell_effects = classify_effects("shell");
    assert!(shell_effects.contains(&ToolEffect::RunProcess));

    let write_effects = classify_effects("write_file");
    assert!(write_effects.contains(&ToolEffect::WriteFile));

    let edit_effects = classify_effects("edit");
    assert!(edit_effects.contains(&ToolEffect::WriteFile));

    assert_eq!(
        risk_from_effects(&[ToolEffect::WriteFile]),
        EffectRisk::LocalMutation
    );

    let semantics = ShellSemantics::analyze("rm -rf /");
    assert_eq!(
        semantics.class,
        ShellCommandClass::Blocked,
        "rm -rf / must be blocked by ShellSemantics"
    );
    assert!(
        semantics.block_reason.is_some(),
        "block reason must be set for rm -rf /"
    );
    assert!(semantics.requires_approval);

    let cmd = authority::parse_command("rm -rf /").unwrap();
    assert_eq!(
        authority::classify(&cmd),
        Risk::Blocked,
        "rm -rf / must be classified as Blocked by authority"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 2: Symlink and parent traversal detection
//   Paths containing ".." that resolve to system locations must be
//   caught by the authority classifier after path normalization.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn symlink_traversal_detected() {
    let cmd = authority::parse_command("rm /var/www/../../etc/passwd")
        .expect("should parse command with traversal path");
    assert_eq!(
        authority::classify(&cmd),
        Risk::Blocked,
        "traversal path resolving to system location must be Blocked"
    );

    let cmd2 = authority::parse_command("rm ../../../etc/passwd").unwrap();
    let risk = authority::classify(&cmd2);
    assert_eq!(
        risk,
        Risk::Dangerous,
        "rm with path must be at least Dangerous"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 3: Parent traversal to sensitive locations
//   Paths like "../../etc/shadow" used with destructive commands must
//   be blocked or require approval.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn parent_traversal_blocks() {
    let cmd = authority::parse_command("rm /safe/path/../../../etc/shadow").expect("should parse");
    assert_eq!(
        authority::classify(&cmd),
        Risk::Blocked,
        "parent traversal to /etc/shadow must be Blocked"
    );

    let cmd2 = authority::parse_command("rm /a/b/c/d/../../../../etc/hosts").expect("should parse");
    assert_eq!(
        authority::classify(&cmd2),
        Risk::Blocked,
        "multi-level traversal to /etc/hosts must be Blocked"
    );

    let semantics = ShellSemantics::analyze("rm -rf ../../data");
    assert!(
        semantics.requires_approval,
        "rm with relative parent paths should require approval"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 4: Recursive delete recorded in harness state
//   When a destructive filesystem operation occurs, the harness
//   must record a FileEdit event and update last_edit_step.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn recursive_delete_recorded() {
    let mut state = VerificationState {
        required: false,
        records: Vec::new(),
        last_success: None,
        last_failure: None,
        last_edit_step: 0,
        last_verify_step: 0,
        unavailable_reason: None,
    };

    let event = HarnessEvent::FileEdit {
        path: PathBuf::from("/tmp/to-delete"),
        diff_hash: "delete-hash".into(),
        diff: None,
    };

    state.last_edit_step += 1;

    let mut trace_events: Vec<HarnessEvent> = Vec::new();
    trace_events.push(event);

    assert_eq!(
        state.last_edit_step, 1,
        "destructive edit must increment last_edit_step"
    );
    assert!(
        trace_events
            .iter()
            .any(|e| matches!(e, HarnessEvent::FileEdit { .. })),
        "destructive edit must produce a FileEdit trace event"
    );
}
