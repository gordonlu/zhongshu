use std::sync::Arc;
use std::time::Duration;

use crate::agent::llm::{Message, StreamEvent, StreamToolCall, ToolCall};
use crate::agent::runtime::AgentRuntime;
use crate::core::context::ContextPack;
use crate::tool::{ToolOutput, ToolStatus};
use anyhow::Context;
use tracing::{debug, info, warn};

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
}

/// Streaming callbacks forwarded to the UI layer.
pub struct AgentCallbacks {
    pub on_text: Box<dyn Fn(&str) + Send + Sync>,
    pub on_tool_start: Box<dyn Fn(&str) + Send + Sync>,
    pub on_tool_done: Box<dyn Fn(&str, bool) + Send + Sync>,
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
) -> anyhow::Result<LoopResult> {
    let mut tool_calls_made = 0;
    let mut consecutive_tool_failures = 0u32;
    let mut tool_call_counts: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();

    for step in 0..runtime.budget.max_steps {
        if let Err(stop_reason) =
            check_budget(tool_calls_made, consecutive_tool_failures, &runtime.budget)
        {
            let tokens = estimate_total_tokens(&messages);
            return Ok(LoopResult {
                messages: std::mem::take(&mut messages),
                stop_reason,
                tool_calls_made,
                estimated_tokens: tokens,
            });
        }

        // Harness: pre-turn checks
        {
            let _ctx = crate::harness::context::HarnessContext {
                input: messages.iter().find_map(|m| {
                    if m.role == crate::agent::llm::Role::User { Some(m.content.clone()) } else { None }
                }).unwrap_or_default(),
                coding_mode: true,
                task_description: None,
                verification_required: false,
            };

            let phase_fb = crate::harness::phase::validate_transition(
                runtime.harness_state.phase,
                runtime.harness_state.phase,
            );
            for fb in phase_fb {
                let text = crate::harness::render::render_feedback(&fb);
                messages.push(Message::system(text));
            }

            let hints = crate::harness::architecture::feedback::generate_hints(
                &crate::harness::architecture::config::default_rules(),
                &crate::harness::architecture::layer::LayerGraph::default(),
            );
            for fb in hints {
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
            return Ok(LoopResult {
                messages: std::mem::take(&mut messages),
                stop_reason: StopReason::BudgetExhausted {
                    tokens: current_tokens,
                    limit: runtime.budget.token_limit,
                },
                tool_calls_made,
                estimated_tokens: tokens,
            });
        }

        debug!(step, tokens = current_tokens, "agent loop iteration");

        let (content, tool_calls) = if let Some(ref cb) = callbacks {
            let n = messages.len();
            let bytes: usize = messages.iter().map(|m| m.content.len()).sum();
            debug!(
                step,
                msg_count = n,
                total_bytes = bytes,
                "stream_step start"
            );
            let result = stream_step(runtime, &messages, cb.clone()).await?;
            debug!(
                step,
                content_len = result.0.len(),
                tool_call_count = result.1.len(),
                "stream_step done"
            );
            result
        } else {
            sync_step(runtime, &messages).await?
        };

        if tool_calls.is_empty() {
            // Harness: pre-finalize checks
            let mut needs_finalize = true;
            let finalize_actions = crate::harness::verification::gate::check(
                &runtime.harness_state.verification,
                &content,
            );
            for action in &finalize_actions {
                if let crate::harness::action::HarnessAction::BlockFinalize { feedback } = action {
                    let text = crate::harness::render::render_feedback(feedback);
                    messages.push(Message::system(text));
                    needs_finalize = false;
                    break;
                }
            }
            if needs_finalize {
                messages.push(Message::assistant(content));
                let tokens = estimate_total_tokens(&messages);
                return Ok(LoopResult {
                    messages: std::mem::take(&mut messages),
                    stop_reason: StopReason::Finished,
                    tool_calls_made,
                    estimated_tokens: tokens,
                });
            }
            continue;
        }

        messages.push(Message::assistant_with_tools(content, tool_calls.clone()));

        for tc in &tool_calls {
            info!(tool = %tc.function.name, "执行中...");
            tool_calls_made += 1;

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

            // Harness: pre-tool checks
            {
                let args_hash = {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    tc.function.arguments.hash(&mut hasher);
                    format!("{:x}", hasher.finish())
                };
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

            let output = match tokio::time::timeout(
                runtime.budget.tool_timeout,
                runtime
                    .registry
                    .execute(&tc.function.name, &tc.function.arguments),
            )
            .await
            {
                Ok(output) => output,
                Err(_elapsed) => {
                    tracing::warn!(
                        "Tool '{}' timed out after {:?}",
                        tc.function.name,
                        runtime.budget.tool_timeout
                    );
                    ToolOutput::error(format!(
                        "tool '{}' timed out after {:?}",
                        tc.function.name, runtime.budget.tool_timeout
                    ))
                }
            };

            // AuthRequired means the tool was not actually executed.
            // Wait for the user to approve/deny before continuing.
            if output.status == ToolStatus::AuthRequired {
                info!(tool = %tc.function.name, status = "auth_required");
                crate::authority::set_pending_source(source);
                messages.push(Message::tool_result(
                    &tc.id,
                    output.render_observation(&tc.function.name),
                ));
                while crate::authority::is_pending() {
                    tokio::time::sleep(Duration::from_millis(200)).await;
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
                return Ok(LoopResult {
                    messages: std::mem::take(&mut messages),
                    stop_reason: StopReason::MaxToolCallsReached,
                    tool_calls_made,
                    estimated_tokens: tokens,
                });
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
                        return Ok(LoopResult {
                            messages: std::mem::take(&mut messages),
                            stop_reason: StopReason::ToolFailurePersistent,
                            tool_calls_made,
                            estimated_tokens: tokens,
                        });
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
                let tool_success = matches!(output.status, ToolStatus::Success);
                // Phase inference
                if let Some(new_phase) = crate::harness::phase::infer_phase_from_event(
                    &tc.function.name, tool_success,
                ) {
                    runtime.harness_state.phase = new_phase;
                }

                // Verification ledger
                crate::harness::verification::ledger::record(
                    &mut runtime.harness_state.verification,
                    &tc.function.name,
                    &tc.function.arguments,
                    if tool_success { Some(0) } else { Some(1) },
                    step,
                );

                // Recovery: failure fingerprint
                if !tool_success {
                    let err_text = output.error.as_deref().unwrap_or("unknown error");
                    crate::harness::recovery::fingerprint::record(
                        &mut runtime.harness_state.recovery,
                        &tc.function.name,
                        &tc.function.arguments,
                        err_text,
                        step,
                    );
                }
            }
        }
    }

    warn!(steps = runtime.budget.max_steps, "max steps reached");
    let tokens = estimate_total_tokens(&messages);
    Ok(LoopResult {
        messages: std::mem::take(&mut messages),
        stop_reason: StopReason::MaxStepsReached,
        tool_calls_made,
        estimated_tokens: tokens,
    })
}

pub async fn run_agent_with_context(
    runtime: &mut AgentRuntime,
    context: ContextPack,
    callbacks: Option<Arc<AgentCallbacks>>,
    source: &str,
) -> anyhow::Result<LoopResult> {
    let messages = context.into_llm_messages();
    run_agent(runtime, messages, callbacks, source).await
}

async fn sync_step(
    runtime: &AgentRuntime,
    messages: &[Message],
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
