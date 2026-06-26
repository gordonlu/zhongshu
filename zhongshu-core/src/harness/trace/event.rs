use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum HarnessEvent {
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
