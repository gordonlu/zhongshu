/// How a cancel request should be processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    /// Signal the current attempt to stop. In-flight tools complete
    /// their current wait/cancel check; side-effecting tools produce
    /// `UnknownOutcome` instead of assuming no-op.
    Graceful,

    /// Graceful cancel first, then force-abort the runtime task if
    /// it does not respond within the configured timeout.
    ForceAfterTimeout { timeout_ms: u64 },
}

impl Default for CancelMode {
    fn default() -> Self {
        CancelMode::Graceful
    }
}

/// Outcome of a cancel request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Cancel signal delivered.
    Accepted,
    /// No active run to cancel.
    NoActiveRun,
}
