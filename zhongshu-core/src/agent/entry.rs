use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::llm::Message;
use crate::agent::loop_::{run_agent, run_agent_with_context, AgentCallbacks, LoopResult};
use crate::agent::runtime::AgentRuntime;
use crate::core::context::ContextPack;
use crate::runtime::profile::ExecutionProfile;

/// The single external entry point for executing an agent attempt.
///
/// All interactive runs, recovery runs, replan runs, and background
/// task steps must go through this function.  Directly calling
/// `run_agent()` or `run_agent_with_context()` is forbidden —
/// the xtask architecture-check enforces this.
///
/// Variant that accepts a pre-built `ContextPack` (used by `run_attempt()`).
pub async fn execute_agent_loop(
    runtime: &mut AgentRuntime,
    context: ContextPack,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
    _profile: ExecutionProfile,
) -> anyhow::Result<LoopResult> {
    run_agent_with_context(runtime, context, callbacks, source, cancel_token).await
}

/// Variant that accepts raw messages (used by CLI and simple callers).
pub async fn execute_agent_loop_with_messages(
    runtime: &mut AgentRuntime,
    messages: Vec<Message>,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
    _profile: ExecutionProfile,
) -> anyhow::Result<LoopResult> {
    run_agent(runtime, messages, callbacks, source, cancel_token).await
}
