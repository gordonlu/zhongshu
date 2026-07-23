use std::sync::Arc;
use std::time::Duration;

use crate::agent::llm::{Message, StreamEvent, StreamToolCall, ToolCall};
use crate::agent::runtime::AgentRuntime;
use crate::core::checkpoint::AgentCheckpoint;
use crate::core::context::ContextPack;
use crate::event::{Event, HarnessUiEvent};
use crate::harness::trace::event::HarnessEvent;
use crate::tool::{ToolOutput, ToolStatus, ToolTermination};
use anyhow::Context;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Per-agent resource budget.
#[derive(Debug, Clone)]
pub struct AgentBudget {
    pub max_steps: u32,
    pub max_tool_calls: u32,
    pub per_tool_limit: u32,
    pub token_limit: usize,
    pub llm_timeout: Duration,
    pub tool_timeout: Duration,
}

impl AgentBudget {
    pub fn assistant_default() -> Self {
        Self {
            max_steps: 80,
            max_tool_calls: 256,
            per_tool_limit: 128,
            token_limit: 500_000,
            llm_timeout: Duration::from_secs(240),
            tool_timeout: Duration::from_secs(120),
        }
    }

    pub fn coding_default() -> Self {
        Self {
            max_steps: 200,
            max_tool_calls: 1024,
            per_tool_limit: 512,
            token_limit: 1_000_000,
            llm_timeout: Duration::from_secs(600),
            tool_timeout: Duration::from_secs(300),
        }
    }
}

impl Default for AgentBudget {
    fn default() -> Self {
        Self::assistant_default()
    }
}

fn check_budget(
    tool_calls_made: usize,
    consecutive_failures: u32,
    budget: &AgentBudget,
) -> Result<(), StopReason> {
    if tool_calls_made >= budget.max_tool_calls as usize {
        return Err(StopReason::MaxToolCallsReached);
    }
    if consecutive_failures >= 3 {
        return Err(StopReason::ToolFailurePersistent);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Finished,
    BudgetExhausted { tokens: usize, limit: usize },
    MaxStepsReached,
    MaxToolCallsReached,
    ToolFailurePersistent,
    Interrupted,
}

/// Unified outcome for a run, consumed by UI, Worker, Task, Runbook, Replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RunOutcome {
    CompletedVerified,
    CompletedUnverified,
    Blocked,
    Interrupted,
    BudgetExhausted,
    Failed,
}

impl From<StopReason> for RunOutcome {
    fn from(s: StopReason) -> Self {
        match s {
            StopReason::Finished => RunOutcome::CompletedUnverified,
            StopReason::BudgetExhausted { .. } => RunOutcome::BudgetExhausted,
            StopReason::MaxStepsReached | StopReason::MaxToolCallsReached => RunOutcome::Blocked,
            StopReason::ToolFailurePersistent => RunOutcome::Failed,
            StopReason::Interrupted => RunOutcome::Interrupted,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub messages: Vec<Message>,
    pub stop_reason: StopReason,
    pub outcome: RunOutcome,
    pub tool_calls_made: usize,
    pub estimated_tokens: usize,
    pub trace_events: Vec<HarnessEvent>,
}

/// Streaming callbacks forwarded to the UI layer.
pub struct AgentCallbacks {
    pub on_text: Box<dyn Fn(&str) + Send + Sync>,
    pub on_tool_start: Box<dyn Fn(&str, &str) + Send + Sync>,
    pub on_tool_done: Box<dyn Fn(&str, &str, ToolCompletionStatus) + Send + Sync>,
    pub run_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCompletionStatus {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    UnknownEffect,
    AwaitingApproval,
}

impl ToolCompletionStatus {
    pub fn as_ledger_status(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::UnknownEffect => "unknown_effect",
            Self::AwaitingApproval => "awaiting_approval",
        }
    }

    pub fn is_success(self) -> bool {
        self == Self::Completed
    }
}

/// Run the full ReAct loop using the given runtime and initial messages.
///
/// The caller owns the message list (system prompt, profile context,
/// context engine, user input — everything) and passes it in.
/// After completion the final message list is returned inside `LoopResult`.
pub(crate) async fn run_agent(
    runtime: &mut AgentRuntime,
    messages: Vec<Message>,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
) -> anyhow::Result<LoopResult> {
    run_agent_with_verification_policy(runtime, messages, callbacks, source, cancel_token, None)
        .await
}

pub(crate) async fn run_agent_with_verification_policy(
    runtime: &mut AgentRuntime,
    mut messages: Vec<Message>,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
    verification_required: Option<bool>,
) -> anyhow::Result<LoopResult> {
    if verification_required.unwrap_or_else(|| user_requested_verification(&messages)) {
        runtime.harness_state.verification.required = true;
    } else if verification_required == Some(false) {
        runtime.harness_state.verification.required = false;
    }
    runtime.harness_state.trace.events.clear();
    record_trace(
        runtime,
        HarnessEvent::RunStarted {
            timestamp: trace_timestamp(),
            input: run_input(&messages, source),
            mode: "react".into(),
        },
    );

    let mut tool_calls_made = 0;
    let mut consecutive_tool_failures = 0u32;
    let mut tool_call_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    let run_id = callbacks
        .as_ref()
        .map(|c| c.run_id.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // ── Restore from checkpoint if available ──────────────────────────
    // If the process crashed mid-run, the last checkpoint contains the
    // exact messages and counters from before the interrupted tool call.
    let mut start_step = 0u32;
    if let Some(ref cs) = runtime.checkpoint_store {
        match cs.load_latest(&run_id) {
            Ok(Some(cp)) => {
                info!(
                    step = cp.step,
                    tool_calls = cp.tool_calls_made,
                    "restored agent checkpoint, resuming from step {}",
                    cp.step,
                );
                messages = cp.messages;
                tool_calls_made = cp.tool_calls_made;
                consecutive_tool_failures = cp.consecutive_failures;
                tool_call_counts = cp.tool_call_counts;
                start_step = cp.step;

                // A checkpoint is written after the assistant emitted tool
                // calls and before execution. Close every unresolved tool call
                // with an explicit unknown-effect result before asking the LLM
                // to continue; provider APIs reject dangling tool-call history.
                close_unresolved_tool_calls(&mut messages);
                if !source.trim().is_empty() {
                    messages.push(Message::user(format!(
                        "恢复运行后的用户指令：{}",
                        source.trim()
                    )));
                }

                // Reconcile any tools that were in-flight at crash time.
                // These started but never completed/failed — their side
                // effects are unknown. Inject a system message so the
                // agent knows to re-evaluate.
                if let Some(ref ledger) = runtime.ledger {
                    if let Ok(inflight) = ledger.reconcile_inflight_tools(&run_id) {
                        for (tool_name, _args, _key) in &inflight {
                            warn!(
                                tool = %tool_name,
                                "in-flight tool at crash time — outcome unknown"
                            );
                        }
                        if !inflight.is_empty() {
                            messages.push(Message::system(
                                "【恢复警告】以下工具在前一次运行中已开始但未能完成：",
                            ));
                            for (tool_name, _args, _key) in &inflight {
                                messages.push(Message::system(&format!(
                                    "- {tool_name}：执行状态未知，请检查是否需要重做"
                                )));
                            }
                        }
                    }
                }
            }
            Ok(None) => { /* no checkpoint, start fresh */ }
            Err(e) => warn!(error = %e, "failed to load agent checkpoint"),
        }
    }

    // ── ActionJournal (wraps the append-only ledger) ────────────────
    let journal = crate::action::ActionJournal::new(runtime.ledger.clone(), &run_id);

    for step in start_step..runtime.budget.max_steps {
        // Harness records use 1-based steps so "0" can remain the
        // sentinel for "no edit/verification has happened yet".
        let harness_step = step + 1;

        if let Err(stop_reason) =
            check_budget(tool_calls_made, consecutive_tool_failures, &runtime.budget)
        {
            let tokens = estimate_total_tokens(&messages);
            return Ok(finish_loop_result(
                runtime,
                &mut messages,
                stop_reason,
                tool_calls_made,
                tokens,
                &run_id,
            ));
        }

        if cancel_token.is_cancelled() {
            let tokens = estimate_total_tokens(&messages);
            return Ok(finish_loop_result(
                runtime,
                &mut messages,
                StopReason::Interrupted,
                tool_calls_made,
                tokens,
                &run_id,
            ));
        }

        // Per-step progress tracking for recovery no-progress detection
        let mut step_had_file_read = false;
        let mut step_had_successful_edit = false;
        let mut step_had_successful_test = false;

        // Harness: pre-turn checks
        {
            // Phase transitions: compare phase from BEFORE this turn's post-tool
            // updates (saved in previous_phase) with the current phase.
            let phase_fb = crate::harness::phase::validate_transition(
                runtime.harness_state.previous_phase,
                runtime.harness_state.phase,
            );
            for fb in phase_fb {
                let text = crate::harness::render::render_feedback(&fb);
                messages.push(Message::system(text));
            }
        }

        let current_tokens = estimate_total_tokens(&messages);

        if current_tokens > runtime.budget.token_limit {
            warn!(
                tokens = current_tokens,
                limit = runtime.budget.token_limit,
                "token budget exhausted"
            );
            let tokens = estimate_total_tokens(&messages);
            return Ok(finish_loop_result(
                runtime,
                &mut messages,
                StopReason::BudgetExhausted {
                    tokens: current_tokens,
                    limit: runtime.budget.token_limit,
                },
                tool_calls_made,
                tokens,
                &run_id,
            ));
        }

        debug!(step, tokens = current_tokens, "agent loop iteration");

        if cancel_token.is_cancelled() {
            let tokens = estimate_total_tokens(&messages);
            return Ok(finish_loop_result(
                runtime,
                &mut messages,
                StopReason::Interrupted,
                tool_calls_made,
                tokens,
                &run_id,
            ));
        }

        let (content, tool_calls) = if let Some(ref cb) = callbacks {
            let n = messages.len();
            let bytes: usize = messages.iter().map(|m| m.content.len()).sum();
            debug!(
                step,
                msg_count = n,
                total_bytes = bytes,
                "stream_step start"
            );
            let result = {
                let cancel = cancel_token.clone();
                tokio::select! {
                    result = stream_step(runtime, &messages, cb.clone(), &cancel_token) => result,
                    _ = cancel.cancelled() => {
                        Ok((String::new(), Vec::new()))
                    }
                }
            };
            let result = result?;
            debug!(
                step,
                content_len = result.0.len(),
                tool_call_count = result.1.len(),
                "stream_step done"
            );
            result
        } else {
            let result = {
                let cancel = cancel_token.clone();
                tokio::select! {
                    result = sync_step(runtime, &messages, &cancel_token) => result,
                    _ = cancel.cancelled() => {
                        Ok((String::new(), Vec::new()))
                    }
                }
            };
            result?
        };

        // The cancellation branch of the select above intentionally drops the
        // in-flight provider future and yields an empty step result. Re-check
        // before finalization so that empty content cannot be misclassified as
        // a successful model answer.
        if cancel_token.is_cancelled() {
            let tokens = estimate_total_tokens(&messages);
            return Ok(finish_loop_result(
                runtime,
                &mut messages,
                StopReason::Interrupted,
                tool_calls_made,
                tokens,
                &run_id,
            ));
        }

        if tool_calls.is_empty() {
            // Harness: pre-finalize checks
            let mut needs_finalize = true;

            // Check verification gate (anti-fake-completion)
            let v_actions = crate::harness::verification::gate::check(
                &runtime.harness_state.verification,
                &content,
            );
            for action in &v_actions {
                if let crate::harness::action::HarnessAction::BlockFinalize { feedback } = action {
                    let text = crate::harness::render::render_feedback(feedback);
                    messages.push(Message::system(text));
                    needs_finalize = false;
                    break;
                }
            }

            // Check unresolved architecture violations (only fatal, current-run)
            if needs_finalize {
                let blocking_count = runtime
                    .harness_state
                    .architecture
                    .violations
                    .iter()
                    .filter(|v| {
                        v.status == crate::harness::state::ViolationStatus::Open
                            && v.severity == crate::harness::action::Severity::Fatal
                            && v.introduced_this_run
                    })
                    .count();
                if blocking_count > 0 {
                    let text = crate::harness::render::render_feedback(
                        &crate::harness::action::HarnessFeedback {
                            source: crate::harness::action::FeedbackSource::Architecture,
                            severity: crate::harness::action::Severity::Fatal,
                            rule_id: "arch/unresolved_violations".into(),
                            message: format!("还有 {} 个致命架构违规未解决。", blocking_count),
                            suggestion: "请先修复架构违规问题。".into(),
                            evidence: None,
                        },
                    );
                    messages.push(Message::system(text));
                    needs_finalize = false;
                }
            }
            if needs_finalize {
                record_trace(
                    runtime,
                    HarnessEvent::FinalClaim {
                        text: content.clone(),
                    },
                );
                messages.push(Message::assistant(content));
                let tokens = estimate_total_tokens(&messages);
                return Ok(finish_loop_result(
                    runtime,
                    &mut messages,
                    StopReason::Finished,
                    tool_calls_made,
                    tokens,
                    &run_id,
                ));
            }
            continue;
        }

        messages.push(Message::assistant_with_tools(content, tool_calls.clone()));

        for tc in &tool_calls {
            info!(tool = %tc.function.name, "执行中...");
            tool_calls_made += 1;
            let args_hash = simple_hash(&tc.function.arguments);

            // Per-tool guard. This is intentionally separate from duplicate
            // argument detection: several distinct read_file calls in one
            // parallel inspection step are legitimate and count separately.
            let count = tool_call_counts
                .entry(tc.function.name.clone())
                .or_insert(0);
            *count += 1;
            if *count > runtime.budget.per_tool_limit {
                warn!(tool = %tc.function.name, total = *count, "tool called too many times, skipping");
                let msg = format!(
                    "[系统：工具 {tool} 已被调用 {count} 次，跳过本次调用，请换用其他方法。]",
                    tool = tc.function.name,
                    count = *count
                );
                // Keep the assistant tool-call chain valid even when the host
                // refuses execution. A plain assistant message here leaves a
                // dangling tool_call_id and can make providers retry forever.
                messages.push(Message::tool_result(&tc.id, msg));
                messages.push(Message::system(format!(
                    "工具 {} 已达到单工具调用上限。不要再次调用它；请基于已有证据给出最终答复，或明确说明证据不足。",
                    tc.function.name
                )));
                consecutive_tool_failures += 1;
                continue;
            }

            // Recovery: track file-read progress signal (set before any `continue`)
            if matches!(tc.function.name.as_str(), "read" | "glob" | "grep" | "bash") {
                step_had_file_read = true;
                // File read may be a workspace file: resolve path from args if possible
                if tc.function.name == "read" {
                    if let Ok(val) =
                        serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                    {
                        if let Some(path_str) = val
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .or_else(|| val.get("path").and_then(|v| v.as_str()))
                        {
                            record_trace(
                                runtime,
                                HarnessEvent::FileRead {
                                    path: PathBuf::from(path_str),
                                },
                            );
                        }
                    }
                }
            }

            // Harness: pre-tool checks
            {
                if let crate::harness::action::HarnessAction::BlockTool { feedback } =
                    crate::harness::tool::loop_guard::check_duplicate(
                        &mut runtime.harness_state.tool_loop,
                        &tc.function.name,
                        &args_hash,
                    )
                {
                    let text = crate::harness::render::render_feedback(&feedback);
                    messages.push(Message::tool_result(
                        &tc.id,
                        format!("[Harness 拦截] {}", text),
                    ));
                    continue;
                }
            }

            // ── Checkpoint before tool execution ──────────────────────
            // Save the full agent state so a crash during the tool call
            // can be recovered.  The checkpoint is overwritten on each
            // tool call so only the latest one matters.
            if runtime.profile.saves_checkpoint() {
                if let Some(ref cs) = runtime.checkpoint_store {
                    let cp = AgentCheckpoint {
                        run_id: run_id.clone(),
                        step,
                        tool_calls_made,
                        consecutive_failures: consecutive_tool_failures,
                        tool_call_counts: tool_call_counts.clone(),
                        messages: messages.clone(),
                        created_at: 0,
                    };
                    if let Err(e) = cs.save(&cp, true) {
                        warn!(error = %e, "failed to save agent checkpoint");
                    }
                }
            }

            let executor = crate::tool::ToolExecutor::with_policy(
                &runtime.registry,
                crate::tool::ToolExecutionPolicy {
                    timeout: runtime.budget.tool_timeout,
                    ..Default::default()
                },
            );
            let workspace_root = std::env::current_dir().unwrap_or_default();
            let shell_mutations_before = (tc.function.name == "shell")
                .then(|| {
                    crate::harness::tool::transaction::workspace_mutation_snapshot(&workspace_root)
                })
                .flatten();

            // ── Dispatch action via ActionRuntime ──────────────────────
            // Handles: idempotency → start callback → tool execution →
            // interruption/unknown-outcome → auth wait → done callback
            let action_result = crate::action::dispatch_with(
                crate::action::ActionRequest::new(
                    &tc.id,
                    &tc.function.name,
                    &tc.function.arguments,
                    step,
                    tool_calls_made,
                ),
                &executor,
                &journal,
                &cancel_token,
                callbacks.as_deref(),
            )
            .await;

            // ── Idempotent skip ───────────────────────────────────────
            if action_result.was_idempotent_skip {
                messages.push(Message::tool_result(&tc.id, action_result.observation));
                continue;
            }

            // ── Unknown outcome (interrupted side-effecting tool) ──────
            if action_result.status == crate::action::ActionStatus::UnknownOutcome {
                messages.push(Message::tool_result(&tc.id, action_result.observation));
                // Not counted as consecutive failure
                continue;
            }

            // ── Auth Required (dispatch already waited for approval) ──
            if action_result.output_status == ToolStatus::AuthRequired {
                messages.push(Message::tool_result(&tc.id, action_result.observation));

                if let Some(ref tool_output) = action_result.tool_output {
                    crate::harness::recovery::record_signal(
                        &mut runtime.harness_state.recovery,
                        crate::harness::recovery::policy::RecoverySignal::permission_blocked(
                            tool_output
                                .error
                                .clone()
                                .unwrap_or_else(|| "tool requires approval".into()),
                        ),
                    );
                    let recovery_feedback = crate::harness::recovery::check(
                        &mut runtime.harness_state.recovery,
                        false,
                        false,
                        false,
                        harness_step,
                    );
                    emit_recovery_feedback(runtime, &mut messages, recovery_feedback);
                }

                if action_result.status == crate::action::ActionStatus::Cancelled {
                    messages.push(Message::system(
                        "用户已中断当前操作，之前的审批请求已取消。",
                    ));
                }
                continue;
            }

            // ── Trace recording ───────────────────────────────────────
            let tool_success = matches!(action_result.output_status, ToolStatus::Success);
            let exit_code = action_result.tool_output.as_ref().and_then(tool_exit_code);
            record_trace(
                runtime,
                HarnessEvent::ToolCall {
                    step: harness_step,
                    tool_name: tc.function.name.clone(),
                    args_hash: args_hash.clone(),
                    success: tool_success,
                },
            );
            record_verification_trace(runtime, tc, tool_success, exit_code, harness_step);

            // ── Budget check ──────────────────────────────────────────
            if tool_calls_made >= runtime.budget.max_tool_calls as usize {
                warn!(
                    made = tool_calls_made,
                    limit = runtime.budget.max_tool_calls,
                    "tool call budget exhausted"
                );
                let tokens = estimate_total_tokens(&messages);
                return Ok(finish_loop_result(
                    runtime,
                    &mut messages,
                    StopReason::MaxToolCallsReached,
                    tool_calls_made,
                    tokens,
                    &run_id,
                ));
            }

            // ── Push result message ───────────────────────────────────
            messages.push(Message::tool_result(&tc.id, action_result.observation));

            // ── Failure tracking ──────────────────────────────────────
            let was_error = matches!(
                action_result.status,
                crate::action::ActionStatus::Failed | crate::action::ActionStatus::TimedOut
            );
            if action_result.status == crate::action::ActionStatus::Completed {
                consecutive_tool_failures = 0;
                info!(tool = %tc.function.name, "✓");
            } else if was_error {
                consecutive_tool_failures += 1;
                warn!(tool = %tc.function.name, error = ?action_result.output_error, consec = consecutive_tool_failures, "✗");
                if consecutive_tool_failures >= 3 {
                    let tokens = estimate_total_tokens(&messages);
                    return Ok(finish_loop_result(
                        runtime,
                        &mut messages,
                        StopReason::ToolFailurePersistent,
                        tool_calls_made,
                        tokens,
                        &run_id,
                    ));
                }
            }

            // Harness: post-tool checks
            let termination = action_result.tool_termination;
            let output = action_result
                .tool_output
                .clone()
                .unwrap_or_else(|| ToolOutput::error(String::from("missing")));
            {
                // Update last_edit_step for mutation tools and shell mutations
                let shell_has_new_mutations = if tc.function.name == "shell" {
                    let after = crate::harness::tool::transaction::workspace_mutation_snapshot(
                        &workspace_root,
                    );
                    matches!((&shell_mutations_before, &after), (Some(before), Some(after)) if before != after)
                } else {
                    false
                };
                // A rejected direct write is not an edit. Recording it as one
                // creates false ownership violations and cross-worker
                // conflicts even though the sandbox preserved the workspace.
                // A shell command is different: a non-zero command can still
                // leave real filesystem changes, so trust the before/after
                // snapshot for that path.
                let direct_mutation_succeeded =
                    tool_success && matches!(tc.function.name.as_str(), "edit" | "write_file");
                let is_mutation = direct_mutation_succeeded || shell_has_new_mutations;
                if is_mutation {
                    runtime.harness_state.verification.last_edit_step = harness_step;

                    // Capture diff for mutation tools
                    let root = std::env::current_dir().unwrap_or_default();

                    let path_from_args: Option<std::path::PathBuf> =
                        serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                            .ok()
                            .and_then(|val| {
                                val.get("file_path")
                                    .or_else(|| val.get("path"))
                                    .and_then(|v| v.as_str())
                                    .map(std::path::PathBuf::from)
                            });

                    let diff: Option<String> = if let Some(ref path) = path_from_args {
                        Some(crate::harness::tool::transaction::safe_capture_diff(
                            &root, path,
                        ))
                    } else if tc.function.name == "shell" {
                        crate::harness::tool::transaction::capture_all_diff(&root)
                    } else {
                        None
                    };

                    record_trace(
                        runtime,
                        HarnessEvent::FileEdit {
                            path: path_from_args.unwrap_or_default(),
                            diff_hash: diff
                                .as_ref()
                                .map(|d| {
                                    use std::hash::{Hash, Hasher};
                                    let mut hasher =
                                        std::collections::hash_map::DefaultHasher::new();
                                    d.hash(&mut hasher);
                                    hasher.finish().to_string()
                                })
                                .unwrap_or_default(),
                            diff,
                        },
                    );
                }

                // Save previous phase before inference (for next pre_turn)
                runtime.harness_state.previous_phase = runtime.harness_state.phase;

                // Phase inference
                if let Some(new_phase) =
                    crate::harness::phase::infer_phase_from_event(&tc.function.name, tool_success)
                {
                    if new_phase != runtime.harness_state.phase {
                        record_trace(
                            runtime,
                            HarnessEvent::PhaseTransition {
                                from: format!("{:?}", runtime.harness_state.phase),
                                to: format!("{new_phase:?}"),
                            },
                        );
                    }
                    runtime.harness_state.phase = new_phase;
                }

                // Verification ledger
                let verification_input =
                    verification_command_text(tc).unwrap_or_else(|| tc.function.arguments.clone());
                crate::harness::verification::ledger::record(
                    &mut runtime.harness_state.verification,
                    &tc.function.name,
                    &verification_input,
                    exit_code,
                    harness_step,
                );

                if termination == ToolTermination::TimedOut {
                    crate::harness::recovery::record_signal(
                        &mut runtime.harness_state.recovery,
                        crate::harness::recovery::policy::RecoverySignal::tool_timeout(
                            &tc.function.name,
                            runtime.budget.tool_timeout,
                        ),
                    );
                }
                if is_verification_tool_call(tc) && !tool_success {
                    crate::harness::recovery::record_signal(
                        &mut runtime.harness_state.recovery,
                        crate::harness::recovery::policy::RecoverySignal::verification_failed(
                            output
                                .error
                                .as_deref()
                                .unwrap_or("verification command failed"),
                        ),
                    );
                }

                // Recovery: failure fingerprint
                if !tool_success {
                    let err_text = output.error.as_deref().unwrap_or("unknown error");
                    crate::harness::recovery::fingerprint::record(
                        &mut runtime.harness_state.recovery,
                        &tc.function.name,
                        &tc.function.arguments,
                        err_text,
                        harness_step,
                    );
                }

                // Recovery: track edit/test progress and patch history
                if tool_success && is_mutation {
                    step_had_successful_edit = true;
                    runtime
                        .harness_state
                        .recovery
                        .patch_history
                        .record(&tc.function.arguments);
                }
                if tool_success && is_verification_tool_call(tc) {
                    step_had_successful_test = true;
                }

                // Architecture: re-index + rule evaluation on mutation
                if is_mutation {
                    // Lazy-build the project index on first mutation
                    let root = std::env::current_dir().unwrap_or_default();
                    if runtime.harness_state.architecture.index.is_none() {
                        let mut idx =
                            crate::harness::architecture::index::ProjectIndex::new(root.clone());
                        idx.scan_dir(&root);
                        runtime.harness_state.architecture.index = Some(idx);
                    }

                    let (arch_traces, arch_impact_msgs, arch_semantic_feedback) = {
                        let mut traces = Vec::new();
                        let mut impact_msgs = Vec::new();
                        let mut semantic_feedback = Vec::new();

                        if let Some(ref mut idx) = runtime.harness_state.architecture.index {
                            let changed_paths =
                                changed_paths_from_tool_args(&tc.function.arguments);
                            let mut changes = Vec::new();

                            for actual_path in changed_paths {
                                if !actual_path.exists() {
                                    continue;
                                }
                                if let Ok(content) = std::fs::read_to_string(&actual_path) {
                                    let old_index = idx.files.get(&actual_path).cloned();
                                    let new_index =
                                        crate::harness::architecture::parser::parse_file(
                                            &actual_path,
                                            &content,
                                        );
                                    changes.extend(
                                        crate::harness::architecture::diff::compute_diff(
                                            old_index.as_ref(),
                                            &new_index,
                                        ),
                                    );
                                    let items = new_index.items.clone();
                                    idx.symbols.update_file(&actual_path, &items);
                                    idx.files.insert(actual_path, new_index);
                                }
                            }

                            // Shell commands may mutate files without exposing paths in tool args.
                            if changes.is_empty() && tc.function.name == "shell" {
                                idx.scan_dir(&root);
                            }

                            let layers = crate::harness::architecture::layer::LayerGraph::default();
                            let rules = crate::harness::architecture::config::default_rules();
                            let (feedback, new_violations) =
                                crate::harness::architecture::rules::evaluate_rules(
                                    &rules,
                                    idx,
                                    &layers,
                                    &changes,
                                    &runtime.harness_state.architecture.violations,
                                );
                            for v in &new_violations {
                                traces.push(HarnessEvent::ArchitectureViolation {
                                    rule_id: v.key.rule_id.clone(),
                                    severity: format!("{:?}", v.severity),
                                });
                            }
                            for v in new_violations {
                                runtime.harness_state.architecture.violations.push(v);
                            }
                            for fb in &feedback {
                                if fb.severity == crate::harness::action::Severity::Fatal {
                                    let text = crate::harness::render::render_feedback(fb);
                                    messages.push(Message::system(text));
                                }
                            }

                            // Architecture depth: impact + semantic
                            // NOTE: api.rs::check_compatibility is NOT called here because
                            // FileIndex does not store source content, so we cannot diff
                            // old vs new source text. Add it when FileIndex gains a content field.
                            if !changes.is_empty() {
                                let impact =
                                    crate::harness::architecture::impact::analyze(&changes, idx);
                                impact_msgs = impact;
                                for file in idx.files.keys() {
                                    if file.exists() {
                                        if let Ok(content) = std::fs::read_to_string(file) {
                                            let sf = crate::harness::architecture::semantic::check_semantics(
                                                &content, file, &rules,
                                            );
                                            semantic_feedback.extend(sf);
                                        }
                                    }
                                }
                            }

                            // Emit architecture analysis event
                            if let Some(ref eb) = runtime.event_bus {
                                let file_count = idx.files.len();
                                let feedback_count = feedback.len();
                                let summary = if feedback.is_empty() {
                                    format!("分析 {} 个文件，未发现架构违规", file_count)
                                } else {
                                    format!(
                                        "分析 {} 个文件，发现 {} 条架构反馈",
                                        file_count, feedback_count
                                    )
                                };
                                let components: Vec<crate::event::ArchitectureComponent> = idx
                                    .files
                                    .iter()
                                    .take(30)
                                    .map(|(path, file_idx)| {
                                        let desc = if file_idx.items.is_empty() {
                                            path.display().to_string()
                                        } else {
                                            format!(
                                                "{} — {} 个符号",
                                                path.display(),
                                                file_idx.items.len()
                                            )
                                        };
                                        crate::event::ArchitectureComponent {
                                            name: path
                                                .file_name()
                                                .map(|n| n.to_string_lossy().to_string())
                                                .unwrap_or_default(),
                                            description: desc,
                                        }
                                    })
                                    .collect();
                                eb.publish(Event::Harness(HarnessUiEvent::ArchitectureAnalysis {
                                    summary,
                                    components,
                                }));
                            }
                        }
                        (traces, impact_msgs, semantic_feedback)
                    };
                    for ev in arch_traces {
                        record_trace(runtime, ev);
                    }
                    for msg in &arch_impact_msgs {
                        info!("arch impact: {msg}");
                    }
                    for fb in &arch_semantic_feedback {
                        let text = crate::harness::render::render_feedback(fb);
                        info!("arch semantic: {text}");
                    }
                }
            }
        }

        // Recovery: check no-progress, repeated failures, repeated patches
        {
            let recovery_feedback = crate::harness::recovery::check(
                &mut runtime.harness_state.recovery,
                step_had_file_read,
                step_had_successful_edit,
                step_had_successful_test,
                harness_step,
            );
            emit_recovery_feedback(runtime, &mut messages, recovery_feedback);
        }
    }

    warn!(steps = runtime.budget.max_steps, "max steps reached");
    let tokens = estimate_total_tokens(&messages);
    Ok(finish_loop_result(
        runtime,
        &mut messages,
        StopReason::MaxStepsReached,
        tool_calls_made,
        tokens,
        &run_id,
    ))
}

fn close_unresolved_tool_calls(messages: &mut Vec<Message>) {
    let resolved_ids: std::collections::HashSet<String> = messages
        .iter()
        .filter_map(|message| message.tool_call_id.clone())
        .collect();
    let unresolved = messages
        .iter()
        .flat_map(|message| message.tool_calls.iter().flatten())
        .filter(|call| !resolved_ids.contains(&call.id))
        .map(|call| (call.id.clone(), call.function.name.clone()))
        .collect::<Vec<_>>();

    for (call_id, tool_name) in unresolved {
        messages.push(Message::tool_result(
            call_id,
            format!(
                "<observation tool=\"{tool_name}\" status=\"unknown_effect\">进程在工具结果持久化前终止；实际执行状态未知，必须先对账再决定是否重试。</observation>"
            ),
        ));
    }
}

pub(crate) async fn run_agent_with_context(
    runtime: &mut AgentRuntime,
    context: ContextPack,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
) -> anyhow::Result<LoopResult> {
    let context_desc = format!(
        "evidence={}, recent={}, input_len={}",
        context.evidence.len(),
        context.recent.len(),
        context.input.len(),
    );
    let messages = context.into_llm_messages();
    record_trace(
        runtime,
        HarnessEvent::ContextIncluded {
            description: context_desc,
            estimated_tokens: 0,
        },
    );
    run_agent(runtime, messages, callbacks, source, cancel_token).await
}

fn finish_loop_result(
    runtime: &mut AgentRuntime,
    messages: &mut Vec<Message>,
    stop_reason: StopReason,
    tool_calls_made: usize,
    estimated_tokens: usize,
    run_id: &str,
) -> LoopResult {
    let outcome = derive_run_outcome(stop_reason, &runtime.harness_state.verification);
    // Only delete checkpoint for clean finishes. For interrupted/failed/
    // blocked/budget-exhausted the checkpoint is preserved so the user
    // can recover or re-examine the state.
    if matches!(stop_reason, StopReason::Finished) {
        if let Some(ref cs) = runtime.checkpoint_store {
            if let Err(e) = cs.delete_run(run_id) {
                warn!(error = %e, "failed to delete agent checkpoint on finish");
            }
        }
    }
    record_trace(
        runtime,
        HarnessEvent::RunCompleted {
            timestamp: trace_timestamp(),
            total_steps: tool_calls_made as u32,
            outcome: format!("{stop_reason:?}"),
        },
    );
    LoopResult {
        messages: std::mem::take(messages),
        stop_reason,
        outcome,
        tool_calls_made,
        estimated_tokens,
        trace_events: runtime.harness_state.trace.events.clone(),
    }
}

fn derive_run_outcome(
    stop_reason: StopReason,
    verification: &crate::harness::state::VerificationState,
) -> RunOutcome {
    if stop_reason != StopReason::Finished {
        return RunOutcome::from(stop_reason);
    }

    let Some(success) = verification.last_success.as_ref() else {
        return RunOutcome::CompletedUnverified;
    };
    let failure_is_newer = verification
        .last_failure
        .as_ref()
        .is_some_and(|failure| failure.step > success.step);
    let verification_is_fresh = success.success
        && success.exit_code == Some(0)
        && success.step > verification.last_edit_step
        && !failure_is_newer;

    if verification_is_fresh {
        RunOutcome::CompletedVerified
    } else {
        RunOutcome::CompletedUnverified
    }
}

fn record_trace(runtime: &mut AgentRuntime, event: HarnessEvent) {
    runtime.harness_state.trace.events.push(event);
}

fn emit_recovery_feedback(
    runtime: &mut AgentRuntime,
    messages: &mut Vec<Message>,
    recovery_feedback: Vec<crate::harness::action::HarnessFeedback>,
) {
    for fb in &recovery_feedback {
        let text = crate::harness::render::render_feedback(fb);
        messages.push(Message::system(text));
        record_trace(
            runtime,
            HarnessEvent::RecoveryFeedback {
                rule_id: fb.rule_id.clone(),
                message: fb.message.clone(),
            },
        );
    }
}

fn record_verification_trace(
    runtime: &mut AgentRuntime,
    tc: &ToolCall,
    tool_success: bool,
    exit_code: Option<i32>,
    step: u32,
) {
    if is_verification_tool_call(tc) {
        record_trace(
            runtime,
            HarnessEvent::Verification {
                command: verification_command_text(tc)
                    .unwrap_or_else(|| tc.function.arguments.clone()),
                success: tool_success,
                exit_code,
                step,
            },
        );
    }
}

fn is_verification_tool_call(tc: &ToolCall) -> bool {
    if tc.function.name == "self_test" {
        return true;
    }
    verification_command_text(tc)
        .map(|command| {
            crate::harness::verification::classify::classify_command(&command)
                != crate::harness::verification::classify::VerificationType::Unknown
        })
        .unwrap_or(false)
}

fn verification_command_text(tc: &ToolCall) -> Option<String> {
    if tc.function.name == "shell" {
        serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("command")
                    .and_then(|command| command.as_str())
                    .map(str::to_string)
            })
    } else {
        None
    }
}

fn tool_exit_code(output: &ToolOutput) -> Option<i32> {
    // Read real exit_code from tool data if available (e.g. ShellTool)
    if let Some(ref data) = output.data {
        if let Some(code) = data.get("exit_code").and_then(|c| c.as_i64()) {
            return Some(code as i32);
        }
    }
    match output.status {
        ToolStatus::Success => Some(0),
        ToolStatus::Error => Some(1),
        ToolStatus::AuthRequired => None,
    }
}

fn run_input(messages: &[Message], source: &str) -> String {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, crate::agent::llm::Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_else(|| source.to_string())
}

fn simple_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

fn trace_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn user_requested_verification(messages: &[Message]) -> bool {
    messages.iter().any(|m| {
        matches!(m.role, crate::agent::llm::Role::User)
            && crate::harness::verification::claim::requests_verification(&m.content)
    })
}

fn changed_paths_from_tool_args(arguments: &str) -> Vec<PathBuf> {
    let direct = PathBuf::from(arguments);
    if direct.exists() {
        return vec![direct];
    }

    let Ok(args) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for key in ["path", "file", "file_path", "target", "target_path"] {
        if let Some(path) = args.get(key).and_then(|p| p.as_str()) {
            paths.push(PathBuf::from(path));
        }
    }
    if let Some(items) = args.get("paths").and_then(|p| p.as_array()) {
        paths.extend(items.iter().filter_map(|p| p.as_str()).map(PathBuf::from));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::{
        ChatCompletionRequest, ChatCompletionResponse, FinalChoice, FunctionCall, LlmProvider,
        ScriptedProvider as OfflineScriptedProvider,
    };
    use crate::agent::AgentRuntime;
    use crate::tool::{default_registry, Tool, ToolRegistry};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[test]
    fn shell_json_command_is_classified_as_verification() {
        let tc = ToolCall {
            id: "call-1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "shell".into(),
                arguments: r#"{"command":"cargo test -p zhongshu-core"}"#.into(),
            },
        };

        assert_eq!(
            verification_command_text(&tc).as_deref(),
            Some("cargo test -p zhongshu-core")
        );
        assert!(is_verification_tool_call(&tc));
    }

    #[derive(Clone)]
    struct TraceTestProvider {
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl LlmProvider for TraceTestProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            let message = if *calls == 1 {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "call-1".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "self_test".into(),
                            arguments: "{}".into(),
                        },
                    }],
                )
            } else {
                Message::assistant("done")
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
            anyhow::bail!("streaming is not used in this test")
        }

        fn model_name(&self) -> &str {
            "trace-test"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    struct TraceTestTool;

    #[async_trait]
    impl Tool for TraceTestTool {
        fn name(&self) -> &str {
            "self_test"
        }

        fn description(&self) -> &str {
            "fake verification tool"
        }

        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(json!({"ok": true}))
        }
    }

    #[derive(Clone)]
    struct RejectedEditProvider {
        calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl LlmProvider for RejectedEditProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            let message = if *calls == 1 {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: "rejected-edit".into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: "edit".into(),
                            arguments: r#"{"path":"outside.txt","old_text":"a","new_text":"b"}"#
                                .into(),
                        },
                    }],
                )
            } else {
                Message::assistant("edit was rejected")
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
            anyhow::bail!("streaming is not used in this test")
        }

        fn model_name(&self) -> &str {
            "rejected-edit"
        }

        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    struct RejectedEditTool;

    #[async_trait]
    impl Tool for RejectedEditTool {
        fn name(&self) -> &str {
            "edit"
        }

        fn description(&self) -> &str {
            "always rejects an edit"
        }

        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{"path":{"type":"string"}}})
        }

        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::error("path is outside the sandbox scope")
        }
    }

    #[test]
    fn detects_user_verification_request() {
        let messages = vec![Message::user("please run tests before finalizing")];
        assert!(user_requested_verification(&messages));
    }

    #[tokio::test]
    async fn explicit_not_required_policy_overrides_embedded_verification_language() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![(
                "__text__".into(),
                "analysis report submitted".into(),
                true,
            )]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut runtime =
            AgentRuntime::new(provider, ToolRegistry::new(), "scripted", small_budget());

        let result = run_agent_with_verification_policy(
            &mut runtime,
            vec![Message::user(
                "analyze this task; another employee will run verification",
            )],
            None,
            "test",
            CancellationToken::new(),
            Some(false),
        )
        .await
        .expect("analysis role should finish without owning verification");

        assert_eq!(result.outcome, RunOutcome::CompletedUnverified);
        assert!(!runtime.harness_state.verification.required);
    }

    #[test]
    fn extracts_json_path_for_changed_file() {
        let paths = changed_paths_from_tool_args(r#"{"path":"src/lib.rs"}"#);
        assert_eq!(paths, vec![PathBuf::from("src/lib.rs")]);
    }

    #[tokio::test]
    async fn run_agent_records_minimal_trace_events() {
        let provider = TraceTestProvider {
            calls: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(TraceTestTool),
            "trace-test",
            AgentBudget {
                max_steps: 5,
                max_tool_calls: 5,
                per_tool_limit: 5,
                token_limit: 10_000,
                llm_timeout: Duration::from_secs(5),
                tool_timeout: Duration::from_secs(5),
            },
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("run checks")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        assert!(matches!(
            result.trace_events.first(),
            Some(HarnessEvent::RunStarted { .. })
        ));
        assert!(result.trace_events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolCall {
                tool_name,
                success: true,
                ..
            } if tool_name == "self_test"
        )));
        assert!(result.trace_events.iter().any(|event| matches!(
            event,
            HarnessEvent::Verification {
                success: true,
                step: 1,
                ..
            }
        )));
        assert!(result.trace_events.iter().any(|event| matches!(
            event,
            HarnessEvent::PhaseTransition { to, .. } if to == "Verify"
        )));
        assert!(matches!(
            result.trace_events.last(),
            Some(HarnessEvent::RunCompleted { .. })
        ));
        assert_eq!(runtime.harness_state.trace.events, result.trace_events);
    }

    #[tokio::test]
    async fn rejected_edit_is_not_recorded_as_a_file_mutation() {
        let provider = RejectedEditProvider {
            calls: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(RejectedEditTool),
            "rejected-edit",
            AgentBudget {
                max_steps: 3,
                max_tool_calls: 3,
                per_tool_limit: 3,
                token_limit: 10_000,
                llm_timeout: Duration::from_secs(5),
                tool_timeout: Duration::from_secs(5),
            },
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("attempt an out-of-scope edit")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(result.trace_events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolCall {
                tool_name,
                success: false,
                ..
            } if tool_name == "edit"
        )));
        assert!(!result
            .trace_events
            .iter()
            .any(|event| matches!(event, HarnessEvent::FileEdit { .. })));
        assert_eq!(runtime.harness_state.verification.last_edit_step, 0);
    }

    // ── Recovery loop tests ──

    #[tokio::test]
    async fn scripted_provider_runs_self_test_without_live_llm() {
        let mut runtime = AgentRuntime::new(
            OfflineScriptedProvider::new("offline-scripted"),
            default_registry(),
            "offline-scripted",
            AgentBudget {
                max_steps: 5,
                max_tool_calls: 5,
                per_tool_limit: 5,
                token_limit: 10_000,
                llm_timeout: Duration::from_secs(5),
                tool_timeout: Duration::from_secs(5),
            },
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("check chat coding proof path")],
            None,
            "offline-proof",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        assert_eq!(result.tool_calls_made, 1);
        assert!(result.trace_events.iter().any(|event| matches!(
            event,
            HarnessEvent::ToolCall {
                tool_name,
                success: true,
                ..
            } if tool_name == "self_test"
        )));
        assert!(result
            .messages
            .iter()
            .any(|message| message.content.contains("without a live LLM")));
    }

    struct NoopTool;

    #[async_trait]
    impl Tool for NoopTool {
        fn name(&self) -> &str {
            "noop"
        }
        fn description(&self) -> &str {
            "no-op tool for testing"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(json!({"ok": true}))
        }
    }

    struct PassingShellTool;

    #[async_trait]
    impl Tool for PassingShellTool {
        fn name(&self) -> &str {
            "shell"
        }
        fn description(&self) -> &str {
            "fake shell for verification-ledger testing"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{"command":{"type":"string"}}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::success(json!({"exit_code": 0, "stdout": "tests passed"}))
        }
    }

    struct SlowTool;

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &str {
            "slow"
        }
        fn description(&self) -> &str {
            "slow tool for timeout recovery testing"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ToolOutput::success(json!({"ok": true}))
        }
    }

    struct AuthTool;

    #[async_trait]
    impl Tool for AuthTool {
        fn name(&self) -> &str {
            "auth_tool"
        }
        fn description(&self) -> &str {
            "authorization tool for recovery testing"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::auth_required("auth_tool", "auth_tool run")
        }
    }

    struct FailingSelfTestTool;

    #[async_trait]
    impl Tool for FailingSelfTestTool {
        fn name(&self) -> &str {
            "self_test"
        }
        fn description(&self) -> &str {
            "failing verification tool for recovery testing"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{}})
        }
        async fn execute(&self, _arguments: &serde_json::Value) -> ToolOutput {
            ToolOutput::error("verification failed")
        }
    }

    /// A provider that follows a scripted sequence of tool-call / text responses.
    #[derive(Clone)]
    struct ScriptedProvider {
        /// Each entry is `(tool_name, tool_args, succeed)` for a tool-call response,
        /// or `("__text__", text, true)` for a plain-text response.
        script: Arc<Vec<(String, String, bool)>>,
        idx: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn chat(
            &self,
            _request: ChatCompletionRequest,
        ) -> anyhow::Result<ChatCompletionResponse> {
            let mut idx = self.idx.lock().unwrap();
            if *idx >= self.script.len() {
                anyhow::bail!("script exhausted (idx={})", *idx);
            }
            let entry = self.script[*idx].clone();
            *idx += 1;

            let message = if entry.0 == "__text__" {
                Message::assistant(entry.1)
            } else {
                Message::assistant_with_tools(
                    "",
                    vec![ToolCall {
                        id: format!("call-{}", *idx),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: entry.0.clone(),
                            arguments: entry.1.clone(),
                        },
                    }],
                )
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
            anyhow::bail!("streaming not used in recovery tests")
        }

        fn model_name(&self) -> &str {
            "scripted"
        }
        fn change_model(&self, _model: &str) -> Arc<dyn LlmProvider> {
            Arc::new(self.clone())
        }
    }

    fn small_budget() -> AgentBudget {
        AgentBudget {
            max_steps: 20,
            max_tool_calls: 20,
            per_tool_limit: 20,
            token_limit: 10_000,
            llm_timeout: Duration::from_secs(5),
            tool_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn per_tool_limit_closes_rejected_tool_call_before_final_answer() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                (
                    "__text__".into(),
                    "final from existing evidence".into(),
                    true,
                ),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut budget = small_budget();
        budget.per_tool_limit = 1;
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(NoopTool),
            "scripted",
            budget,
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("inspect once, then finalize")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .expect("run");

        assert_eq!(result.stop_reason, StopReason::Finished);
        assert!(result.messages.iter().any(|message| {
            message.tool_call_id.as_deref() == Some("call-2")
                && message.content.contains("跳过本次调用")
        }));
        assert!(result
            .messages
            .iter()
            .any(|message| { message.content.contains("不要再次调用它") }));
        assert_eq!(
            result
                .messages
                .last()
                .map(|message| message.content.as_str()),
            Some("final from existing evidence")
        );
    }

    #[tokio::test]
    async fn shell_json_command_records_fresh_verification_in_ledger() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                (
                    "shell".into(),
                    r#"{"command":"cargo test 2>&1"}"#.into(),
                    true,
                ),
                ("__text__".into(), "tests passed; final review".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(PassingShellTool),
            "scripted",
            small_budget(),
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("review this and run tests")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .expect("run");

        assert_eq!(result.stop_reason, StopReason::Finished);
        assert_eq!(result.outcome, RunOutcome::CompletedVerified);
        let verification = runtime
            .harness_state
            .verification
            .last_success
            .as_ref()
            .expect("verification record");
        assert_eq!(verification.command, "cargo test 2>&1");
        assert_eq!(verification.exit_code, Some(0));
    }

    fn has_recovery_rule(result: &LoopResult, expected_rule: &str) -> bool {
        result.trace_events.iter().any(|event| {
            matches!(
                event,
                HarnessEvent::RecoveryFeedback { rule_id, .. } if rule_id == expected_rule
            )
        })
    }

    #[tokio::test]
    async fn recovery_no_progress_triggers_after_5_steps() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("__text__".into(), "done".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(NoopTool),
            "recovery-test",
            small_budget(),
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("do some work")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        // No-progress after 5 consecutive noop steps → recovery feedback emitted
        let recovery_events: Vec<_> = result
            .trace_events
            .iter()
            .filter(|e| matches!(e, HarnessEvent::RecoveryFeedback { .. }))
            .collect();
        assert!(
            !recovery_events.is_empty(),
            "expected recovery feedback events"
        );
        let has_no_progress = recovery_events.iter().any(|e| {
            matches!(e, HarnessEvent::RecoveryFeedback { message, .. } if message.contains("没有取得进展"))
        });
        assert!(
            has_no_progress,
            "expected no-progress hint in: {:?}",
            recovery_events
        );
    }

    #[tokio::test]
    async fn recovery_records_tool_timeout_signal() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("slow".into(), "{}".into(), true),
                ("__text__".into(), "done".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut budget = small_budget();
        budget.tool_timeout = Duration::from_millis(10);
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(SlowTool),
            "recovery-test",
            budget,
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("run a slow tool")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        assert!(has_recovery_rule(&result, "recovery/tool_timeout"));
    }

    #[tokio::test]
    async fn recovery_records_auth_required_signal() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("auth_tool".into(), "{}".into(), true),
                ("__text__".into(), "done".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(AuthTool),
            "recovery-test",
            small_budget(),
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("run auth tool")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        assert!(has_recovery_rule(&result, "recovery/permission_blocked"));
    }

    #[tokio::test]
    async fn recovery_records_verification_failure_signal() {
        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("self_test".into(), "{}".into(), true),
                ("__text__".into(), "not tested".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };
        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(FailingSelfTestTool),
            "recovery-test",
            small_budget(),
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("check status")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(result.stop_reason, StopReason::Finished));
        assert!(has_recovery_rule(&result, "recovery/verification_failed"));
    }

    #[tokio::test]
    async fn recovery_no_progress_resets_on_read() {
        struct ReadTool;
        #[async_trait]
        impl Tool for ReadTool {
            fn name(&self) -> &str {
                "read"
            }
            fn description(&self) -> &str {
                "reads a file"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{}})
            }
            async fn execute(&self, _: &serde_json::Value) -> ToolOutput {
                ToolOutput::success(json!([]))
            }
        }

        let provider = ScriptedProvider {
            script: Arc::new(vec![
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("read".into(), r#"{"path":"fake.rs"}"#.into(), true),
                ("noop".into(), "{}".into(), true),
                ("noop".into(), "{}".into(), true),
                ("__text__".into(), "done".into(), true),
            ]),
            idx: Arc::new(Mutex::new(0)),
        };

        let mut runtime = AgentRuntime::new(
            provider,
            ToolRegistry::new().register(NoopTool).register(ReadTool),
            "recovery-test",
            small_budget(),
        );

        let result = run_agent(
            &mut runtime,
            vec![Message::user("do some work")],
            None,
            "test",
            CancellationToken::new(),
        )
        .await
        .unwrap();

        let recovery_events: Vec<_> = result
            .trace_events
            .iter()
            .filter(|e| matches!(e, HarnessEvent::RecoveryFeedback { .. }))
            .collect();
        // With a read at step 3, 2 noops before and 1 after is not enough (need 5 consecutive)
        assert!(
            recovery_events.is_empty(),
            "read should reset no-progress counter"
        );
    }

    #[test]
    fn finished_outcome_requires_fresh_successful_verification() {
        let mut verification = crate::harness::HarnessState::new().verification;
        assert_eq!(
            derive_run_outcome(StopReason::Finished, &verification),
            RunOutcome::CompletedUnverified
        );

        crate::harness::verification::ledger::record(
            &mut verification,
            "shell",
            "cargo test",
            Some(0),
            2,
        );
        assert_eq!(
            derive_run_outcome(StopReason::Finished, &verification),
            RunOutcome::CompletedVerified
        );

        verification.last_edit_step = 2;
        assert_eq!(
            derive_run_outcome(StopReason::Finished, &verification),
            RunOutcome::CompletedUnverified
        );
    }

    #[test]
    fn checkpoint_recovery_closes_dangling_tool_calls() {
        let call = ToolCall {
            id: "call-1".into(),
            call_type: "function".into(),
            function: crate::agent::llm::FunctionCall {
                name: "write_file".into(),
                arguments: r#"{"path":"src/a.rs"}"#.into(),
            },
        };
        let mut messages = vec![Message::assistant_with_tools("", vec![call])];

        close_unresolved_tool_calls(&mut messages);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].tool_call_id.as_deref(), Some("call-1"));
        assert!(messages[1].content.contains("unknown_effect"));
    }
}

async fn sync_step(
    runtime: &AgentRuntime,
    messages: &[Message],
    _cancel_token: &CancellationToken,
) -> anyhow::Result<(String, Vec<ToolCall>)> {
    let response = tokio::time::timeout(
        runtime.budget.llm_timeout,
        runtime.provider.chat(build_request(runtime, messages)),
    )
    .await
    .map_err(|_elapsed| {
        tracing::warn!("LLM timeout after {:?}", runtime.budget.llm_timeout);
        anyhow::anyhow!("LLM timeout after {:?}", runtime.budget.llm_timeout)
    })?;
    let response = response.context("LLM chat failed")?;
    let choice = response
        .choices
        .into_iter()
        .next()
        .context("no choices in response")?;
    Ok((
        choice.message.content,
        choice.message.tool_calls.unwrap_or_default(),
    ))
}

async fn stream_step(
    runtime: &AgentRuntime,
    messages: &[Message],
    cb: Arc<AgentCallbacks>,
    _cancel_token: &CancellationToken,
) -> anyhow::Result<(String, Vec<ToolCall>)> {
    let content = Arc::new(std::sync::Mutex::new(String::new()));
    let tool_calls = Arc::new(std::sync::Mutex::new(Vec::<StreamToolCall>::new()));

    let c = content.clone();
    let tc = tool_calls.clone();

    tokio::time::timeout(
        runtime.budget.llm_timeout,
        runtime.provider.stream_chat(
            build_request(runtime, messages),
            Box::new(move |event| match event {
                StreamEvent::TextDelta(text) => {
                    (cb.on_text)(&text);
                    c.lock().unwrap().push_str(&text);
                }
                StreamEvent::ToolCallDelta {
                    index: _,
                    id: _,
                    name: _,
                    arguments: _,
                } => {
                    // Tool start notifications are deferred to the main loop
                    // where the full arguments are available, ensuring the
                    // ledger key matches between start and completion.
                }
                StreamEvent::Finished {
                    tool_calls: tcs, ..
                } => {
                    *tc.lock().unwrap() = tcs;
                }
            }),
        ),
    )
    .await
    .map_err(|_elapsed| {
        tracing::warn!("LLM timeout after {:?}", runtime.budget.llm_timeout);
        anyhow::anyhow!("LLM timeout after {:?}", runtime.budget.llm_timeout)
    })?
    .context("stream chat failed")?;

    let calls: Vec<ToolCall> = tool_calls
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .map(|tc| ToolCall {
            id: tc.id,
            call_type: "function".into(),
            function: crate::agent::llm::FunctionCall {
                name: tc.name,
                arguments: tc.arguments,
            },
        })
        .collect();

    let result_content = content.lock().unwrap().clone();
    Ok((result_content, calls))
}

fn build_request(
    runtime: &AgentRuntime,
    messages: &[Message],
) -> crate::agent::llm::ChatCompletionRequest {
    crate::agent::llm::ChatCompletionRequest {
        model: runtime.model.clone(),
        messages: messages.to_vec(),
        tools: Some(runtime.registry.as_tool_defs()),
        tool_choice: Some("auto".into()),
        stream: false,
        temperature: None,
        max_tokens: None,
        reasoning_effort: runtime.reasoning_effort.clone(),
    }
}

fn estimate_total_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|m| (m.content.len() as f64 / 3.5).ceil() as usize)
        .sum()
}
