/// Controls reliability and checkpoint semantics for a run attempt.
///
/// ExecutionProfile is not about *what* the agent does — it's about
/// *how* the runtime treats state, recovery, and external-side-effect
/// guarantees for this particular attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionProfile {
    /// Quick ephemeral session. No checkpoint, minimal journal.
    /// Suitable for chat, single reads, short tasks with user present.
    Interactive,

    /// Session that may be interrupted and resumed within the same
    /// process lifetime. Saves checkpoint before each side-effecting
    /// tool. Default for interactive tasks with history.
    Resumable,

    /// Full durability. Journal is append-only and survives process
    /// restart. Checkpoint saved before every tool. Suitable for
    /// unattended / background tasks and external-side-effect ops.
    Durable,

    /// Child agent run managed by a parent (orchestrator or task).
    /// Does not manage its own recovery — parent handles retry.
    Worker,
}

impl Default for ExecutionProfile {
    fn default() -> Self {
        ExecutionProfile::Resumable
    }
}

impl ExecutionProfile {
    /// Whether the runtime should save a full checkpoint before each tool call.
    ///
    /// - `Interactive` → false (fast path, no serialization overhead)
    /// - `Resumable`   → true  (crash recovery within session)
    /// - `Durable`     → true  (cross-process crash recovery)
    /// - `Worker`      → false (parent manages retry)
    pub fn saves_checkpoint(&self) -> bool {
        matches!(self, ExecutionProfile::Resumable | ExecutionProfile::Durable)
    }

    /// Whether tool actions should be recorded in the append-only journal.
    ///
    /// - `Interactive` → false (journal skipped for speed)
    /// - `Resumable`   → true  (in-process recovery)
    /// - `Durable`     → true  (cross-process audit trail)
    /// - `Worker`      → false (parent manages auditing)
    pub fn records_journal(&self) -> bool {
        matches!(self, ExecutionProfile::Resumable | ExecutionProfile::Durable)
    }
}
