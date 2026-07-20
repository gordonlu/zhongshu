use std::sync::Arc;

use crate::agent::llm::LlmProvider;
use crate::agent::loop_::AgentBudget;
use crate::core::checkpoint::CheckpointStore;
use crate::harness::HarnessState;
use crate::tool::ToolRegistry;

/// Long-lived execution context for an agent.
///
/// Holds the provider, tool set, model ID, and budget that together
/// define *how* an agent can act.  No per-turn state (messages,
/// conversation history) lives here — that belongs in the caller
/// (the Invocation Layer).
///
/// Cheap to clone — the provider is reference-counted.
#[derive(Clone)]
pub struct AgentRuntime {
    pub provider: Arc<dyn LlmProvider>,
    pub registry: ToolRegistry,
    pub model: String,
    pub budget: AgentBudget,
    pub reasoning_effort: Option<String>,
    pub harness_state: HarnessState,
    /// Optional check to see if a tool call (name, args) has already been
    /// completed in the current run.  Returns true if the tool was already
    /// completed and should be skipped (survives restarts via the ledger).
    pub idempotency_checker: Option<Arc<dyn Fn(&str, &str) -> bool + Send + Sync>>,
    /// Optional checkpoint store for durable agent state persistence.
    /// When set, the agent loop saves a checkpoint before each tool call
    /// so a crashed process can restore from the last known-good state.
    pub checkpoint_store: Option<CheckpointStore>,
    /// Optional ledger for reconciling in-flight tools after a crash.
    pub ledger: Option<crate::core::ledger::RunLedger>,
}

impl AgentRuntime {
    pub fn new(
        provider: impl LlmProvider + 'static,
        registry: ToolRegistry,
        model: impl Into<String>,
        budget: AgentBudget,
    ) -> Self {
        AgentRuntime {
            provider: Arc::new(provider),
            registry,
            model: model.into(),
            budget,
            reasoning_effort: None,
            harness_state: HarnessState::new(),
            idempotency_checker: None,
            checkpoint_store: None,
            ledger: None,
        }
    }

    /// Create a new runtime with a different LLM provider/model (Phase 7).
    pub fn with_llm(
        provider: Arc<dyn LlmProvider>,
        model: String,
        registry: ToolRegistry,
        budget: AgentBudget,
    ) -> Self {
        AgentRuntime {
            provider,
            registry,
            model,
            budget,
            reasoning_effort: None,
            harness_state: HarnessState::new(),
            idempotency_checker: None,
            checkpoint_store: None,
            ledger: None,
        }
    }

    /// Apply profile-level LLM overrides (Phase 7).
    /// Returns self if no overrides, or a new runtime with the overridden provider/model.
    pub fn with_profile_llm(mut self, profile: &crate::agent::profile::AgentProfile) -> Self {
        if let Some(ref m) = profile.llm_model {
            self.model = m.clone();
        }
        if let Some(ref r) = profile.llm_reasoning_effort {
            self.reasoning_effort = Some(r.clone());
        }
        self
    }
}
