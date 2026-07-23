use crate::agent::run::RunState;
use crate::runtime::ExecutionProfile;

/// UI-facing state, projected from canonical `RunStatus`.
/// Must NOT be used as business truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAgentState {
    Idle,
    Thinking,
    Streaming,
    Executing,
    WaitingApproval,
    Paused,
    Done { success: bool },
}

impl From<RunStatus> for UiAgentState {
    fn from(s: RunStatus) -> Self {
        match s {
            RunStatus::Created => UiAgentState::Idle,
            RunStatus::Running => UiAgentState::Thinking,
            RunStatus::WaitingApproval => UiAgentState::WaitingApproval,
            RunStatus::Paused => UiAgentState::Paused,
            RunStatus::Recovering => UiAgentState::Paused,
            RunStatus::Completed => UiAgentState::Done { success: true },
            RunStatus::Failed | RunStatus::Cancelled | RunStatus::UnknownOutcome => {
                UiAgentState::Done { success: false }
            }
        }
    }
}

/// Canonical status for a run attempt.
///
/// This is the single source of truth — UI state and task status
/// are derived projections of this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Run identity allocated but not yet started.
    Created,
    /// Actively executing (LLM call, tool execution, streaming).
    Running,
    /// Waiting for user approval on an action.
    WaitingApproval,
    /// Interrupted, waiting to be resumed (user interjection, pause).
    Paused,
    /// Being restored from checkpoint after a crash.
    Recovering,
    /// Completed successfully.
    Completed,
    /// Completed with a non-recoverable error.
    Failed,
    /// Cancelled by user or timeout before completion.
    Cancelled,
    /// Tool with side effects was cancelled/timeout — real outcome unknown.
    UnknownOutcome,
}

impl RunStatus {
    /// Best-effort mapping from the legacy `RunState` to canonical `RunStatus`.
    /// The mapping is lossy — `RunState::Finished` bundles multiple terminal
    /// outcomes into one variant with a string discriminator.
    pub fn from_run_state(state: &RunState) -> Self {
        match state {
            RunState::Idle => RunStatus::Created,
            RunState::Thinking | RunState::Streaming | RunState::Resuming => RunStatus::Running,
            RunState::ToolExecuting { .. } => RunStatus::Running,
            RunState::WaitingApproval { .. } => RunStatus::WaitingApproval,
            RunState::Interrupted { .. } => RunStatus::Paused,
            RunState::Finished { stop_reason } => match stop_reason.as_str() {
                "finished" | "max_tool_calls" | "max_steps" => RunStatus::Completed,
                "failed" | "persistent_failure" => RunStatus::Failed,
                "cancelled" | "interrupted" => RunStatus::Cancelled,
                "unknown_effect" => RunStatus::UnknownOutcome,
                _ => RunStatus::Completed,
            },
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled | RunStatus::UnknownOutcome
        )
    }
}

/// Persistent record of a single run attempt.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: String,
    pub task_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub profile: ExecutionProfile,
    pub status: RunStatus,
    pub attempt_no: u32,
}

impl RunRecord {
    pub fn new(run_id: impl Into<String>, profile: ExecutionProfile) -> Self {
        Self {
            run_id: run_id.into(),
            task_id: None,
            parent_run_id: None,
            profile,
            status: RunStatus::Created,
            attempt_no: 0,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}
