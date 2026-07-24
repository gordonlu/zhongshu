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
use uuid::Uuid;
use zhongshu_core::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, FinalChoice, FunctionCall, LlmProvider, Message,
    Role, StreamEvent, ToolCall,
};
use zhongshu_core::agent::{
    execute_agent_loop_with_messages, AgentBudget, AgentCallbacks, AgentRuntime, LoopResult,
    RunOutcome, StopReason,
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
    async fn chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
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
        request: ChatCompletionRequest,
        mut on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        let response = self.chat(request).await?;
        if let Some(choice) = response.choices.into_iter().next() {
            let tool_calls = choice.message.tool_calls.unwrap_or_default();
            let stream_calls: Vec<zhongshu_core::agent::llm::StreamToolCall> = tool_calls
                .into_iter()
                .map(|tc| zhongshu_core::agent::llm::StreamToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: tc.function.arguments,
                })
                .collect();
            on_event(StreamEvent::Finished {
                finish_reason: choice.finish_reason.unwrap_or_default(),
                content: choice.message.content,
                tool_calls: stream_calls,
            });
        }
        Ok(())
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
    execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        cancel,
        zhongshu_core::runtime::ExecutionProfile::Interactive,
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

/// Full turn (tool call → final text) produces RunOutcome::CompletedUnverified
/// when no verification evidence is provided.
#[tokio::test]
async fn completed_turn_has_completed_unverified_outcome() {
    let result = run_agent_with(
        scripted(&[("noop", "{}")]),
        ToolRegistry::new().register(OkTool),
        small_budget(),
        CancellationToken::new(),
    )
    .await;

    assert_eq!(result.outcome, RunOutcome::CompletedUnverified);
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

    let result = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
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
        scripted(&[
            ("noop", r#"{"n":1}"#),
            ("noop", r#"{"n":2}"#),
            ("noop", r#"{"n":3}"#),
        ]),
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

    let result = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
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
        fn name(&self) -> &str {
            "slow_tool"
        }
        fn description(&self) -> &str {
            "slow"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
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
        execute_agent_loop_with_messages(
            &mut runtime,
            vec![Message::user("run contract test")],
            None,
            "contract-test",
            cancel_clone,
            zhongshu_core::runtime::ExecutionProfile::Interactive,
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

/// Simulate a crash DURING tool execution: save a dirty checkpoint while
/// the tool is marked as "started" but before completion.
#[tokio::test]
async fn crash_during_tool_detects_inflight_and_reports_unknown_effect() {
    use std::collections::HashMap;
    use std::sync::Arc;
    use zhongshu_core::agent::execute_agent_loop_with_messages;
    use zhongshu_core::agent::AgentCallbacks;
    use zhongshu_core::core::checkpoint::{AgentCheckpoint, CheckpointStore};
    use zhongshu_core::core::ledger::RunLedger;
    use zhongshu_core::core::Database;

    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path().join("crash.db"));
    db.migrate().unwrap();

    let checkpoint_store = CheckpointStore::new(db.clone());
    let ledger = RunLedger::new(db);
    let run_id = uuid::Uuid::new_v4();

    // Record the tool start (simulating state at crash time)
    ledger
        .record_run_started(&run_id.to_string(), "crash test")
        .unwrap();
    ledger
        .record_tool_call(
            &run_id.to_string(),
            "noop",
            "{}",
            "started",
            None,
            Some(&zhongshu_core::agent::run::RunController::idempotency_key(
                "noop", "{}",
            )),
        )
        .unwrap();

    // Save checkpoint as it would be before tool execution
    let crash_cp = AgentCheckpoint {
        run_id: run_id.to_string(),
        step: 1,
        tool_calls_made: 0,
        consecutive_failures: 0,
        tool_call_counts: HashMap::new(),
        messages: vec![
            Message::system("测试助手。"),
            Message::user("run crashtest"),
            Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: "call-crash-1".into(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "noop".into(),
                        arguments: "{}".into(),
                    },
                }],
            ),
        ],
        created_at: 0,
    };
    checkpoint_store.save(&crash_cp, true).unwrap();

    // Verify inflight detection
    assert!(
        ledger.has_inflight_tools(&run_id.to_string()).unwrap(),
        "started tool should be in-flight"
    );

    // Fresh runtime loading the checkpoint
    let mut recovery_runtime = AgentRuntime::new(
        scripted(&[("noop", "{}")]),
        ToolRegistry::new().register(OkTool),
        "contract-test",
        small_budget(),
    );
    recovery_runtime.checkpoint_store = Some(checkpoint_store.clone());
    recovery_runtime.ledger = Some(ledger.clone());

    let callbacks = Arc::new(AgentCallbacks {
        on_text: Box::new(|_| {}),
        on_tool_start: Box::new(|_: &str, _: &str| {}),
        on_tool_done: Box::new(
            |_: &str, _: &str, _: zhongshu_core::agent::loop_::ToolCompletionStatus| {},
        ),
        run_id,
    });

    let recovery_result = execute_agent_loop_with_messages(
        &mut recovery_runtime,
        vec![Message::user("recovery test")],
        Some(callbacks.clone()),
        "crash-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
    )
    .await
    .unwrap();

    // Recovery should complete successfully
    assert!(
        matches!(recovery_result.outcome, RunOutcome::CompletedUnverified),
        "recovery run should complete: got {:?}",
        recovery_result.outcome
    );

    // Messages should contain unknown-effect or inflight warning
    let has_unknown = recovery_result
        .messages
        .iter()
        .any(|m| m.content.contains("unknown_effect") || m.content.contains("状态未知"));
    assert!(
        has_unknown,
        "recovery should report unknown effect for in-flight tool"
    );

    // After recovery, the agent received the unknown-effect observation.
    // The ledger still preserves the original 'started' record — it is
    // the crash evidence and is never deleted or overwritten.
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

    let result = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("run contract test")],
        None,
        "contract-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
    )
    .await
    .unwrap();

    let has_error = result
        .messages
        .iter()
        .any(|m| matches!(m.role, Role::Tool) && m.content.contains("simulated failure"));
    assert!(has_error, "tool result should contain error text");
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

    let r1 = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("first run")],
        None,
        "contract-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
    )
    .await
    .unwrap();
    assert_eq!(r1.outcome, RunOutcome::CompletedUnverified);

    let r2 = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("second run")],
        None,
        "contract-test",
        CancellationToken::new(),
        zhongshu_core::runtime::ExecutionProfile::Interactive,
    )
    .await
    .unwrap();
    assert_eq!(r2.outcome, RunOutcome::CompletedUnverified);
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

    let result = execute_agent_loop_with_messages(
        &mut runtime,
        vec![Message::user("should not run")],
        None,
        "contract-test",
        cancel,
        zhongshu_core::runtime::ExecutionProfile::Interactive,
    )
    .await
    .unwrap();

    assert_eq!(result.outcome, RunOutcome::Interrupted);
}

// ── Interruption while provider is streaming text (Invariant 3) ─────────
//
// Provider that emits one streaming delta then blocks.
struct SlowStreamProvider;

#[async_trait]
impl LlmProvider for SlowStreamProvider {
    async fn chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message: Message::assistant("slow stream complete"),
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        })
    }

    async fn stream_chat(
        &self,
        _request: ChatCompletionRequest,
        mut on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        on_event(StreamEvent::TextDelta("Starting...".into()));
        tokio::time::sleep(Duration::from_secs(30)).await;
        on_event(StreamEvent::Finished {
            finish_reason: "stop".into(),
            content: "slow stream complete".into(),
            tool_calls: Vec::new(),
        });
        Ok(())
    }

    fn model_name(&self) -> &str {
        "slow-stream"
    }
    fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(Self)
    }
    async fn embed(&self, _input: &str) -> anyhow::Result<Vec<f32>> {
        Ok(vec![0.0])
    }
}

/// (a) Interrupt during streaming: cancel_token fires while stream_chat is
///     emitting deltas → loop returns Interrupted, partial content is NOT
///     committed to the message list.
#[tokio::test]
async fn cancel_during_streaming_blocks_text_and_returns_interrupted() {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let mut runtime = AgentRuntime::new(
        SlowStreamProvider,
        ToolRegistry::new(),
        "contract-test",
        small_budget(),
    );

    let callbacks = Arc::new(AgentCallbacks {
        on_text: Box::new(|_| {}),
        on_tool_start: Box::new(|_, _| {}),
        on_tool_done: Box::new(|_, _, _| {}),
        run_id: Uuid::new_v4(),
    });

    let handle = tokio::spawn(async move {
        execute_agent_loop_with_messages(
            &mut runtime,
            vec![Message::user("test")],
            Some(callbacks),
            "contract-test",
            cancel_clone,
            zhongshu_core::runtime::ExecutionProfile::Interactive,
        )
        .await
        .unwrap()
    });

    // Let the provider emit the first delta.
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = handle.await.unwrap();
    assert_eq!(result.outcome, RunOutcome::Interrupted);
    // No full response content should appear in the final messages.
    assert!(
        result
            .messages
            .iter()
            .all(|m| !m.content.contains("slow stream complete")),
        "interrupted stream must not commit streamed content to message list"
    );
}

/// (b) Interrupt during tool execution: cancel_token fires while a tool is
///     running → loop returns Interrupted, tool output is NOT committed.
#[tokio::test]
async fn cancel_during_tool_execution_does_not_commit_result() {
    struct HangingTool;
    #[async_trait]
    impl Tool for HangingTool {
        fn name(&self) -> &str {
            "hanging_tool"
        }
        fn description(&self) -> &str {
            "hangs until cancelled"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            tokio::time::sleep(Duration::from_secs(30)).await;
            ToolOutput::success(json!({"done": true}))
        }
    }

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let mut runtime = AgentRuntime::new(
        scripted(&[("hanging_tool", "{}")]),
        ToolRegistry::new().register(HangingTool),
        "contract-test",
        small_budget(),
    );

    let handle = tokio::spawn(async move {
        execute_agent_loop_with_messages(
            &mut runtime,
            vec![Message::user("run tool")],
            None,
            "contract-test",
            cancel_clone,
            zhongshu_core::runtime::ExecutionProfile::Interactive,
        )
        .await
        .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();

    let result = handle.await.unwrap();
    assert_eq!(result.outcome, RunOutcome::Interrupted);
    // Tool output observation must NOT appear in messages.
    assert!(
        result
            .messages
            .iter()
            .all(|m| !m.content.contains("done")),
        "interrupted tool must not commit tool output to message list"
    );
}

/// (c) Interrupt during sync_step (non-streaming, text response): cancel fires
///     while the LLM call is in flight → loop returns Interrupted.
#[tokio::test]
async fn cancel_during_sync_step_returns_interrupted() {
    struct SlowTextProvider;
    #[async_trait]
    impl LlmProvider for SlowTextProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(ChatCompletionResponse {
                choices: vec![FinalChoice {
                    message: Message::assistant("slow text answer"),
                    finish_reason: Some("stop".into()),
                }],
                usage: None,
            })
        }
        async fn stream_chat(
            &self,
            _request: ChatCompletionRequest,
            _on_event: Box<dyn FnMut(StreamEvent) + Send>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        fn model_name(&self) -> &str {
            "slow-text"
        }
        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(Self)
        }
        async fn embed(&self, _input: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![0.0])
        }
    }

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let mut runtime = AgentRuntime::new(
        SlowTextProvider,
        ToolRegistry::new(),
        "contract-test",
        small_budget(),
    );

    let handle = tokio::spawn(async move {
        execute_agent_loop_with_messages(
            &mut runtime,
            vec![Message::user("test")],
            None,
            "contract-test",
            cancel_clone,
            zhongshu_core::runtime::ExecutionProfile::Interactive,
        )
        .await
        .unwrap()
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = handle.await.unwrap();
    assert_eq!(result.outcome, RunOutcome::Interrupted);
    assert!(
        result
            .messages
            .iter()
            .all(|m| !m.content.contains("slow text answer")),
        "interrupted sync step must not commit text content"
    );
}
