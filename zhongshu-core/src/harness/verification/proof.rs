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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFixtureFile {
    pub schema_version: u32,
    pub capability_case_id: String,
    pub fixture_id: String,
    pub deeplossless_conversation_id: Option<i64>,
    pub deeplossless_replay_execution_id: Option<String>,
    pub normalization: ReplayNormalizationPolicy,
    pub expected_tool_calls: Vec<String>,
    pub expected_changed_files: Vec<String>,
    pub expected_final_outcome: String,
    pub events: Vec<HarnessEvent>,
}

const FIXTURE_SCHEMA_VERSION: u32 = 1;

fn fixture_file_path(case_id: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR parent (workspace root)")
        .join("fixtures")
        .join("capability")
        .join(case_id)
        .join("replay.json")
}

fn load_fixture_file(case_id: &str) -> Option<ReplayFixtureFile> {
    let path = fixture_file_path(case_id);
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_fixture_file(case_id: &str, fixture: &ReplayFixtureFile) -> Result<(), String> {
    let path = fixture_file_path(case_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create fixture dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(fixture)
        .map_err(|e| format!("failed to serialize fixture: {e}"))?;
    std::fs::write(&path, content)
        .map_err(|e| format!("failed to write fixture: {e}"))?;
    Ok(())
}

fn recording_fixtures() -> bool {
    std::env::var("ZHONGSHU_RECORD_FIXTURES").is_ok()
}

fn fixtures_available(case_ids: &[&str]) -> bool {
    case_ids.iter().all(|id| fixture_file_path(id).exists())
}

fn fixture_file_to_metadata(f: &ReplayFixtureFile) -> ReplayFixtureMetadata {
    ReplayFixtureMetadata {
        id: f.fixture_id.clone(),
        deeplossless_conversation_id: f.deeplossless_conversation_id,
        deeplossless_replay_execution_id: f.deeplossless_replay_execution_id.clone(),
        normalization: f.normalization.clone(),
        expected_tool_calls: f.expected_tool_calls.clone(),
        expected_changed_files: f.expected_changed_files.clone(),
        expected_final_outcome: f.expected_final_outcome.clone(),
    }
}

fn fixture_file_to_evidence(f: &ReplayFixtureFile) -> ReplayFixtureEvidence {
    ReplayFixtureEvidence {
        id: f.fixture_id.clone(),
        deeplossless_conversation_id: f.deeplossless_conversation_id,
        deeplossless_replay_execution_id: f.deeplossless_replay_execution_id.clone(),
        normalization: f.normalization.clone(),
        events: f.events.clone(),
    }
}

fn inline_to_fixture_file(
    case_id: &str,
    evidence: &ReplayFixtureEvidence,
    expected: &ReplayFixtureMetadata,
) -> ReplayFixtureFile {
    ReplayFixtureFile {
        schema_version: FIXTURE_SCHEMA_VERSION,
        capability_case_id: case_id.to_string(),
        fixture_id: expected.id.clone(),
        deeplossless_conversation_id: evidence.deeplossless_conversation_id,
        deeplossless_replay_execution_id: evidence.deeplossless_replay_execution_id.clone(),
        normalization: evidence.normalization.clone(),
        expected_tool_calls: expected.expected_tool_calls.clone(),
        expected_changed_files: expected.expected_changed_files.clone(),
        expected_final_outcome: expected.expected_final_outcome.clone(),
        events: evidence.events.clone(),
    }
}

/// Controls where replay evidence is loaded from.
#[derive(Debug, Clone)]
pub enum ReplaySource {
    File,
    Deeplossless { base_url: String },
}

/// Load replay evidence from the specified source.
/// Returns None if the source is unavailable (file not found, deeplossless not reachable).
pub async fn load_replay_evidence(
    source: &ReplaySource,
    case_id: &str,
) -> Option<ReplayFixtureEvidence> {
    match source {
        ReplaySource::File => {
            let file = load_fixture_file(case_id)?;
            Some(fixture_file_to_evidence(&file))
        }
        ReplaySource::Deeplossless { base_url } => {
            load_deeplossless_evidence(base_url, case_id).await
        }
    }
}

async fn load_deeplossless_evidence(base_url: &str, case_id: &str) -> Option<ReplayFixtureEvidence> {
    let file = load_fixture_file(case_id)?;
    let exec_id = file.deeplossless_replay_execution_id.as_ref()?;

    let url = format!("{}/lcm/replay/{}", base_url.trim_end_matches('/'), exec_id);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let replay_json: serde_json::Value = resp.json().await.ok()?;

    let events = replay_to_harness_events(&replay_json, &file)?;

    Some(ReplayFixtureEvidence {
        id: file.fixture_id.clone(),
        deeplossless_conversation_id: file.deeplossless_conversation_id,
        deeplossless_replay_execution_id: file.deeplossless_replay_execution_id.clone(),
        normalization: file.normalization.clone(),
        events,
    })
}

/// Convert a deeplossless replay API response into HarnessEvents.
///
/// The replay JSON is expected to have the format returned by
/// `GET /v1/lcm/replay/{execution_id}`:
/// ```json
/// { "execution_id": 123, "events": [{ "seq_no": 1, "event": { "type": "tool_call_start", ... } }] }
/// ```
///
/// StreamEvent types used for conversion:
/// - `tool_call_start` → `HarnessEvent::ToolCall`
/// - `output_item_done` / `function_call_arguments_done` → `HarnessEvent::Verification` (when tool name/args match verification patterns)
/// - `done` → `HarnessEvent::RunCompleted`
/// - `error` → `HarnessEvent::RunCompleted` with error outcome
///
/// The returned events always start with a `CodingSessionStarted` derived from the fixture file metadata.
/// File edit events are not emitted (deeplossless diff data requires a separate `/v1/lcm/sessions/{id}/patches` fetch).
pub fn replay_to_harness_events(
    replay_json: &serde_json::Value,
    file: &ReplayFixtureFile,
) -> Option<Vec<HarnessEvent>> {
    let raw_events = replay_json["events"].as_array()?;
    let mut events = Vec::new();
    let mut step_counter = 0u32;
    let mut saw_error = false;

    events.push(HarnessEvent::CodingSessionStarted {
        timestamp: 0,
        session_id: format!("deeplossless-{}", file.capability_case_id),
        trace_id: format!("trace-{}", file.capability_case_id),
        repo_root: PathBuf::from("/workspace/replay"),
        intent: "deeplossless replay validation".into(),
        model: "unknown".into(),
        source: "deeplossless-replay".into(),
        deeplossless_conversation_id: file.deeplossless_conversation_id,
        deeplossless_replay_execution_id: file.deeplossless_replay_execution_id.clone(),
    });

    for raw in raw_events {
        let stream_event = &raw["event"];
        let event_type = stream_event["type"].as_str()?;

        match event_type {
            "tool_call_start" => {
                step_counter += 1;
                let tool_name = stream_event["name"].as_str().unwrap_or("unknown");
                events.push(HarnessEvent::ToolCall {
                    step: step_counter,
                    tool_name: tool_name.into(),
                    args_hash: format!("dl-{step_counter}"),
                    success: true,
                });
            }
            "output_item_done" | "function_call_arguments_done" => {
                let name = stream_event["name"].as_str().unwrap_or("");
                let arguments = stream_event["arguments"].as_str().unwrap_or("{}");
                let is_error = stream_event
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if is_error {
                    saw_error = true;
                }

                if name.contains("test") || name.contains("verify") || arguments.contains("self_test") || arguments.contains("test") {
                    let has_error_marker = name.contains("fail")
                        || arguments.contains("fail")
                        || arguments.contains("exit_code")
                        && arguments.contains("101");
                    events.push(HarnessEvent::Verification {
                        command: format!("deeplossless:{name}"),
                        success: !is_error && !has_error_marker,
                        exit_code: if is_error || has_error_marker {
                            Some(1)
                        } else {
                            Some(0)
                        },
                        step: step_counter,
                    });

                }
            }
            "done" => {
                let finish_reason = stream_event["finish_reason"].as_str().unwrap_or("completed");
                let incomplete = stream_event["incomplete"].as_bool().unwrap_or(false);
                let outcome = if incomplete || saw_error {
                    "blocked"
                } else {
                    match finish_reason {
                        "completed" | "end_turn" | "stop" => "completed",
                        _ => "blocked",
                    }
                };
                events.push(HarnessEvent::RunCompleted {
                    timestamp: 0,
                    total_steps: step_counter,
                    outcome: outcome.into(),
                });
            }
            "error" => {
                saw_error = true;
                events.push(HarnessEvent::RunCompleted {
                    timestamp: 0,
                    total_steps: step_counter,
                    outcome: format!(
                        "error: {}",
                        stream_event["message"].as_str().unwrap_or("unknown")
                    ),
                });
            }
            _ => {}
        }
    }

    if !events.iter().any(|e| matches!(e, HarnessEvent::RunCompleted { .. })) {
        events.push(HarnessEvent::RunCompleted {
            timestamp: 0,
            total_steps: step_counter,
            outcome: if saw_error { "blocked".into() } else { "completed".into() },
        });
    }

    Some(events)
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

fn first_wave_case_ids() -> [&'static str; 8] {
    [
        "failing-unit",
        "multi-file-api",
        "failure-recovery",
        "workspace-search-edit",
        "permission-artifact",
        "cross-module-refactor",
        "mcp-tool-extension",
        "worker-conflict",
    ]
}

pub fn first_wave_replay_fixtures() -> Vec<ReplayFixtureEvidence> {
    let case_ids = first_wave_case_ids();

    if recording_fixtures() {
        let inline = inline_first_wave_evidence();
        for (case_id, evidence) in case_ids.iter().zip(inline.iter()) {
            let case = default_capability_cases()
                .into_iter()
                .find(|c| c.id == *case_id)
                .expect("case exists");
            let file = inline_to_fixture_file(case_id, evidence, &case.replay);
            if let Err(e) = save_fixture_file(case_id, &file) {
                eprintln!("failed to record fixture {case_id}: {e}");
            }
        }
        inline
    } else if fixtures_available(&case_ids) {
        case_ids
            .iter()
            .filter_map(|id| load_fixture_file(id))
            .map(|f| fixture_file_to_evidence(&f))
            .collect()
    } else {
        inline_first_wave_evidence()
    }
}

fn inline_first_wave_evidence() -> Vec<ReplayFixtureEvidence> {
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
        replay_fixture(
            "replay-workspace-search-edit",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-workspace-search-edit".into(),
                    trace_id: "trace-workspace-search-edit".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/workspace-search-edit"),
                    intent: "Find the right implementation before making the edit.".into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(2001),
                    deeplossless_replay_execution_id: Some("replay-workspace-search-edit".into()),
                },
                HarnessEvent::ToolCall {
                    step: 1,
                    tool_name: "grep".into(),
                    args_hash: "find-target-file".into(),
                    success: true,
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/catalog.rs"),
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/catalog.rs"),
                    diff_hash: "catalog-fix".into(),
                    diff: None,
                },
                HarnessEvent::Verification {
                    command: "cargo test catalog".into(),
                    success: true,
                    exit_code: Some(0),
                    step: 2,
                },
                HarnessEvent::RunCompleted {
                    timestamp: 2,
                    total_steps: 2,
                    outcome: "completed".into(),
                },
            ],
        ),
        replay_fixture(
            "replay-permission-artifact",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-permission-artifact".into(),
                    trace_id: "trace-permission-artifact".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/permission-artifact"),
                    intent: "Request approval before performing the protected operation.".into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(2002),
                    deeplossless_replay_execution_id: Some("replay-permission-artifact".into()),
                },
                HarnessEvent::ArchitectureViolation {
                    rule_id: "requires_approval".into(),
                    severity: "warning".into(),
                },
                HarnessEvent::ToolCall {
                    step: 2,
                    tool_name: "shell".into(),
                    args_hash: "approved-artifact-cmd".into(),
                    success: true,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/artifact.rs"),
                    diff_hash: "artifact-patch".into(),
                    diff: None,
                },
                HarnessEvent::Verification {
                    command: "cargo test artifact".into(),
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
            "replay-cross-module-refactor",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-cross-module-refactor".into(),
                    trace_id: "trace-cross-module-refactor".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/cross-module-refactor"),
                    intent: "Refactor across owned modules without touching unrelated files."
                        .into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(2003),
                    deeplossless_replay_execution_id: Some("replay-cross-module-refactor".into()),
                },
                HarnessEvent::ToolCall {
                    step: 1,
                    tool_name: "grep".into(),
                    args_hash: "find-cross-module-symbols".into(),
                    success: true,
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/config.rs"),
                },
                HarnessEvent::FileRead {
                    path: PathBuf::from("src/runner.rs"),
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/config.rs"),
                    diff_hash: "config-refactor".into(),
                    diff: None,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/runner.rs"),
                    diff_hash: "runner-refactor".into(),
                    diff: None,
                },
                HarnessEvent::ToolCall {
                    step: 3,
                    tool_name: "self_test".into(),
                    args_hash: "cross-module-verify".into(),
                    success: true,
                },
                HarnessEvent::Verification {
                    command: "cargo test refactor".into(),
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
            "replay-mcp-tool-extension",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-mcp-tool-extension".into(),
                    trace_id: "trace-mcp-tool-extension".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/mcp-tool-extension"),
                    intent: "Enable a manifest-declared MCP tool and call it through ToolSpec."
                        .into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(2004),
                    deeplossless_replay_execution_id: Some("replay-mcp-tool-extension".into()),
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("equipment/manifest.json"),
                    diff_hash: "mcp-manifest".into(),
                    diff: None,
                },
                HarnessEvent::ToolCall {
                    step: 2,
                    tool_name: "mcp:test".into(),
                    args_hash: "mcp-tool-call".into(),
                    success: true,
                },
                HarnessEvent::Verification {
                    command: "cargo test mcp".into(),
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
            "replay-worker-conflict",
            vec![
                HarnessEvent::CodingSessionStarted {
                    timestamp: 1,
                    session_id: "session-worker-conflict".into(),
                    trace_id: "trace-worker-conflict".into(),
                    repo_root: PathBuf::from("/workspace/fixtures/capability/worker-conflict"),
                    intent: "Run worker ownership checks and report conflict instead of merging."
                        .into(),
                    model: "scripted".into(),
                    source: "offline-scripted-provider".into(),
                    deeplossless_conversation_id: Some(2005),
                    deeplossless_replay_execution_id: Some("replay-worker-conflict".into()),
                },
                HarnessEvent::WorkerStarted {
                    session_id: Some("session-worker-conflict".into()),
                    worker: "worker-alpha".into(),
                    task_id: "task-overlap".into(),
                    owned_files: vec![PathBuf::from("src/shared.rs")],
                },
                HarnessEvent::WorkerConflict {
                    session_id: Some("session-worker-conflict".into()),
                    worker: "worker-alpha".into(),
                    task_id: "task-overlap".into(),
                    reason: "overlapping edit range in src/shared.rs with worker-beta".into(),
                },
                HarnessEvent::RecoveryFeedback {
                    rule_id: "worker_conflict".into(),
                    message: "resolve overlapping ownership for src/shared.rs".into(),
                },
                HarnessEvent::ToolCall {
                    step: 3,
                    tool_name: "self_test".into(),
                    args_hash: "worker-conflict-tool".into(),
                    success: true,
                },
                HarnessEvent::FileEdit {
                    path: PathBuf::from("src/shared.rs"),
                    diff_hash: "shared-fix".into(),
                    diff: None,
                },
                HarnessEvent::Verification {
                    command: "cargo test shared".into(),
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
    let case_ids = first_wave_case_ids();

    if fixtures_available(&case_ids) {
        case_ids
            .iter()
            .enumerate()
            .filter_map(|(i, case_id)| {
                let case = &cases[i];
                let file = load_fixture_file(case_id)?;
                let meta = fixture_file_to_metadata(&file);
                let evidence = fixture_file_to_evidence(&file);
                let mut proof = evaluate_capability_replay_inner(&meta, &evidence);
                proof.case_id = case.id.clone();
                proof.replay_fixture_id = file.fixture_id.clone();
                Some(proof)
            })
            .collect()
    } else {
        let inline = first_wave_replay_fixtures();
        cases.iter().zip(inline.iter()).map(|(case, evidence)| {
            evaluate_capability_replay(case, evidence)
        }).collect()
    }
}

fn evaluate_capability_replay_inner(
    expected: &ReplayFixtureMetadata,
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
    for expected_tool in &expected.expected_tool_calls {
        if !observed_tool_calls.iter().any(|tool| tool == expected_tool) {
            missing_evidence.push(format!("tool call '{expected_tool}'"));
        }
    }
    for expected_file in &expected.expected_changed_files {
        if !observed_changed_files.iter().any(|path| path == expected_file) {
            missing_evidence.push(format!("changed file '{expected_file}'"));
        }
    }
    if !final_outcome_matches(
        &expected.expected_final_outcome,
        observed_final_outcome.as_deref(),
    ) {
        missing_evidence.push(format!(
            "final outcome '{}'",
            expected.expected_final_outcome
        ));
    }
    if expected.id.contains("failure-recovery")
        && !has_failed_then_passing_verification(&evidence.events)
    {
        missing_evidence.push("failed verification followed by passing rerun".into());
    }

    CapabilityReplayProof {
        case_id: String::new(),
        replay_fixture_id: evidence.id.clone(),
        passed: missing_evidence.is_empty(),
        missing_evidence,
        observed_tool_calls,
        observed_changed_files,
        observed_final_outcome,
        normalized_evidence_hash: normalized_replay_hash(evidence),
    }
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

        assert_eq!(results.len(), 8, "all 8 capability cases pass expected assertions");
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

    #[test]
    fn fixture_files_are_valid_json() {
        let case_ids = first_wave_case_ids();
        assert_eq!(case_ids.len(), 8);
        for case_id in &case_ids {
            assert!(
                fixture_file_path(case_id).exists(),
                "fixture file should exist for {case_id}"
            );
        }
    }

    #[test]
    fn fixture_files_deserialize_with_schema_v1() {
        let case_ids = first_wave_case_ids();
        for case_id in &case_ids {
            let file = load_fixture_file(case_id)
                .unwrap_or_else(|| panic!("{case_id} fixture file must load"));
            assert_eq!(
                file.schema_version, 1,
                "{case_id} fixture uses expected schema version"
            );
            assert_eq!(
                file.capability_case_id, *case_id,
                "{case_id} fixture capability_case_id matches"
            );
            assert!(file.fixture_id.starts_with("replay-"));
            assert!(!file.events.is_empty(), "{case_id} fixture has events");
            assert!(
                file.expected_tool_calls.len() >= 1,
                "{case_id} fixture has expected tool calls"
            );
            assert!(
                file.expected_changed_files.len() >= 1,
                "{case_id} fixture has expected changed files"
            );
            assert!(!file.expected_final_outcome.is_empty());
            assert!(
                file.deeplossless_conversation_id.is_some()
                    || file.deeplossless_replay_execution_id.is_some()
            );

            let meta = fixture_file_to_metadata(&file);
            assert_eq!(meta.id, file.fixture_id);

            let evidence = fixture_file_to_evidence(&file);
            assert_eq!(evidence.events.len(), file.events.len());
            assert_eq!(evidence.id, file.fixture_id);
        }
    }

    #[test]
    fn fixture_files_roundtrip_with_expected_assertions() {
        let case_ids = first_wave_case_ids();
        let cases = default_capability_cases();
        for case_id in &case_ids {
            let file = load_fixture_file(case_id)
                .unwrap_or_else(|| panic!("{case_id} fixture file must load"));
            let meta = fixture_file_to_metadata(&file);
            let evidence = fixture_file_to_evidence(&file);
            let case = cases.iter().find(|c| c.id == *case_id).unwrap();

            let proof = evaluate_capability_replay_inner(&meta, &evidence);
            assert!(
                proof.passed,
                "{case_id}: missing {:?}",
                proof.missing_evidence
            );
            assert!(!proof.normalized_evidence_hash.is_empty());

            let proof2 = evaluate_capability_replay(case, &evidence);
            assert!(
                proof2.passed,
                "{case_id}: case-based missing {:?}",
                proof2.missing_evidence
            );
        }
    }

    #[test]
    fn replay_to_harness_converts_tool_call_start() {
        let file = load_fixture_file("failing-unit").expect("fixture file exists");
        let replay_json = serde_json::json!({
            "execution_id": 1001,
            "events": [
                {
                    "seq_no": 1,
                    "event": { "type": "tool_call_start", "index": 0, "id": "call_1", "name": "self_test" }
                },
                {
                    "seq_no": 2,
                    "event": { "type": "output_item_done", "index": 0, "item_id": "item_1", "item_type": "tool_result", "name": "self_test", "arguments": "{}" }
                },
                {
                    "seq_no": 3,
                    "event": { "type": "done", "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }, "finish_reason": "completed", "incomplete": false }
                }
            ],
            "total": 3,
            "corrupt_count": 0
        });

        let events = replay_to_harness_events(&replay_json, &file).expect("convert");
        assert!(events.len() >= 3);

        assert!(matches!(events[0], HarnessEvent::CodingSessionStarted { .. }));

        assert!(matches!(&events[1], HarnessEvent::ToolCall { tool_name, .. } if tool_name == "self_test"));

        let has_completed = events.iter().any(|e| matches!(e, HarnessEvent::RunCompleted { outcome, .. } if outcome == "completed"));
        assert!(has_completed);
    }

    #[test]
    fn replay_to_harness_detects_error_outcome() {
        let file = load_fixture_file("failing-unit").expect("fixture file exists");
        let replay_json = serde_json::json!({
            "execution_id": 1001,
            "events": [
                {
                    "seq_no": 1,
                    "event": { "type": "tool_call_start", "index": 0, "id": "call_1", "name": "self_test" }
                },
                {
                    "seq_no": 2,
                    "event": { "type": "error", "message": "API rate limit exceeded", "code": "rate_limit" }
                }
            ],
            "total": 2,
            "corrupt_count": 0
        });

        let events = replay_to_harness_events(&replay_json, &file).expect("convert");
        let has_blocked = events.iter().any(|e| matches!(e, HarnessEvent::RunCompleted { outcome, .. } if outcome.starts_with("error:")));
        assert!(has_blocked);
    }

    #[test]
    fn replay_to_harness_detects_verification_failure() {
        let file = load_fixture_file("failing-unit").expect("fixture file exists");
        let replay_json = serde_json::json!({
            "execution_id": 1001,
            "events": [
                {
                    "seq_no": 1,
                    "event": { "type": "tool_call_start", "index": 0, "id": "call_1", "name": "self_test" }
                },
                {
                    "seq_no": 2,
                    "event": { "type": "output_item_done", "index": 0, "item_id": "item_1", "item_type": "tool_result", "name": "self_test", "arguments": "{\"command\": \"cargo test\", \"exit_code\": 101}" }
                },
                {
                    "seq_no": 3,
                    "event": { "type": "done", "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }, "finish_reason": "completed", "incomplete": false }
                }
            ],
            "total": 3,
            "corrupt_count": 0
        });

        let events = replay_to_harness_events(&replay_json, &file).expect("convert");
        let has_failed_verify = events.iter().any(|e| matches!(e, HarnessEvent::Verification { success: false, .. }));
        assert!(has_failed_verify);
    }

    #[test]
    fn replay_to_harness_empty_events_falls_back() {
        let file = load_fixture_file("failing-unit").expect("fixture file exists");
        let replay_json = serde_json::json!({
            "execution_id": 1001,
            "events": [],
            "total": 0,
            "corrupt_count": 0
        });

        let events = replay_to_harness_events(&replay_json, &file).expect("convert");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], HarnessEvent::CodingSessionStarted { .. }));
        assert!(matches!(&events[1], HarnessEvent::RunCompleted { outcome, .. } if outcome == "completed"));
    }

    #[test]
    fn replay_to_harness_injects_final_run_completed() {
        let file = load_fixture_file("failing-unit").expect("fixture file exists");
        let replay_json = serde_json::json!({
            "execution_id": 1001,
            "events": [
                {
                    "seq_no": 1,
                    "event": { "type": "tool_call_start", "index": 0, "id": "call_1", "name": "grep" }
                }
            ],
            "total": 1,
            "corrupt_count": 0
        });

        let events = replay_to_harness_events(&replay_json, &file).expect("convert");
        let has_completed = events.iter().any(|e| matches!(e, HarnessEvent::RunCompleted { .. }));
        assert!(has_completed);
    }

    #[test]
    fn replay_source_file_loads_evidence() {
        let source = ReplaySource::File;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let evidence = rt.block_on(load_replay_evidence(&source, "failing-unit"));
        assert!(evidence.is_some(), "file source should load failing-unit fixture");
        let evidence = evidence.unwrap();
        assert_eq!(evidence.id, "replay-failing-unit");
        assert!(!evidence.events.is_empty());
    }

    #[test]
    fn replay_source_deeplossless_returns_none_when_unavailable() {
        let source = ReplaySource::Deeplossless {
            base_url: "http://127.0.0.1:1".into(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let evidence = rt.block_on(load_replay_evidence(&source, "failing-unit"));
        assert!(evidence.is_none(), "deeplossless on port 1 should be unavailable");
    }

    #[test]
    fn replay_source_missing_case_returns_none() {
        let source = ReplaySource::File;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let evidence = rt.block_on(load_replay_evidence(&source, "nonexistent-case"));
        assert!(evidence.is_none());
    }
}
