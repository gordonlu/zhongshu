use std::path::PathBuf;

use crate::patch::PatchDiffPayload;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HarnessEvent {
    CodingSessionStarted {
        timestamp: u64,
        session_id: String,
        trace_id: String,
        repo_root: PathBuf,
        intent: String,
        model: String,
        source: String,
        deeplossless_conversation_id: Option<i64>,
        deeplossless_replay_execution_id: Option<String>,
    },
    CodingPlanCreated {
        session_id: String,
        step_count: usize,
        risk: String,
    },
    CodingStepStarted {
        session_id: String,
        step_id: String,
        kind: String,
        title: String,
    },
    CodingStepCompleted {
        session_id: String,
        step_id: String,
        status: String,
    },
    WorkerStarted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        owned_files: Vec<PathBuf>,
    },
    WorkerCompleted {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        success: bool,
        trace_event_count: usize,
    },
    WorkerConflict {
        session_id: Option<String>,
        worker: String,
        task_id: String,
        reason: String,
    },
    PatchPreview {
        session_id: Option<String>,
        path: PathBuf,
        operation: String,
        diff_summary: String,
        #[serde(default)]
        diff: Option<PatchDiffPayload>,
    },
    PatchApplied {
        session_id: Option<String>,
        path: PathBuf,
        operation: String,
        changed: bool,
    },
    ContextPressure {
        pressure_percent: u8,
        dropped_evidence: usize,
        dropped_recent: usize,
    },
    ReplayAvailable {
        conversation_id: Option<i64>,
        replay_execution_id: Option<String>,
    },
    CodingOutcomeRecorded {
        timestamp: u64,
        session_id: String,
        outcome: String,
    },
    RunStarted {
        timestamp: u64,
        input: String,
        mode: String,
    },
    ContextIncluded {
        description: String,
        estimated_tokens: usize,
    },
    ToolCall {
        step: u32,
        tool_name: String,
        args_hash: String,
        success: bool,
    },
    FileRead {
        path: PathBuf,
    },
    FileEdit {
        path: PathBuf,
        diff_hash: String,
        diff: Option<String>,
    },
    Verification {
        command: String,
        success: bool,
        exit_code: Option<i32>,
        step: u32,
    },
    ArchitectureViolation {
        rule_id: String,
        severity: String,
    },
    RecoveryFeedback {
        rule_id: String,
        message: String,
    },
    PhaseTransition {
        from: String,
        to: String,
    },
    FinalClaim {
        text: String,
    },
    RunCompleted {
        timestamp: u64,
        total_steps: u32,
        outcome: String,
    },
}

#[allow(dead_code)]
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
