/// Top-level action returned by harness checkers.
pub enum HarnessAction {
    None,
    InjectObservation(HarnessFeedback),
    BlockTool { feedback: HarnessFeedback },
    BlockFinalize { feedback: HarnessFeedback },
    Abort { reason: String },
}

/// Structured feedback from a harness checker.
pub struct HarnessFeedback {
    pub source: FeedbackSource,
    pub severity: Severity,
    pub rule_id: String,
    pub message: String,
    pub suggestion: String,
    pub evidence: Option<String>,
}

pub enum FeedbackSource {
    Architecture,
    Verification,
    Recovery,
    ToolLoop,
    Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Fatal,
    BlockTool,
}
