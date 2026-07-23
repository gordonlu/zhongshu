use serde::{Deserialize, Serialize};

use crate::tool::{ToolOutput, ToolTermination};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionStatus {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    UnknownOutcome,
}

#[derive(Debug, Clone)]
pub struct ActionRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: String,
    pub step: u32,
    pub tool_calls_made: usize,
}

impl ActionRequest {
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: impl Into<String>,
        step: u32,
        tool_calls_made: usize,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            arguments: arguments.into(),
            step,
            tool_calls_made,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActionResult {
    pub status: ActionStatus,
    pub observation: String,
    pub tool_calls_made: usize,
    pub tool_termination: ToolTermination,
    pub output_status: crate::tool::ToolStatus,
    pub output_error: Option<String>,
    pub output_request_id: Option<String>,
    pub tool_output: Option<ToolOutput>,
    pub was_idempotent_skip: bool,
}
