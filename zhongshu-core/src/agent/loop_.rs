use std::sync::Arc;
use std::time::Duration;

use crate::agent::llm::{Message, StreamEvent, StreamToolCall, ToolCall};
use crate::agent::runtime::AgentRuntime;
use crate::core::context::ContextPack;
use crate::harness::trace::event::HarnessEvent;
use crate::tool::{infer_side_effect, SideEffect, ToolOutput, ToolStatus};
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
            max_tool_calls: 160,
            per_tool_limit: 40,
            token_limit: 500_000,
            llm_timeout: Duration::from_secs(240),
            tool_timeout: Duration::from_secs(120),
        }
    }

    pub fn coding_default() -> Self {
        Self {
            max_steps: 200,
            max_tool_calls: 400,
            per_tool_limit: 200,
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
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub messages: Vec<Message>,
    pub stop_reason: StopReason,
    pub tool_calls_made: usize,
    pub estimated_tokens: usize,
    pub trace_events: Vec<HarnessEvent>,
}

/// Streaming callbacks forwarded to the UI layer.
pub struct AgentCallbacks {
    pub on_text: Box<dyn Fn(&str) + Send + Sync>,
    pub on_tool_start: Box<dyn Fn(&str) + Send + Sync>,
    pub on_tool_done: Box<dyn Fn(&str, bool) + Send + Sync>,
    pub run_id: Uuid,
}

/// Run the full ReAct loop using the given runtime and initial messages.
///
/// The caller owns the message list (system prompt, profile context,
/// context engine, user input — everything) and passes it in.
/// After completion the final message list is returned inside `LoopResult`.
pub async fn run_agent(
    runtime: &mut AgentRuntime,
    mut messages: Vec<Message>,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
    cancel_token: CancellationToken,
) -> anyhow::Result<LoopResult> {
    if user_requested_verification(&messages) {
        runtime.harness_state.verification.required = true;
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

    for step in 0..runtime.budget.max_steps {
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
            ));
        }

        if cancel_token.is_cancelled() {
            break;
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
            ));
        }

        debug!(step, tokens = current_tokens, "agent loop iteration");

        if cancel_token.is_cancelled() {
            break;
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
                ));
            }
            continue;
        }

        messages.push(Message::assistant_with_tools(content, tool_calls.clone()));

        for tc in &tool_calls {
            info!(tool = %tc.function.name, "执行中...");
            tool_calls_made += 1;
            let args_hash = simple_hash(&tc.function.arguments);

            // Per-tool retry guard: if any single tool is called 5+ times
            // across the entire run, assume it's stuck and stop.
            let count = tool_call_counts
                .entry(tc.function.name.clone())
                .or_insert(0);
            *count += 1;
            if *count >= runtime.budget.per_tool_limit {
                warn!(tool = %tc.function.name, total = *count, "tool called too many times, skipping");
                let msg = format!(
                    "[系统：工具 {tool} 已被调用 {count} 次，跳过本次调用，请换用其他方法。]",
                    tool = tc.function.name,
                    count = *count
                );
                messages.push(Message::assistant(msg));
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

            let mut tool_timed_out = false;
            let output = {
                let cancel = cancel_token.clone();
                tokio::select! {
                    result = tokio::time::timeout(
                        runtime.budget.tool_timeout,
                        runtime.registry.execute(&tc.function.name, &tc.function.arguments),
                    ) => {
                        match result {
                            Ok(output) => output,
                            Err(_elapsed) => {
                                tracing::warn!(
                                    "Tool '{}' timed out after {:?}",
                                    tc.function.name,
                                    runtime.budget.tool_timeout
                                );
                                tool_timed_out = true;
                                ToolOutput::error(format!(
                                    "tool '{}' timed out after {:?}",
                                    tc.function.name, runtime.budget.tool_timeout
                                ))
                            }
                        }
                    }
                    _ = cancel.cancelled() => {
                        tool_timed_out = true;
                        ToolOutput::error("执行被用户中断")
                    }
                }
            };

            // Interruption handling: check cancellation status and side effect
            if cancel_token.is_cancelled() {
                let side_effect = runtime
                    .registry
                    .get(&tc.function.name)
                    .map(|t| t.spec().side_effect)
                    .unwrap_or_else(|| infer_side_effect(&tc.function.name));
                match side_effect {
                    SideEffect::ReadOnly => {
                        // Fast read-only: accept result if tool already completed
                    }
                    SideEffect::LocalWrite | SideEffect::SystemChange
                    | SideEffect::ExternalAction | SideEffect::Irreversible => {
                        // Force pause: tool output is NOT applied automatically
                        // Override output to signal interruption — loop continues but
                        // the LLM sees a failure observation.
                        let interrupted = ToolOutput::error(format!(
                            "{} 被用户中断。此操作未执行。",
                            tc.function.name
                        ));
                        // Interrupted tools do not count as tool failures for recovery
                        // (consecutive_failures is not incremented for these)
                        messages.push(Message::tool_result(
                            &tc.id,
                            interrupted.render_observation(&tc.function.name),
                        ));
                        continue;
                    }
                }
            }

            let tool_success = matches!(output.status, ToolStatus::Success);
            let exit_code = tool_exit_code(&output);
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

            // AuthRequired means the tool was not actually executed.
            // Wait for the user to approve/deny before continuing.
            if output.status == ToolStatus::AuthRequired {
                info!(tool = %tc.function.name, status = "auth_required");
                crate::harness::recovery::record_signal(
                    &mut runtime.harness_state.recovery,
                    crate::harness::recovery::policy::RecoverySignal::permission_blocked(
                        output
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
                crate::authority::set_pending_source(source);
                messages.push(Message::tool_result(
                    &tc.id,
                    output.render_observation(&tc.function.name),
                ));
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            if !crate::authority::is_pending() {
                                break;
                            }
                        }
                        _ = cancel_token.cancelled() => {
                            crate::authority::deny_pending(source);
                            messages.push(Message::system("用户已中断当前操作，之前的审批请求已取消。"));
                            break;
                        }
                    }
                }
                // User approved — update the stale auth_required observation so
                // the LLM sees "approved" rather than concluding the request was denied.
                if let Some(last) = messages.last_mut() {
                    last.content = format!("<observation tool=\"{}\" status=\"approved\">用户已授权，可以执行此工具。</observation>", tc.function.name);
                }
                continue;
            }

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
                ));
            }

            match output.status {
                ToolStatus::Success => {
                    consecutive_tool_failures = 0;
                    info!(tool = %tc.function.name, "✓");
                    messages.push(Message::tool_result(
                        &tc.id,
                        output.render_observation(&tc.function.name),
                    ));
                    if let Some(ref cb) = callbacks {
                        (cb.on_tool_done)(&tc.function.name, true);
                    }
                }
                ToolStatus::Error => {
                    consecutive_tool_failures += 1;
                    warn!(tool = %tc.function.name, error = ?output.error, consec = consecutive_tool_failures, "✗");
                    messages.push(Message::tool_result(
                        &tc.id,
                        output.render_observation(&tc.function.name),
                    ));
                    if let Some(ref cb) = callbacks {
                        (cb.on_tool_done)(&tc.function.name, false);
                    }

                    if consecutive_tool_failures >= 3 {
                        let tokens = estimate_total_tokens(&messages);
                        return Ok(finish_loop_result(
                            runtime,
                            &mut messages,
                            StopReason::ToolFailurePersistent,
                            tool_calls_made,
                            tokens,
                        ));
                    }
                }
                _ => {
                    messages.push(Message::tool_result(
                        &tc.id,
                        output.render_observation(&tc.function.name),
                    ));
                }
            }

            // Harness: post-tool checks
            {
                // Update last_edit_step for mutation tools and shell mutations
                let is_mutation = matches!(tc.function.name.as_str(), "edit" | "write_file")
                    || (tc.function.name == "shell"
                        && crate::harness::tool::transaction::workspace_has_mutations(
                            &std::env::current_dir().unwrap_or_default(),
                        ));
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
                crate::harness::verification::ledger::record(
                    &mut runtime.harness_state.verification,
                    &tc.function.name,
                    &tc.function.arguments,
                    exit_code,
                    harness_step,
                );

                if tool_timed_out {
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
    ))
}

pub async fn run_agent_with_context(
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
) -> LoopResult {
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
        tool_calls_made,
        estimated_tokens,
        trace_events: runtime.harness_state.trace.events.clone(),
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

    #[test]
    fn detects_user_verification_request() {
        let messages = vec![Message::user("please run tests before finalizing")];
        assert!(user_requested_verification(&messages));
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
                    name,
                    arguments: _,
                } => {
                    if let Some(n) = name {
                        (cb.on_tool_start)(&n);
                    }
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
