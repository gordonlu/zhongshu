use crate::harness::recovery::patch_history::PatchHistory;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

/// Cross-turn memory for all harness checkers.
#[derive(Clone)]
pub struct HarnessState {
    pub phase: CodingPhase,
    /// Phase before the most recent post-tool inference, used by pre_turn
    /// to detect phase transitions.
    pub previous_phase: CodingPhase,
    pub verification: VerificationState,
    pub tool_loop: ToolLoopState,
    pub recovery: RecoveryState,
    pub architecture: ArchitectureState,
    pub trace: TraceState,
}

impl HarnessState {
    pub fn new() -> Self {
        HarnessState {
            phase: CodingPhase::Understand,
            previous_phase: CodingPhase::Understand,
            verification: VerificationState {
                required: false,
                records: Vec::new(),
                last_success: None,
                last_failure: None,
                last_edit_step: 0,
                last_verify_step: 0,
                unavailable_reason: None,
            },
            tool_loop: ToolLoopState {
                recent_calls: VecDeque::new(),
                counts: HashMap::new(),
            },
            recovery: RecoveryState {
                failures: Vec::new(),
                last_feedback_step: 0,
                consecutive_no_progress: 0,
                patch_history: PatchHistory::new(),
            },
            architecture: ArchitectureState {
                violations: Vec::new(),
                emitted_hint_ids: Vec::new(),
                index: None,
            },
            trace: TraceState {
                events: Vec::new(),
                trace_file: None,
            },
        }
    }
}

// ── Phase ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingPhase {
    Understand,
    Inspect,
    Plan,
    Edit,
    Verify,
    Repair,
    Summarize,
}

// ── Verification ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VerificationRecord {
    pub command: String,
    pub command_hash: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub step: u32,
}

#[derive(Clone)]
pub struct VerificationState {
    pub required: bool,
    pub records: Vec<VerificationRecord>,
    pub last_success: Option<VerificationRecord>,
    pub last_failure: Option<VerificationRecord>,
    pub last_edit_step: u32,
    pub last_verify_step: u32,
    pub unavailable_reason: Option<String>,
}

// ── Tool Loop ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ToolCallFingerprint {
    pub tool_name: String,
    pub args_hash: String,
}

#[derive(Clone)]
pub struct ToolLoopState {
    pub recent_calls: VecDeque<ToolCallFingerprint>,
    pub counts: HashMap<ToolCallFingerprint, u32>,
}

// ── Recovery ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FailureSignature {
    pub command_hash: String,
    pub error_fingerprint: String,
    pub count: u32,
    pub first_seen_step: u32,
}

#[derive(Clone)]
pub struct RecoveryState {
    pub failures: Vec<FailureSignature>,
    pub last_feedback_step: u32,
    pub consecutive_no_progress: u32,
    pub patch_history: PatchHistory,
}

// ── Architecture ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViolationKey {
    pub rule_id: String,
    pub file_path: PathBuf,
    pub symbol_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationStatus {
    Open,
    Acknowledged,
    Resolved,
    Dismissed,
}

#[derive(Clone)]
pub struct OpenViolation {
    pub key: ViolationKey,
    pub status: ViolationStatus,
    pub severity: crate::harness::action::Severity,
    pub confidence: crate::harness::action::Confidence,
    pub message: String,
    pub introduced_this_run: bool,
    pub raised_step: u32,
}

#[derive(Clone)]
pub struct ArchitectureState {
    pub violations: Vec<OpenViolation>,
    pub emitted_hint_ids: Vec<String>,
    /// Project index for AST-based rule evaluation. Built lazily on first
    /// mutation in coding mode, then updated incrementally via update_file().
    pub index: Option<crate::harness::architecture::index::ProjectIndex>,
}

// ── Trace ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct TraceState {
    pub events: Vec<crate::harness::trace::event::HarnessEvent>,
    pub trace_file: Option<std::path::PathBuf>,
}
