// ── Full-pipeline smoke test ──────────────────────────────────────────
//
// Tests the core pipeline end-to-end:
//
//   Source → EventBus → RuleEngine → TaskQueue → Worker → Report → AttentionManager
//
// Uses a MockProvider so no real LLM API key is needed.
// Steps through the pipeline manually for deterministic verification.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use zhongshu_core::agent::attention::AttentionLevel;
use zhongshu_core::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, FinalChoice, LlmProvider, Message, Role,
    StreamEvent,
};
use zhongshu_core::agent::run::{InterruptionAction, RunController};
use zhongshu_core::agent::{
    run_agent, AgentBudget, AgentProfile, AgentRuntime, AttentionManager, Report, Worker,
};
use zhongshu_core::event::{Event, EventBus, SourceEvent};
use zhongshu_core::rule::{Rule, RuleCondition, RuleEngine, RuleTask};
use zhongshu_core::source::Source;
use zhongshu_core::task::TaskQueue;
use zhongshu_core::tool::{Tool, ToolOutput, ToolRegistry};

// ── Mock LLM provider ─────────────────────────────────────────────────

struct MockProvider;

#[async_trait]
impl LlmProvider for MockProvider {
    async fn chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message: Message::assistant("smoke test complete"),
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

    fn change_model(&self, _model: &str) -> std::sync::Arc<dyn LlmProvider> {
        std::sync::Arc::new(MockProvider)
    }
    fn model_name(&self) -> &str {
        "mock"
    }
}

// ── Test source: fires a single Tick event then goes silent ────────────

struct OneShotSource {
    name: String,
    fired: bool,
}

#[async_trait]
impl Source for OneShotSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn poll(&mut self) -> Option<Event> {
        if !self.fired {
            self.fired = true;
            Some(Event::Source(SourceEvent::Tick {
                name: self.name.clone(),
            }))
        } else {
            None
        }
    }
}

// ── Pipeline smoke test ───────────────────────────────────────────────

#[tokio::test]
async fn smoke_full_pipeline_source_to_attention() {
    // ── 1. Setup ──────────────────────────────────────────────────────

    let eb = EventBus::new(64);
    let queue = TaskQueue::new();
    let registry = ToolRegistry::new();

    // Mock LLM provider — returns canned responses, no API key needed.
    let runtime = Arc::new(AgentRuntime::new(
        MockProvider,
        registry,
        "mock-model",
        AgentBudget::default(),
    ));

    // Worker profile for the smoke test.
    let profile = AgentProfile::new(
        "smoke-worker",
        "你是一个测试助手。",
        vec![],
        AgentBudget::default(),
    );

    // AttentionManager — processes reports, routes by AttentionLevel.
    let mut attn_mgr = AttentionManager::new(eb.clone());

    // RuleEngine — one rule: "tick" → spawn a task.
    let mut rule_engine = RuleEngine::new(eb.clone(), queue.clone());
    rule_engine.add_rule(Rule {
        id: "smoke-rule".into(),
        event_pattern: "tick".into(),
        source: None,
        condition: RuleCondition::Always,
        task: RuleTask {
            source: "smoke-source".into(),
            tool: "shell".into(),
            arguments: serde_json::json!({"cmd": "echo hello"}),
        },
    });

    // Subscribe to catch output events.
    let mut rx = eb.subscribe();

    // ── 2. Source fires a Tick event ────────────────────────────────

    let mut source = OneShotSource {
        name: "smoke-source".into(),
        fired: false,
    };
    let source_event = source.poll().await.expect("source should fire once");

    // ── 3. EventBus delivers to RuleEngine ──────────────────────────

    eb.publish(source_event);

    // RuleEngine processes the event (manual step, no spawn needed).
    let received_event = rx
        .try_recv()
        .expect("rule engine should receive the event from EventBus");
    rule_engine.process(&received_event);

    // ── 4. TaskQueue receives the Task from RuleEngine ─────────────

    let task = tokio::time::timeout(std::time::Duration::from_secs(1), queue.recv())
        .await
        .expect("worker should receive a task within timeout")
        .expect("task should not be None");
    assert_eq!(task.source, "rule:smoke-rule");
    assert_eq!(task.tool, "shell");

    // ── 5. Worker executes the Task → produces Report ──────────────

    let report: Report = Worker::execute(&runtime, &profile, task, None)
        .await
        .expect("worker execution should succeed");
    assert_eq!(report.worker, "smoke-worker");
    assert!(report.findings.contains("smoke test complete"));

    // ── 6. AttentionManager processes the Report → routes by level ─
    //
    // The mock LLM returns "smoke test complete" which is non-urgent,
    // so the report is inferred as AttentionLevel::Digest.
    // Digest reports are queued internally, not published to EventBus.

    attn_mgr.process(report);

    // Verify Digest-level reports go to the internal queue, not EventBus.
    let bus_event = rx.try_recv().ok();
    assert!(
        bus_event.is_none(),
        "Digest-level reports should not be published to EventBus, got {bus_event:?}"
    );

    // The report should be in the digest queue instead.
    let drained = attn_mgr.drain_digest();
    assert_eq!(drained.len(), 1, "digest queue should contain the report");
    assert_eq!(drained[0].worker, "smoke-worker");
    assert_eq!(drained[0].attention, AttentionLevel::Digest);
}

// ── Pipeline edge cases ───────────────────────────────────────────────

#[tokio::test]
async fn smoke_rule_engine_ignores_non_matching_events() {
    let eb = EventBus::new(16);
    let queue = TaskQueue::new();

    let mut engine = RuleEngine::new(eb.clone(), queue.clone());
    engine.add_rule(Rule {
        id: "only-ticks".into(),
        event_pattern: "tick".into(),
        source: None,
        condition: RuleCondition::Always,
        task: RuleTask {
            source: "src".into(),
            tool: "x".into(),
            arguments: serde_json::json!({}),
        },
    });

    // Non-tick events should not produce tasks.
    engine.process(&Event::Memory(zhongshu_core::event::MemoryEvent::Compacted));
    engine.process(&Event::Agent(
        zhongshu_core::event::AgentEvent::StateChanged {
            from: zhongshu_core::event::AgentState::Idle,
            to: zhongshu_core::event::AgentState::Thinking,
        },
    ));

    let task_found =
        tokio::time::timeout(std::time::Duration::from_millis(200), queue.recv()).await;
    assert!(
        task_found.is_err(),
        "non-matching events should not produce tasks"
    );
}

#[tokio::test]
async fn smoke_worker_produces_report_with_expected_fields() {
    let runtime = Arc::new(AgentRuntime::new(
        MockProvider,
        ToolRegistry::new(),
        "mock",
        AgentBudget::default(),
    ));
    let profile = AgentProfile::new(
        "test-worker",
        "你是一个测试助手。",
        vec![],
        AgentBudget::default(),
    );

    let task = zhongshu_core::task::Task {
        id: "test-task".into(),
        source: "test".into(),
        tool: "shell".into(),
        arguments: serde_json::json!({"cmd": "date"}),
    };

    let report = Worker::execute(&runtime, &profile, task, None)
        .await
        .expect("worker should succeed");
    assert_eq!(report.task_id, "test-task");
    assert_eq!(report.worker, "test-worker");
    assert!(!report.findings.is_empty());
    assert!(report.confidence >= 0.0);
    assert!(report.confidence <= 1.0);
    // Default attention for unknown content is Digest.
    assert_eq!(report.attention, AttentionLevel::Digest);
}

#[tokio::test]
async fn smoke_attention_manager_drains_digest_queue() {
    let eb = EventBus::new(16);
    let mut mgr = AttentionManager::new(eb);

    // Process two digest-level reports.
    for i in 0..3 {
        mgr.process(Report {
            task_id: format!("t{i}"),
            worker: "w".into(),
            summary: "sum".into(),
            findings: "findings".into(),
            confidence: 0.5,
            success: true,
            outcome: zhongshu_core::agent::RunOutcome::CompletedVerified,
            attention: AttentionLevel::Digest,
            trace_events: vec![],
        });
    }

    let drained = mgr.drain_digest();
    assert_eq!(drained.len(), 3);
    assert!(
        mgr.drain_digest().is_empty(),
        "second drain should be empty"
    );
}

// ── ContextPack smoke test ───────────────────────────────────────────
//
// Tests that ContextPackBuilder produces correct LLM messages from
// realistic inputs: system prompt, state block, evidence, history, input.

use zhongshu_core::core::context::{
    ContextMessage, ContextPackBuilder, ContextRole, EvidenceBlock, EvidenceSource, RecentUnit,
    StateBlock, TrustLevel,
};

#[test]
fn smoke_context_pack_full_pipeline() {
    let evidence = vec![EvidenceBlock {
        id: "ev1".into(),
        source: EvidenceSource::WebSearch,
        source_id: None,
        locator: Some("https://example.com".into()),
        chunk_id: None,
        span: None,
        content: "Rust & C++ are systems languages.".into(),
        confidence: 0.9,
        relevance: 0.8,
        trust: TrustLevel::Untrusted,
    }];

    let state = StateBlock {
        goals: vec!["Answer the user's question".into()],
        todos: vec![],
        memories: vec![],
    };

    let recent = vec![RecentUnit::UserAssistant {
        user: ContextMessage {
            role: ContextRole::User,
            content: "What is Rust?".into(),
            tool_call_id: None,
            tool_calls: vec![],
        },
        assistant: Some(ContextMessage {
            role: ContextRole::Assistant,
            content: "Rust is a systems language.".into(),
            tool_call_id: None,
            tool_calls: vec![],
        }),
    }];

    let (pack, report) = ContextPackBuilder::new()
        .stable_system("You are a helpful assistant.".into())
        .state(state)
        .with_evidence(evidence)
        .with_recent(recent)
        .input("Tell me more".into())
        .build(500_000)
        .expect("ContextPack build should succeed");

    assert!(report.stable_system_tokens > 0);
    assert!(report.state_tokens > 0);
    assert!(report.evidence_tokens > 0);
    assert!(report.recent_tokens > 0);
    assert!(report.input_tokens > 0);
    assert!(report.dropped_evidence_ids.is_empty());
    assert_eq!(report.dropped_recent_units, 0);
    assert!(!report.stable_prefix_hash.is_empty());

    let msgs = pack.into_llm_messages();
    assert_eq!(msgs.len(), 4, "system + user/assistant + user(input) = 4");
    assert_eq!(msgs[0].role, Role::System, "first message should be system");
    assert_eq!(msgs[1].content, "What is Rust?");
    assert_eq!(msgs[2].content, "Rust is a systems language.");
    assert!(msgs[3].content.contains("Tell me more"));
    assert!(
        msgs[3].content.contains("<context>"),
        "input should contain context block"
    );
    assert!(
        msgs[3].content.contains("&amp;"),
        "evidence & should be escaped"
    );
}

#[test]
fn smoke_context_pack_crops_excess_evidence() {
    let many_blocks: Vec<EvidenceBlock> = (0..10)
        .map(|i| EvidenceBlock {
            id: format!("ev{i}"),
            source: EvidenceSource::WebSearch,
            source_id: None,
            locator: None,
            chunk_id: None,
            span: None,
            content: "x".repeat(200),
            confidence: if i < 3 { 0.9 } else { 0.1 },
            relevance: if i < 3 { 0.9 } else { 0.1 },
            trust: TrustLevel::Untrusted,
        })
        .collect();

    let (_pack, report) = ContextPackBuilder::new()
        .stable_system("sys".into())
        .with_evidence(many_blocks)
        .input("Hi".into())
        .build(500)
        .expect("build should succeed with tight budget");

    // Low-scored evidence should be dropped
    assert!(
        !report.dropped_evidence_ids.is_empty(),
        "some evidence should be dropped at tight budget"
    );
    // All dropped IDs should be from the low-confidence group (indices 3-9)
    // Note: ev0, ev1, ev2 have same score (0.486) — if budget fits 2,
    // ev0 and ev1 kept (stable sort), ev2 also dropped.
    let high_confidence_kept: Vec<&String> = report
        .dropped_evidence_ids
        .iter()
        .filter(|id| {
            let num: u32 = id.trim_start_matches("ev").parse().unwrap_or(99);
            num < 3
        })
        .collect();
    // At most 1 high-confidence should be dropped (ev2, when only 2 fit)
    assert!(
        high_confidence_kept.len() <= 1,
        "at most 1 high-confidence evidence should be dropped, got {}: {:?}",
        high_confidence_kept.len(),
        high_confidence_kept,
    );
    // All low-confidence evidence (3-9) should be dropped
    for i in 3..10 {
        let id = format!("ev{}", i);
        assert!(
            report.dropped_evidence_ids.contains(&id),
            "low-confidence ev{} should have been dropped",
            i
        );
    }
}

// ── Step Budget smoke test ───────────────────────────────────────────

#[test]
fn smoke_budget_assistant_defaults() {
    let b = AgentBudget::assistant_default();
    assert_eq!(b.max_steps, 80);
    assert_eq!(b.max_tool_calls, 160);
    assert_eq!(b.per_tool_limit, 40);
    assert_eq!(b.token_limit, 500_000);
    assert_eq!(b.llm_timeout.as_secs(), 240);
    assert_eq!(b.tool_timeout.as_secs(), 120);
}

#[test]
fn smoke_budget_coding_defaults() {
    let b = AgentBudget::coding_default();
    assert_eq!(b.max_steps, 200);
    assert_eq!(b.max_tool_calls, 400);
    assert_eq!(b.per_tool_limit, 200);
    assert_eq!(b.token_limit, 1_000_000);
    assert_eq!(b.llm_timeout.as_secs(), 600);
    assert_eq!(b.tool_timeout.as_secs(), 300);
}

#[test]
fn smoke_budget_default_is_assistant() {
    let b = AgentBudget::default();
    assert_eq!(b.max_steps, 80);
    assert_eq!(b.llm_timeout.as_secs(), 240);
}

// ── Interruption smoke test ────────────────────────────────────────────
//
// Tests that CancellationToken stops the agent loop mid-flight.

#[derive(Clone)]
struct YieldProvider;

#[async_trait]
impl LlmProvider for YieldProvider {
    async fn chat(
        &self,
        _request: ChatCompletionRequest,
    ) -> anyhow::Result<ChatCompletionResponse> {
        tokio::task::yield_now().await;
        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message: Message::assistant("done"),
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
    fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(self.clone())
    }
    fn model_name(&self) -> &str {
        "yield"
    }
}

struct SleepTool;

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &str {
        "sleep"
    }
    fn description(&self) -> &str {
        "sleeps for testing"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{}})
    }
    async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
        tokio::time::sleep(Duration::from_millis(200)).await;
        ToolOutput::success(serde_json::json!({"slept": true}))
    }
}

#[tokio::test]
async fn smoke_token_cancellation_stops_agent_loop() {
    let mut runtime = AgentRuntime::new(
        YieldProvider,
        ToolRegistry::new().register(SleepTool),
        "yield",
        AgentBudget {
            max_steps: 10,
            max_tool_calls: 10,
            per_tool_limit: 10,
            token_limit: 10_000,
            llm_timeout: Duration::from_secs(5),
            tool_timeout: Duration::from_secs(5),
        },
    );

    let cancel_token = CancellationToken::new();
    // Cancel immediately so the agent loop sees it on first iteration
    cancel_token.cancel();

    let result = run_agent(
        &mut runtime,
        vec![Message::user("请一直运行")],
        None,
        "cancel-test",
        cancel_token,
    )
    .await;

    let result = result.expect("run_agent should return Ok even if cancelled");
    assert!(
        matches!(
            result.stop_reason,
            zhongshu_core::agent::StopReason::Interrupted
        ),
        "cancelled token should produce Interrupted, got {:?}",
        result.stop_reason
    );
}

// ── RunController interrupt sync test ──────────────────────────────────
// RunController uses tokio::sync::RwLock internally, so its methods
// must be called outside a tokio runtime context.

#[test]
fn smoke_run_controller_interrupt_captures_context() {
    let eb = Arc::new(EventBus::new(16));
    let (response_tx, _response_rx) = tokio::sync::mpsc::channel(64);

    let controller = RunController::new(eb, response_tx);

    controller.start_run("测试插话打断");
    assert!(
        controller.run_id().is_some(),
        "run_id should be set after start_run"
    );

    let action = controller.interrupt("停下");

    assert!(
        matches!(action, InterruptionAction::CancelAndReplan { .. }),
        "interrupt('停下') should produce CancelAndReplan, got {action:?}"
    );
    assert!(
        controller.is_interrupted(),
        "controller should report interrupted"
    );
    assert!(
        controller.interruption_ctx().is_some(),
        "interruption context should be captured"
    );
    if let Some(ctx) = controller.interruption_ctx() {
        assert!(
            ctx.user_message.contains("停下"),
            "context should contain the interjection message"
        );
    }
}
