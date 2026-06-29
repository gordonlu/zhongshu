use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::harness::trace::event::HarnessEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofMode {
    Local,
    Pr,
    Baseline,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofCheckStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofCheckResult {
    pub id: String,
    pub title: String,
    pub status: ProofCheckStatus,
    pub command: Vec<String>,
    pub log_path: Option<String>,
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofReport {
    pub schema_version: u32,
    pub mode: ProofMode,
    pub checks: Vec<ProofCheckResult>,
}

impl ProofReport {
    pub fn summary(&self) -> ProofSummary {
        let mut summary = ProofSummary::default();
        for check in &self.checks {
            match check.status {
                ProofCheckStatus::Passed => summary.passed += 1,
                ProofCheckStatus::Failed => summary.failed += 1,
                ProofCheckStatus::Skipped => summary.skipped += 1,
            }
        }
        summary
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiProofChecklist {
    pub id: String,
    pub platform: String,
    pub mode: String,
    pub required_evidence: Vec<String>,
    pub manual_notes: Vec<String>,
}

pub fn ubuntu_chat_coding_manual_checklist() -> UiProofChecklist {
    UiProofChecklist {
        id: "ubuntu-gtk-chat-coding".into(),
        platform: "ubuntu".into(),
        mode: "manual".into(),
        required_evidence: vec![
            "GTK overlay opens and accepts chat input".into(),
            "coding mode can submit a task and stream assistant output".into(),
            "safe command execution completes without blocking the UI".into(),
            "stop/approval/settings actions do not deadlock event polling".into(),
        ],
        manual_notes: vec![
            "User reported Ubuntu command execution path has already run successfully.".into(),
            "Keep this as a replay/proof checklist so future UI/runtime changes can revalidate it."
                .into(),
        ],
    }
}

pub fn windows_webview2_smoke_checklist() -> UiProofChecklist {
    UiProofChecklist {
        id: "windows-webview2-chat-coding".into(),
        platform: "windows".into(),
        mode: "automated_or_manual".into(),
        required_evidence: vec![
            "WebView2 overlay opens or reports a visible startup error".into(),
            "submit IPC command reaches the overlay action queue".into(),
            "assistant delta and completion events serialize into window.handleIpc".into(),
            "coding cards render plan, worker, patch, verification, context, and replay events"
                .into(),
            "close hides the overlay instead of exiting the app".into(),
        ],
        manual_notes: vec![
            "Contract tests can run without creating a WebView2 window.".into(),
            "Full visual proof should attach a screenshot or window log from a Windows machine."
                .into(),
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayNormalizationPolicy {
    pub normalize_workspace_root: bool,
    pub normalize_home_dir: bool,
    pub normalize_timestamps: bool,
    pub normalize_uuids: bool,
    pub normalize_durations: bool,
    pub normalize_provider_request_ids: bool,
    pub normalize_token_and_cost_counters: bool,
}

impl Default for ReplayNormalizationPolicy {
    fn default() -> Self {
        Self {
            normalize_workspace_root: true,
            normalize_home_dir: true,
            normalize_timestamps: true,
            normalize_uuids: true,
            normalize_durations: true,
            normalize_provider_request_ids: true,
            normalize_token_and_cost_counters: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFixtureMetadata {
    pub id: String,
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
    pub normalization: ReplayNormalizationPolicy,
    pub expected_tool_calls: Vec<String>,
    pub expected_changed_files: Vec<String>,
    pub expected_final_outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFixtureEvidence {
    pub id: String,
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
    pub normalization: ReplayNormalizationPolicy,
    pub events: Vec<HarnessEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityReplayProof {
    pub case_id: String,
    pub replay_fixture_id: String,
    pub passed: bool,
    pub missing_evidence: Vec<String>,
    pub observed_tool_calls: Vec<String>,
    pub observed_changed_files: Vec<String>,
    pub observed_final_outcome: Option<String>,
    pub normalized_evidence_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityCase {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub fixture_workspace: String,
    pub required_evidence: Vec<String>,
    pub replay: ReplayFixtureMetadata,
}

pub fn default_capability_cases() -> Vec<CapabilityCase> {
    vec![
        capability_case(
            "failing-unit",
            "Fix one failing unit test without unrelated edits",
            "Fix the failing unit test and run the focused verification.",
            vec!["failed test", "bounded edit", "passing focused test"],
            vec!["self_test"],
            vec!["src/lib.rs"],
        ),
        capability_case(
            "multi-file-api",
            "Update a shared API and all known consumers",
            "Update the API call shape and fix every affected caller.",
            vec!["affected symbols", "consumer edits", "verification command"],
            vec!["grep", "self_test"],
            vec!["src/api.rs", "src/app.rs"],
        ),
        capability_case(
            "failure-recovery",
            "Recover after failed verification",
            "Run verification, inspect the failure, fix the cause, and rerun.",
            vec!["verification failure", "recovery action", "passing rerun"],
            vec!["self_test"],
            vec!["src/slug.rs"],
        ),
        capability_case(
            "workspace-search-edit",
            "Search before editing the target file",
            "Find the right implementation before making the edit.",
            vec!["search trace", "file read", "patch applied"],
            vec!["grep"],
            vec!["src/catalog.rs"],
        ),
        capability_case(
            "permission-artifact",
            "Surface approval for risky action",
            "Request approval before performing the protected operation.",
            vec![
                "pending auth",
                "approve or deny outcome",
                "no silent mutation",
            ],
            vec!["shell"],
            vec!["src/artifact.rs"],
        ),
        capability_case(
            "cross-module-refactor",
            "Bounded cross-module refactor",
            "Refactor across owned modules without touching unrelated files.",
            vec!["ownership map", "expected changed files", "verification"],
            vec!["grep", "self_test"],
            vec!["src/config.rs", "src/runner.rs"],
        ),
        capability_case(
            "mcp-tool-extension",
            "Register and call an MCP stdio tool safely",
            "Enable a manifest-declared MCP tool and call it through ToolSpec.",
            vec!["MCP preflight", "ToolSpec", "permission result"],
            vec!["mcp:test"],
            vec!["equipment/manifest.json"],
        ),
        capability_case(
            "worker-conflict",
            "Detect overlapping worker edits",
            "Run worker ownership checks and report conflict instead of merging.",
            vec!["file claim", "conflict evidence", "blocked merge"],
            vec!["self_test"],
            vec!["src/shared.rs"],
        ),
    ]
}

pub fn first_wave_replay_fixtures() -> Vec<ReplayFixtureEvidence> {
    vec![
        replay_fixture(
            "replay-failing-unit",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-failing-unit".into(),
                    trace_id: "trace-failing-unit".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/failing-unit"),
                    intent: "Fix the failing unit test and run the focused verification.".into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(1001),
                    deeplossless_replay_execution_id: Some("replay-failing-unit".into()),
                },
                HarnessEvent::Verification {
                    command: "cargo test failing_unit".into(),
                    success: false,
                    exit_code: Some(101),
                    step: 1,
                },
                HarnessEvent::ToolCall {
                    step: 2,
                    tool_name: "self_test".into(),
                    args_hash: "failing-unit-tool".into(),
                    success: true,
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/lib.rs"),
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/lib.rs"),
                    diff_hash: "fix-failing-unit".into(),
                    diff: None,
                },
                HarnessEvent::Verification {
                    command: "cargo test failing_unit".into(),
                    success: true,
                    exit_code: Some(0),
                    step: 3,
                },
                HarnessEvent::RunCompleted {
                    timestamp: 2,
                    total_steps: 3,
                    outcome: "completed".into(),
                },
            ],
        ),
        replay_fixture(
            "replay-multi-file-api",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-multi-file-api".into(),
                    trace_id: "trace-multi-file-api".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/multi-file-api"),
                    intent: "Update the API call shape and fix every affected caller.".into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(1002),
                    deeplossless_replay_execution_id: Some("replay-multi-file-api".into()),
                },
                HarnessEvent::ToolCall {
                    step: 1,
                    tool_name: "grep".into(),
                    args_hash: "find-api-callers".into(),
                    success: true,
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/api.rs"),
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/app.rs"),
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/api.rs"),
                    diff_hash: "api-signature".into(),
                    diff: None,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/app.rs"),
                    diff_hash: "api-consumer".into(),
                    diff: None,
                },
                HarnessEvent::ToolCall {
                    step: 3,
                    tool_name: "self_test".into(),
                    args_hash: "multi-file-api-verify".into(),
                    success: true,
                },
                HarnessEvent::Verification {
                    command: "cargo test api_contract".into(),
                    success: true,
                    exit_code: Some(0),
                    step: 4,
                },
                HarnessEvent::RunCompleted {
                    timestamp: 2,
                    total_steps: 4,
                    outcome: "completed".into(),
                },
            ],
        ),
        replay_fixture(
            "replay-failure-recovery",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-failure-recovery".into(),
                    trace_id: "trace-failure-recovery".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/failure-recovery"),
                    intent: "Run verification, inspect the failure, fix the cause, and rerun."
                        .into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(1003),
                    deeplossless_replay_execution_id: Some("replay-failure-recovery".into()),
                },
                HarnessEvent::Verification {
                    command: "cargo test slug_recovery".into(),
                    success: false,
                    exit_code: Some(101),
                    step: 1,
                },
                HarnessEvent::RecoveryFeedback {
                    rule_id: "verification_failed".into(),
                    message: "inspect failing assertion and patch slug normalization".into(),
                },
                HarnessEvent::ToolCall {
                    step: 2,
                    tool_name: "self_test".into(),
                    args_hash: "failure-recovery-tool".into(),
                    success: true,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/slug.rs"),
                    diff_hash: "slug-fix".into(),
                    diff: None,
                },
                HarnessEvent::Verification {
                    command: "cargo test slug_recovery".into(),
                    success: true,
                    exit_code: Some(0),
                    step: 3,
                },
                HarnessEvent::RunCompleted {
                    timestamp: 2,
                    total_steps: 3,
                    outcome: "completed".into(),
                },
            ],
        ),
    ]
}

pub fn evaluate_capability_replay(
    case: &CapabilityCase,
    evidence: &ReplayFixtureEvidence,
) -> CapabilityReplayProof {
    let observed_tool_calls = observed_tool_calls(&evidence.events);
    let observed_changed_files = observed_changed_files(&evidence.events);
    let observed_final_outcome = observed_final_outcome(&evidence.events);
    let mut missing_evidence = Vec::new();

    if evidence.deeplossless_conversation_id.is_none()
        && evidence.deeplossless_replay_execution_id.is_none()
    {
        missing_evidence.push("deeplossless conversation or replay execution id".into());
    }
    for expected in &case.replay.expected_tool_calls {
        if !observed_tool_calls.iter().any(|tool| tool == expected) {
            missing_evidence.push(format!("tool call '{expected}'"));
        }
    }
    for expected in &case.replay.expected_changed_files {
        if !observed_changed_files.iter().any(|path| path == expected) {
            missing_evidence.push(format!("changed file '{expected}'"));
        }
    }
    if !final_outcome_matches(
        &case.replay.expected_final_outcome,
        observed_final_outcome.as_deref(),
    ) {
        missing_evidence.push(format!(
            "final outcome '{}'",
            case.replay.expected_final_outcome
        ));
    }
    if case.id == "failure-recovery" && !has_failed_then_passing_verification(&evidence.events) {
        missing_evidence.push("failed verification followed by passing rerun".into());
    }

    CapabilityReplayProof {
        case_id: case.id.clone(),
        replay_fixture_id: evidence.id.clone(),
        passed: missing_evidence.is_empty(),
        missing_evidence,
        observed_tool_calls,
        observed_changed_files,
        observed_final_outcome,
        normalized_evidence_hash: normalized_replay_hash(evidence),
    }
}

pub fn evaluate_first_wave_replay_fixtures() -> Vec<CapabilityReplayProof> {
    let cases = default_capability_cases();
    let fixtures = first_wave_replay_fixtures();
    cases
        .iter()
        .take(3)
        .zip(fixtures.iter())
        .map(|(case, evidence)| evaluate_capability_replay(case, evidence))
        .collect()
}

fn capability_case(
    id: &str,
    title: &str,
    prompt: &str,
    required_evidence: Vec<&str>,
    expected_tool_calls: Vec<&str>,
    expected_changed_files: Vec<&str>,
) -> CapabilityCase {
    CapabilityCase {
        id: id.into(),
        title: title.into(),
        prompt: prompt.into(),
        fixture_workspace: format!("fixtures/capability/{id}"),
        required_evidence: required_evidence.into_iter().map(str::to_string).collect(),
        replay: ReplayFixtureMetadata {
            id: format!("replay-{id}"),
            deeplossless_conversation_id: None,
            deeplossless_replay_execution_id: None,
            normalization: ReplayNormalizationPolicy::default(),
            expected_tool_calls: expected_tool_calls
                .into_iter()
                .map(str::to_string)
                .collect(),
            expected_changed_files: expected_changed_files
                .into_iter()
                .map(str::to_string)
                .collect(),
            expected_final_outcome: "completed_or_blocked_with_reason".into(),
        },
    }
}

fn replay_fixture(id: &str, events: Vec<HarnessEvent>) -> ReplayFixtureEvidence {
    let (conversation_id, replay_execution_id) = events
        .iter()
        .find_map(|event| match event {
            HarnessEvent::CodingSessionStarted {
                deeplossless_conversation_id,
                deeplossless_replay_execution_id,
                ..
            } => Some((
                *deeplossless_conversation_id,
                deeplossless_replay_execution_id.clone(),
            )),
            HarnessEvent::ReplayAvailable {
                conversation_id,
                replay_execution_id,
            } => Some((*conversation_id, replay_execution_id.clone())),
            _ => None,
        })
        .unwrap_or((None, None));

    ReplayFixtureEvidence {
        id: id.into(),
        deeplossless_conversation_id: conversation_id,
        deeplossless_replay_execution_id: replay_execution_id,
        normalization: ReplayNormalizationPolicy::default(),
        events,
    }
}

fn observed_tool_calls(events: &[HarnessEvent]) -> Vec<String> {
    let mut tools = BTreeSet::new();
    for event in events {
        if let HarnessEvent::ToolCall { tool_name, .. } = event {
            tools.insert(tool_name.clone());
        }
    }
    tools.into_iter().collect()
}

fn observed_changed_files(events: &[HarnessEvent]) -> Vec<String> {
    let mut files = BTreeSet::new();
    for event in events {
        match event {
            HarnessEvent::FileEdit { path, .. }
            | HarnessEvent::PatchPreview { path, .. }
            | HarnessEvent::PatchApplied { path, .. } => {
                files.insert(normalize_path(path));
            }
            _ => {}
        }
    }
    files.into_iter().collect()
}

fn observed_final_outcome(events: &[HarnessEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        HarnessEvent::RunCompleted { outcome, .. }
        | HarnessEvent::CodingOutcomeRecorded { outcome, .. } => Some(outcome.clone()),
        _ => None,
    })
}

fn final_outcome_matches(expected: &str, observed: Option<&str>) -> bool {
    match expected {
        "completed_or_blocked_with_reason" => matches!(observed, Some("completed" | "blocked")),
        other => observed == Some(other),
    }
}

fn has_failed_then_passing_verification(events: &[HarnessEvent]) -> bool {
    let mut saw_failure = false;
    for event in events {
        if let HarnessEvent::Verification { success, .. } = event {
            if !success {
                saw_failure = true;
            } else if saw_failure {
                return true;
            }
        }
    }
    false
}

fn normalized_replay_hash(evidence: &ReplayFixtureEvidence) -> String {
    let mut lines = Vec::new();
    lines.push(format!("fixture={}", evidence.id));
    lines.push(format!(
        "conversation={:?}",
        evidence.deeplossless_conversation_id
    ));
    lines.push(format!(
        "replay={:?}",
        evidence.deeplossless_replay_execution_id
    ));
    for event in &evidence.events {
        lines.push(normalized_event_line(event));
    }
    compute_stdout_hash(&lines.join("\n"))
}

fn normalized_event_line(event: &HarnessEvent) -> String {
    match event {
        HarnessEvent::CodingSessionStarted {
            intent,
            model,
            source,
            deeplossless_conversation_id,
            deeplossless_replay_execution_id,
            ..
        } => format!(
            "session|intent={intent}|model={model}|source={source}|conversation={deeplossless_conversation_id:?}|replay={deeplossless_replay_execution_id:?}"
        ),
        HarnessEvent::ToolCall {
            tool_name, success, ..
        } => format!("tool|{tool_name}|success={success}"),
        HarnessEvent::FileRead { path } => format!("read|{}", normalize_path(path)),
        HarnessEvent::FileEdit { path, diff_hash, .. } => {
            format!("edit|{}|{diff_hash}", normalize_path(path))
        }
        HarnessEvent::PatchPreview { path, operation, diff_summary, .. } => {
            format!("patch_preview|{}|{operation}|{diff_summary}", normalize_path(path))
        }
        HarnessEvent::PatchApplied { path, operation, changed, .. } => {
            format!("patch_applied|{}|{operation}|changed={changed}", normalize_path(path))
        }
        HarnessEvent::Verification {
            command,
            success,
            exit_code,
            ..
        } => format!("verification|{command}|success={success}|exit={exit_code:?}"),
        HarnessEvent::RecoveryFeedback { rule_id, message } => {
            format!("recovery|{rule_id}|{message}")
        }
        HarnessEvent::RunCompleted {
            total_steps,
            outcome,
            ..
        } => format!("completed|steps={total_steps}|outcome={outcome}"),
        other => format!("{other:?}"),
    }
}

fn normalize_path(path: &PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn compute_stdout_hash(stdout: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(normalize_stdout(stdout).as_bytes());
    hex::encode(hasher.finalize())
}

fn normalize_stdout(stdout: &str) -> String {
    stdout
        .replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_summary_counts_statuses() {
        let report = ProofReport {
            schema_version: 1,
            mode: ProofMode::Local,
            checks: vec![
                ProofCheckResult {
                    id: "fmt".into(),
                    title: "Format".into(),
                    status: ProofCheckStatus::Passed,
                    command: vec!["cargo".into(), "fmt".into(), "--check".into()],
                    log_path: None,
                    skip_reason: None,
                },
                ProofCheckResult {
                    id: "ui".into(),
                    title: "UI".into(),
                    status: ProofCheckStatus::Skipped,
                    command: Vec::new(),
                    log_path: None,
                    skip_reason: Some("manual evidence required".into()),
                },
            ],
        };

        assert_eq!(
            report.summary(),
            ProofSummary {
                passed: 1,
                failed: 0,
                skipped: 1
            }
        );
    }

    #[test]
    fn capability_cases_cover_first_wave() {
        let ids: Vec<_> = default_capability_cases()
            .into_iter()
            .map(|case| case.id)
            .collect();

        assert_eq!(
            ids,
            vec![
                "failing-unit",
                "multi-file-api",
                "failure-recovery",
                "workspace-search-edit",
                "permission-artifact",
                "cross-module-refactor",
                "mcp-tool-extension",
                "worker-conflict",
            ]
        );
    }

    #[test]
    fn replay_normalization_defaults_are_stable() {
        let policy = ReplayNormalizationPolicy::default();
        assert!(policy.normalize_workspace_root);
        assert!(policy.normalize_home_dir);
        assert!(policy.normalize_timestamps);
        assert!(policy.normalize_uuids);
        assert!(policy.normalize_provider_request_ids);
    }

    #[test]
    fn first_wave_replay_fixtures_pass_expected_assertions() {
        let results = evaluate_first_wave_replay_fixtures();

        assert_eq!(results.len(), 3);
        for result in results {
            assert!(
                result.passed,
                "{} missing {:?}",
                result.case_id, result.missing_evidence
            );
            assert!(!result.normalized_evidence_hash.is_empty());
        }
    }

    #[test]
    fn replay_fixture_requires_deeplossless_anchor() {
        let case = default_capability_cases().remove(0);
        let evidence = ReplayFixtureEvidence {
            id: "missing-anchor".into(),
            deeplossless_conversation_id: None,
            deeplossless_replay_execution_id: None,
            normalization: ReplayNormalizationPolicy::default(),
            events: vec![
                HarnessEvent::ToolCall {
                    step: 1,
                    tool_name: "self_test".into(),
                    args_hash: "x".into(),
                    success: true,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/lib.rs"),
                    diff_hash: "x".into(),
                    diff: None,
                },
                HarnessEvent::RunCompleted {
                    timestamp: 1,
                    total_steps: 1,
                    outcome: "completed".into(),
                },
            ],
        };

        let result = evaluate_capability_replay(&case, &evidence);

        assert!(!result.passed);
        assert!(result
            .missing_evidence
            .iter()
            .any(|item| item.contains("deeplossless")));
    }

    #[test]
    fn ui_checklists_pin_platform_evidence() {
        let ubuntu = ubuntu_chat_coding_manual_checklist();
        assert_eq!(ubuntu.platform, "ubuntu");
        assert!(ubuntu
            .required_evidence
            .iter()
            .any(|item| item.contains("command execution")));

        let windows = windows_webview2_smoke_checklist();
        assert_eq!(windows.platform, "windows");
        assert!(windows
            .required_evidence
            .iter()
            .any(|item| item.contains("WebView2")));
    }

    #[test]
    fn stdout_hash_normalizes_line_endings() {
        assert_eq!(
            compute_stdout_hash("a\r\nb\r\n"),
            compute_stdout_hash("a\nb\n")
        );
    }
}
