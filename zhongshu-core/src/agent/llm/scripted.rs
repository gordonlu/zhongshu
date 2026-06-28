use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, FinalChoice, FunctionCall, LlmProvider, Message,
    Role, StreamEvent, StreamToolCall, ToolCall,
};

/// Deterministic provider for offline proof runs.
///
/// The first turn requests a safe `self_test` tool when it is available. The
/// next turn returns a final answer. This exercises the same chat/coding/tool
/// event path as a live provider without touching the network.
#[derive(Debug, Clone)]
pub struct ScriptedProvider {
    model: String,
}

impl ScriptedProvider {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }

    fn should_call_tool(&self, request: &ChatCompletionRequest) -> bool {
        let has_tool_result = request
            .messages
            .iter()
            .any(|message| message.role == Role::Tool);
        if has_tool_result {
            return false;
        }

        request
            .tools
            .as_ref()
            .map(|tools| {
                tools
                    .iter()
                    .any(|tool| tool.function.name.as_str() == "self_test")
            })
            .unwrap_or(false)
    }

    fn tool_call(&self) -> ToolCall {
        ToolCall {
            id: "offline-proof-self-test".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "self_test".into(),
                arguments: serde_json::json!({
                    "steps": [{
                        "name": "offline system-info smoke",
                        "tool": "system_info",
                        "args": {},
                        "expect_contains": "hostname"
                    }]
                })
                .to_string(),
            },
        }
    }

    fn final_text(&self, request: &ChatCompletionRequest) -> String {
        let user_input = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == Role::User)
            .map(|message| message.content.as_str())
            .unwrap_or("");
        format!(
            "offline proof complete: chat coding path handled input {:?} without a live LLM",
            user_input
        )
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(&self, request: ChatCompletionRequest) -> anyhow::Result<ChatCompletionResponse> {
        let message = if self.should_call_tool(&request) {
            Message::assistant_with_tools(
                "offline proof: running safe self-test",
                vec![self.tool_call()],
            )
        } else {
            Message::assistant(self.final_text(&request))
        };

        Ok(ChatCompletionResponse {
            choices: vec![FinalChoice {
                message,
                finish_reason: Some("stop".into()),
            }],
            usage: None,
        })
    }

    async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
        mut on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        if self.should_call_tool(&request) {
            on_event(StreamEvent::TextDelta(
                "offline proof: running safe self-test\n".into(),
            ));
            on_event(StreamEvent::ToolCallDelta {
                index: 0,
                id: Some("offline-proof-self-test".into()),
                name: Some("self_test".into()),
                arguments: None,
            });
            on_event(StreamEvent::Finished {
                finish_reason: "tool_calls".into(),
                content: "offline proof: running safe self-test\n".into(),
                tool_calls: vec![StreamToolCall {
                    id: "offline-proof-self-test".into(),
                    name: "self_test".into(),
                    arguments: self.tool_call().function.arguments,
                }],
            });
            return Ok(());
        }

        let text = self.final_text(&request);
        on_event(StreamEvent::TextDelta(text.clone()));
        on_event(StreamEvent::Finished {
            finish_reason: "stop".into(),
            content: text,
            tool_calls: Vec::new(),
        });
        Ok(())
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn change_model(&self, model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(Self::new(model))
    }

    async fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let mut hash = 0u32;
        for byte in input.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u32);
        }
        Ok((0..16)
            .map(|idx| ((hash.rotate_left(idx) & 0xff) as f32) / 255.0)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::{ToolDef, ToolFunctionDef};

    fn request_with_self_test(messages: Vec<Message>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "offline".into(),
            messages,
            tools: Some(vec![ToolDef {
                def_type: "function".into(),
                function: ToolFunctionDef {
                    name: "self_test".into(),
                    description: "test".into(),
                    parameters: serde_json::json!({ "type": "object" }),
                },
            }]),
            tool_choice: Some("auto".into()),
            stream: false,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
        }
    }

    #[tokio::test]
    async fn first_turn_requests_safe_self_test() {
        let provider = ScriptedProvider::new("offline");
        let response = provider
            .chat(request_with_self_test(vec![Message::user("proof")]))
            .await
            .unwrap();

        let tool_calls = response.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect("tool call");
        assert_eq!(tool_calls[0].function.name, "self_test");
    }

    #[tokio::test]
    async fn tool_result_turn_finishes() {
        let provider = ScriptedProvider::new("offline");
        let response = provider
            .chat(request_with_self_test(vec![
                Message::user("proof"),
                Message::tool_result("offline-proof-self-test", "ok"),
            ]))
            .await
            .unwrap();

        assert!(response.choices[0]
            .message
            .content
            .contains("offline proof complete"));
        assert!(response.choices[0].message.tool_calls.is_none());
    }
}
