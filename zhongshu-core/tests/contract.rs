// ── Cross-layer runtime contract tests ─────────────────────────────────
//
// These tests verify the full agent pipeline end-to-end (Looper → Provider
// → ToolExecutor → ApprovalGate → RunOutcome). Each test exercises a
// specific runtime contract that was identified as critical in the project
// review (2026-07-17).
//
// Unlike unit tests, these integration tests use the real `run_agent` entry
// point with controlled providers and tools, validating that the runtime
// contracts hold across layer boundaries.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zhongshu_core::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, FinalChoice, FunctionCall, LlmProvider, Message,
    Role, StreamEvent, ToolCall,
};
use zhongshu_core::agent::{
    run_agent, AgentBudget, AgentRuntime, LoopResult, RunOutcome, StopReason,
};
use zhongshu_core::tool::{Tool, ToolOutput, ToolRegistry};

// ── Scripted LLM provider ──────────────────────────────────────────────
//
// Follows a script of (tool_name, args) tuples. Each entry produces a tool
// call with those arguments. After the script runs out, a final text
// message is returned. Using distinct args per entry avoids the harness
// duplicate-tool guard.

type ScriptEntry = (String, String);

struct ScriptedProvider {
    script: Arc<Vec<ScriptEntry>>,
    idx: Arc<Mutex<usize>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(&self, _request: ChatCompletionRequest) -> anyhow::Result<ChatCompletionResponse> {
        let mut idx = self.idx.lock().unwrap();
        let i = *idx;
        *idx += 1;

        let message = if i < self.script.len() {
            let (tool_name, args) = &self.script[i];
            Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: format!("call-{i}"),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: tool_name.clone(),
                        arguments: args.clone(),
                    },
                }],
            )
        } else {
            Message::assistant("contract test complete")
        };

        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message,
                finish_reason: None,
            }],
            usage: None,
        })
    }

    async fn stream_chat(
        &self,
        _request: ChatCompletionRequest,
        _on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("streaming not used in contract tests")
    }

    fn model_name(&self) -> &str {
        "contract-test"
    }

    fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(ScriptedProvider {
            script: self.script.clone(),
            idx: Arc::new(Mutex::new(0)),
        })
    }
}

// ── Controlled tools for contract tests ────────────────────────────────

struct OkTool;

#[async_trait]
impl Tool for OkTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "always succeeds"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{}})
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        ToolOutput::success(json!({"ok": true}))
    }
}

struct ErrorTool;

#[async_trait]
impl Tool for ErrorTool {
    fn name(&self) -> &str {
        "error_tool"
    }

    fn description(&self) -> &str {
        "always fails"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({"type":"object","properties":{}})
    }

    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        ToolOutput::error("simulated failure")
    }
}

fn small_budget() -> AgentBudget {
    AgentBudget {
        max_steps: 10,
        max_tool_calls: 10,
        per_tool_limit: 10,
        token_limit: 10_000,
        llm_timeout: Duration::from_secs(5),
        tool_timeout: Duration::from_secs(5),
    }
}

async fn run_agent_with(
    provider: ScriptedProvider,
    registry: ToolRegistry,
    budget: AgentBudget,
    cancel: CancellationToken,
) -> LoopResult {
    let mut runtime = AgentRuntime::new(provider, registry, "contract-test", budget);
    run_agent(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        cancel,
    )
    .await
    .unwrap()
}

fn scripted(entries: &[(&str, &str)]) -> ScriptedProvider {
    ScriptedProvider {
        script: Arc::new(
            entries
                .iter()
                .map(|(name, args)| (name.to_string(), args.to_string()))
                .collect(),
        ),
        idx: Arc::new(Mutex::new(0)),
    }
}

// ── Contract tests ─────────────────────────────────────────────────────

/// Full turn (tool call → final text) produces RunOutcome::CompletedVerified.
#[tokio::test]
async fn completed_turn_has_completed_verified_outcome() {
    let result = run_agent_with(
        scripted(&[("noop", "{}")]),
        ToolRegistry::new().register(OkTool),
        small_budget(),
        CancellationToken::new(),
    )
    .await;

    assert_eq!(result.outcome, RunOutcome::CompletedVerified);
    assert!(matches!(result.stop_reason, StopReason::Finished));
}

/// 3+ consecutive tool errors produce ToolFailurePersistent → Failed.
/// Uses unique args per call to bypass the harness duplicate-tool guard.
#[tokio::test]
async fn persistent_tool_failure_returns_failed() {
    let result = run_agent_with(
        scripted(&[
            ("error_tool", r#"{"n":1}"#),
            ("error_tool", r#"{"n":2}"#),
            ("error_tool", r#"{"n":3}"#),
            ("error_tool", r#"{"n":4}"#),
        ]),
        ToolRegistry::new().register(ErrorTool),
        small_budget(),
        CancellationToken::new(),
    )
    .await;

    assert_eq!(result.outcome, RunOutcome::Failed);
    assert!(matches!(
        result.stop_reason,
        StopReason::ToolFailurePersistent
    ));
}

/// Hitting max_tool_calls produces Blocked.
#[tokio::test]
async fn max_tool_calls_exhausted_returns_blocked() {
    let mut runtime = AgentRuntime::new(
        scripted(&[("noop", r#"{"n":1}"#), ("noop", r#"{"n":2}"#)]),
        ToolRegistry::new().register(OkTool),
        "contract-test",
        AgentBudget {
            max_steps: 10,
            max_tool_calls: 1,
            per_tool_limit: 5,
            token_limit: 10_000,
            llm_timeout: Duration::from_secs(5),
            tool_timeout: Duration::from_secs(5),
        },
    );

    let result = run_agent(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.outcome, RunOutcome::Blocked);
    assert!(matches!(
        result.stop_reason,
        StopReason::MaxToolCallsReached
    ));
}

/// Hitting max_steps produces Blocked.
#[tokio::test]
async fn max_steps_exhausted_returns_blocked() {
    let mut runtime = AgentRuntime::new(
        scripted(&[("noop", r#"{"n":1}"#), ("noop", r#"{"n":2}"#), ("noop", r#"{"n":3}"#)]),
        ToolRegistry::new().register(OkTool),
        "contract-test",
        AgentBudget {
            max_steps: 2,
            max_tool_calls: 10,
            per_tool_limit: 5,
            token_limit: 10_000,
            llm_timeout: Duration::from_secs(5),
            tool_timeout: Duration::from_secs(5),
        },
    );

    let result = run_agent(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(result.outcome, RunOutcome::Blocked);
    assert!(matches!(result.stop_reason, StopReason::MaxStepsReached));
}

/// Cancel during agent run produces Interrupted.
/// Use a slow tool to guarantee the agent is still running when we cancel.
#[tokio::test]
async fn cancel_during_run_returns_interrupted() {
    struct SlowTool;
    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &str { "slow_tool" }
        fn description(&self) -> &str { "slow" }
        fn parameters(&self) -> serde_json::Value { json!({"type":"object","properties":{}}) }
        async fn execute(&self, _: &serde_json::Value) -> ToolOutput {
            tokio::time::sleep(Duration::from_secs(10)).await;
            ToolOutput::success(json!({"ok": true}))
        }
    }

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let mut runtime = AgentRuntime::new(
        scripted(&[("slow_tool", "{}")]),
        ToolRegistry::new().register(SlowTool),
        "contract-test",
        small_budget(),
    );

    let handle = tokio::spawn(async move {
        run_agent(
            &mut runtime,
            vec![Message::user("run contract test")],
            None,
            "contract-test",
            cancel_clone,
        )
        .await
        .unwrap()
    });

    // Give the agent time to start the slow tool
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    let result = handle.await.unwrap();
    assert_eq!(result.outcome, RunOutcome::Interrupted);
}

/// Tool error observation contains the error message in the tool result.
#[tokio::test]
async fn tool_error_observation_renders_error_text() {
    let mut runtime = AgentRuntime::new(
        scripted(&[("error_tool", r#"{"n":1}"#)]),
        ToolRegistry::new().register(ErrorTool),
        "contract-test",
        small_budget(),
    );

    let result = run_agent(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let has_error = result.messages.iter().any(|m| {
        matches!(m.role, Role::Tool) && m.content.contains("simulated failure")
    });
    assert!(
        has_error,
        "tool result should contain error text"
    );
}

/// Two consecutive runs produce independent results.
#[tokio::test]
async fn two_consecutive_runs_produce_independent_results() {
    let mut runtime = AgentRuntime::new(
        scripted(&[("noop", r#"{"n":1}"#)]),
        ToolRegistry::new().register(OkTool),
        "contract-test",
        small_budget(),
    );

    let r1 = run_agent(
        &mut runtime,
        vec![Message::user("first run")],
        None,
        "contract-test",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(r1.outcome, RunOutcome::CompletedVerified);

    let r2 = run_agent(
        &mut runtime,
        vec![Message::user("second run")],
        None,
        "contract-test",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(r2.outcome, RunOutcome::CompletedVerified);
}

/// Pre-cancelled token skips the loop and returns Interrupted immediately.
#[tokio::test]
async fn pre_cancelled_token_returns_interrupted() {
    let cancel = CancellationToken::new();
    cancel.cancel();

    let mut runtime = AgentRuntime::new(
        scripted(&[("noop", "{}")]),
        ToolRegistry::new().register(OkTool),
        "contract-test",
        small_budget(),
    );

    let result = run_agent(
        &mut runtime,
        vec![Message::user("should not run")],
        None,
        "contract-test",
        cancel,
    )
    .await
    .unwrap();

    assert_eq!(result.outcome, RunOutcome::Interrupted);
}
