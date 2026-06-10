use std::sync::Arc;

use crate::agent::llm::{LlmProvider, Message, StreamEvent, StreamToolCall, ToolCall};
use crate::tool::{ToolRegistry, ToolStatus};
use anyhow::Context;
use tracing::{debug, info, warn};

const DEFAULT_MAX_STEPS: usize = 30;
const DEFAULT_MAX_TOOL_CALLS: usize = 20;
const DEFAULT_TOKEN_LIMIT: usize = 50_000;

#[derive(Debug, Clone)]
pub struct AgentBudget {
    pub max_steps: usize,
    pub max_tool_calls: usize,
    pub token_limit: usize,
}

impl Default for AgentBudget {
    fn default() -> Self {
        AgentBudget { max_steps: DEFAULT_MAX_STEPS, max_tool_calls: DEFAULT_MAX_TOOL_CALLS, token_limit: DEFAULT_TOKEN_LIMIT }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Finished,
    FinalAnswer,
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

pub struct AgentLoop {
    provider: Box<dyn LlmProvider>,
    registry: ToolRegistry,
    model: String,
    budget: AgentBudget,
    messages: Vec<Message>,
}

impl AgentLoop {
    pub fn new(provider: impl LlmProvider + 'static, registry: ToolRegistry, model: impl Into<String>) -> Self {
        AgentLoop { provider: Box::new(provider), registry, model: model.into(), budget: AgentBudget::default(), messages: Vec::new() }
    }

    pub fn with_budget(mut self, budget: AgentBudget) -> Self { self.budget = budget; self }
    pub fn with_system(mut self, system_prompt: impl Into<String>) -> Self { self.messages.insert(0, Message::system(system_prompt)); self }
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self { self.messages = messages; self }

    pub async fn run(self, user_input: impl Into<String>) -> anyhow::Result<LoopResult> {
        self.run_inner(user_input, None).await
    }

    pub async fn run_streaming(
        self,
        user_input: impl Into<String>,
        on_text: impl Fn(&str) + Send + Sync + 'static,
        on_tool_start: impl Fn(&str) + Send + Sync + 'static,
        on_tool_done: impl Fn(&str, bool) + Send + Sync + 'static,
    ) -> anyhow::Result<LoopResult> {
        let callbacks = Arc::new(StreamCallbacks {
            on_text: Box::new(on_text),
            on_tool_start: Box::new(on_tool_start),
            on_tool_done: Box::new(on_tool_done),
        });
        self.run_inner(user_input, Some(callbacks)).await
    }

    async fn run_inner(
        mut self,
        user_input: impl Into<String>,
        stream_cb: Option<Arc<StreamCallbacks>>,
    ) -> anyhow::Result<LoopResult> {
        self.messages.push(Message::user(user_input));

        let mut tool_calls_made = 0;
        let mut consecutive_tool_failures = 0u32;

        for step in 0..self.budget.max_steps {
            let current_tokens = estimate_total_tokens(&self.messages);

            if current_tokens > self.budget.token_limit {
                warn!(tokens = current_tokens, limit = self.budget.token_limit, "token budget exhausted");
                let tokens = estimate_total_tokens(&self.messages);
                return Ok(LoopResult {
                    messages: std::mem::take(&mut self.messages),
                    stop_reason: StopReason::BudgetExhausted { tokens: current_tokens, limit: self.budget.token_limit },
                    tool_calls_made, estimated_tokens: tokens,
                });
            }

            debug!(step, tokens = current_tokens, "agent loop iteration");

            let (content, tool_calls) = if let Some(ref cb) = stream_cb {
                self.stream_step(cb.clone()).await?
            } else {
                self.sync_step().await?
            };

            if is_final_answer(&content) && tool_calls.is_empty() {
                info!("<final_answer>");
                self.messages.push(Message::assistant(strip_final_answer(&content)));
                let tokens = estimate_total_tokens(&self.messages);
                return Ok(LoopResult {
                    messages: std::mem::take(&mut self.messages),
                    stop_reason: StopReason::FinalAnswer, tool_calls_made, estimated_tokens: tokens,
                });
            }

            if tool_calls.is_empty() {
                self.messages.push(Message::assistant(content));
                let tokens = estimate_total_tokens(&self.messages);
                return Ok(LoopResult {
                    messages: std::mem::take(&mut self.messages),
                    stop_reason: StopReason::Finished, tool_calls_made, estimated_tokens: tokens,
                });
            }

            self.messages.push(Message::assistant_with_tools(content, tool_calls.clone()));

            for tc in &tool_calls {
                info!(tool = %tc.function.name, "执行中...");
                tool_calls_made += 1;

                if tool_calls_made > self.budget.max_tool_calls {
                    warn!(made = tool_calls_made, limit = self.budget.max_tool_calls, "tool call budget exhausted");
                    let tokens = estimate_total_tokens(&self.messages);
                    return Ok(LoopResult {
                        messages: std::mem::take(&mut self.messages),
                        stop_reason: StopReason::MaxToolCallsReached, tool_calls_made, estimated_tokens: tokens,
                    });
                }

                let output = self.registry.execute(&tc.function.name, &tc.function.arguments).await;

                match output.status {
                    ToolStatus::Success => {
                        consecutive_tool_failures = 0;
                        info!(tool = %tc.function.name, "✓");
                        self.messages.push(Message::tool_result(&tc.id, output.render_observation(&tc.function.name)));
                        if let Some(ref cb) = stream_cb { (cb.on_tool_done)(&tc.function.name, true); }
                    }
                    ToolStatus::AuthRequired => {
                        // Not a tool failure — the LLM should ask the user for approval.
                        info!(tool = %tc.function.name, status = "auth_required");
                        self.messages.push(Message::tool_result(&tc.id, output.render_observation(&tc.function.name)));
                    }
                    ToolStatus::Error => {
                        consecutive_tool_failures += 1;
                        warn!(tool = %tc.function.name, error = ?output.error, consec = consecutive_tool_failures, "✗");
                        self.messages.push(Message::tool_result(&tc.id, output.render_observation(&tc.function.name)));
                        if let Some(ref cb) = stream_cb { (cb.on_tool_done)(&tc.function.name, false); }

                        if consecutive_tool_failures >= 3 {
                            let tokens = estimate_total_tokens(&self.messages);
                            return Ok(LoopResult {
                                messages: std::mem::take(&mut self.messages),
                                stop_reason: StopReason::ToolFailurePersistent,
                                tool_calls_made, estimated_tokens: tokens,
                            });
                        }
                    }
                }
            }
        }

        warn!(steps = self.budget.max_steps, "max steps reached");
        let tokens = estimate_total_tokens(&self.messages);
        Ok(LoopResult {
            messages: std::mem::take(&mut self.messages),
            stop_reason: StopReason::MaxStepsReached, tool_calls_made, estimated_tokens: tokens,
        })
    }

    async fn sync_step(&self) -> anyhow::Result<(String, Vec<ToolCall>)> {
        let response = self.provider.chat(self.build_request()).await.context("LLM chat failed")?;
        let choice = response.choices.into_iter().next().context("no choices in response")?;
        Ok((choice.message.content, choice.message.tool_calls.unwrap_or_default()))
    }

    async fn stream_step(&self, cb: Arc<StreamCallbacks>) -> anyhow::Result<(String, Vec<ToolCall>)> {
        let content = Arc::new(std::sync::Mutex::new(String::new()));
        let tool_calls = Arc::new(std::sync::Mutex::new(Vec::<StreamToolCall>::new()));

        let c = content.clone();
        let tc = tool_calls.clone();

        self.provider.stream_chat(
            self.build_request(),
            Box::new(move |event| {
                match event {
                    StreamEvent::TextDelta(text) => {
                        (cb.on_text)(&text);
                        c.lock().unwrap().push_str(&text);
                    }
                    StreamEvent::ToolCallDelta { index: _, id: _, name, arguments: _ } => {
                        if let Some(n) = name { (cb.on_tool_start)(&n); }
                    }
                    StreamEvent::Finished { tool_calls: tcs, .. } => {
                        *tc.lock().unwrap() = tcs;
                    }
                }
            }),
        ).await.context("stream chat failed")?;

        let calls: Vec<ToolCall> = tool_calls.lock().unwrap().clone()
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                call_type: "function".into(),
                function: crate::agent::llm::FunctionCall { name: tc.name, arguments: tc.arguments },
            }).collect();

        let result_content = content.lock().unwrap().clone();

        Ok((result_content, calls))
    }

    fn build_request(&self) -> crate::agent::llm::ChatCompletionRequest {
        crate::agent::llm::ChatCompletionRequest {
            model: self.model.clone(),
            messages: self.messages.clone(),
            tools: Some(self.registry.as_tool_defs()),
            tool_choice: Some("auto".into()),
            stream: false,
            temperature: None,
            max_tokens: None,
        }
    }
}

struct StreamCallbacks {
    on_text: Box<dyn Fn(&str) + Send + Sync>,
    on_tool_start: Box<dyn Fn(&str) + Send + Sync>,
    on_tool_done: Box<dyn Fn(&str, bool) + Send + Sync>,
}

fn is_final_answer(content: &str) -> bool {
    content.contains("<final_answer>") || content.contains("<final-answer>")
}

fn strip_final_answer(content: &str) -> String {
    content.replace("<final_answer>", "").replace("</final_answer>", "")
        .replace("<final-answer>", "").replace("</final-answer>", "").trim().to_string()
}

fn estimate_total_tokens(messages: &[Message]) -> usize {
    messages.iter().map(|m| (m.content.len() as f64 / 3.5).ceil() as usize).sum()
}
