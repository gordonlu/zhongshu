use crate::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, LlmProvider, StreamEvent, StreamToolCall,
};
use std::collections::HashMap;

#[derive(Clone)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        OpenAiProvider {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: "https://api.deepseek.com/v1".into(),
            model: model.into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn build_body(&self, mut request: ChatCompletionRequest) -> serde_json::Value {
        request.model = self.model.clone();
        request.stream = false;

        let msgs: Vec<serde_json::Value> = request.messages.iter().map(|m| {
            let mut obj = serde_json::json!({ "role": m.role.as_str(), "content": m.content });
            if let Some(ref tc) = m.tool_calls {
                obj["tool_calls"] = serde_json::to_value(tc).unwrap();
            }
            if let Some(ref tci) = m.tool_call_id {
                obj["tool_call_id"] = serde_json::Value::String(tci.clone());
            }
            obj
        }).collect();

        let mut body = serde_json::json!({ "model": request.model, "messages": msgs, "stream": false });
        if let Some(ref tools) = request.tools { body["tools"] = serde_json::to_value(tools).unwrap(); }
        if let Some(ref tc) = request.tool_choice { body["tool_choice"] = serde_json::Value::String(tc.clone()); }
        if let Some(t) = request.temperature { body["temperature"] = serde_json::Value::from(t); }
        if let Some(mt) = request.max_tokens { body["max_tokens"] = serde_json::Value::from(mt); }
        body
    }

    fn build_stream_body(&self, mut request: ChatCompletionRequest) -> serde_json::Value {
        request.model = self.model.clone();

        let msgs: Vec<serde_json::Value> = request.messages.iter().map(|m| {
            let mut obj = serde_json::json!({ "role": m.role.as_str(), "content": m.content });
            if let Some(ref tc) = m.tool_calls {
                obj["tool_calls"] = serde_json::to_value(tc).unwrap();
            }
            if let Some(ref tci) = m.tool_call_id {
                obj["tool_call_id"] = serde_json::Value::String(tci.clone());
            }
            obj
        }).collect();

        let mut body = serde_json::json!({ "model": request.model, "messages": msgs, "stream": true });
        if let Some(ref tools) = request.tools { body["tools"] = serde_json::to_value(tools).unwrap(); }
        if let Some(ref tc) = request.tool_choice { body["tool_choice"] = serde_json::Value::String(tc.clone()); }
        if let Some(t) = request.temperature { body["temperature"] = serde_json::Value::from(t); }
        if let Some(mt) = request.max_tokens { body["max_tokens"] = serde_json::Value::from(mt); }
        body
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(&self, request: ChatCompletionRequest) -> anyhow::Result<ChatCompletionResponse> {
        let body = self.build_body(request);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| anyhow::anyhow!("DeepSeek API request failed: {e}"))?;

        let status = response.status();
        let text = response.text().await
            .map_err(|e| anyhow::anyhow!("Failed to read response body: {e}"))?;

        if !status.is_success() {
            return Err(anyhow::anyhow!("DeepSeek API error {}: {}", status, text.chars().take(500).collect::<String>()));
        }

        serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Failed to parse DeepSeek response: {} — body: {}", e, text.chars().take(300).collect::<String>()))
    }

    async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
        mut on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        let body = self.build_stream_body(request);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send().await
            .map_err(|e| anyhow::anyhow!("DeepSeek stream request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("DeepSeek API error {}: {}", status, text.chars().take(500).collect::<String>()));
        }

        let mut stream = response.bytes_stream();

        let mut content_buf = String::new();
        let mut tool_call_bufs: HashMap<u32, ToolCallAccum> = HashMap::new();
        let mut finish_reason = String::new();

        use futures::StreamExt;

        let mut line_buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("Stream read error: {e}"))?;
            let text = String::from_utf8_lossy(&chunk);

            for line in text.lines() {
                let line = line.trim().to_string();
                if line_buf.is_empty() && !line.starts_with("data: ") {
                    continue;
                }
                line_buf.push_str(&line);

                if !line_buf.starts_with("data: ") {
                    continue;
                }

                let data = line_buf["data: ".len()..].trim().to_string();
                line_buf.clear();

                if data == "[DONE]" {
                    break;
                }

                let delta: SSEChunk = match serde_json::from_str(&data) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                for choice in &delta.choices {
                    if let Some(ref content) = choice.delta.content {
                        content_buf.push_str(content);
                        on_event(StreamEvent::TextDelta(content.clone()));
                    }

                    if let Some(ref fr) = choice.finish_reason {
                        finish_reason = fr.clone();
                        if fr == "tool_calls" || fr == "stop" || fr == "length" {
                            // Normal finish or tool call — text delta already emitted above
                        } else {
                            continue;
                        }
                    }

                    if let Some(ref tc_deltas) = choice.delta.tool_calls {
                        for tc_delta in tc_deltas {
                            let index = tc_delta.index.unwrap_or(0);
                            let entry = tool_call_bufs.entry(index).or_default();

                            if let Some(ref id) = tc_delta.id {
                                entry.id = Some(id.clone());
                            }
                            if let Some(ref f) = tc_delta.function {
                                if let Some(ref name) = f.name {
                                    entry.name = Some(name.clone());
                                    on_event(StreamEvent::ToolCallDelta {
                                        index,
                                        id: entry.id.clone(),
                                        name: Some(name.clone()),
                                        arguments: None,
                                    });
                                }
                                if let Some(ref args) = f.arguments {
                                    entry.args.push_str(args);
                                    on_event(StreamEvent::ToolCallDelta {
                                        index,
                                        id: entry.id.clone(),
                                        name: None,
                                        arguments: Some(args.clone()),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let tool_calls: Vec<StreamToolCall> = {
            let mut indices: Vec<u32> = tool_call_bufs.keys().copied().collect();
            indices.sort();
            indices.into_iter().filter_map(|i| {
                let acc = tool_call_bufs.remove(&i)?;
                Some(StreamToolCall {
                    id: acc.id.unwrap_or_default(),
                    name: acc.name.unwrap_or_default(),
                    arguments: acc.args,
                })
            }).collect()
        };

        on_event(StreamEvent::Finished {
            finish_reason,
            content: content_buf,
            tool_calls,
        });

        Ok(())
    }

    fn model_name(&self) -> &str { &self.model }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SSEChunk {
    choices: Vec<SSEChoice>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SSEChoice {
    delta: SSEDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SSEDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<SSEToolCallDelta>>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SSEToolCallDelta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<SSEFunctionDelta>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SSEFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ToolCallAccum {
    id: Option<String>,
    name: Option<String>,
    args: String,
}
