use crate::agent::llm::{
    ChatCompletionRequest, ChatCompletionResponse, LlmProvider, StreamEvent, StreamToolCall,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    embedding_model: Option<String>,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        OpenAiProvider {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            base_url: "https://api.deepseek.com/v1".into(),
            model: model.into(),
            embedding_model: None,
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = Some(model.into());
        self
    }

    pub fn with_model(&self, model: impl Into<String>) -> Self {
        Self {
            client: self.client.clone(),
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: model.into(),
            embedding_model: self.embedding_model.clone(),
        }
    }

    fn build_body(&self, mut request: ChatCompletionRequest) -> serde_json::Value {
        request.model = self.model.clone();
        request.stream = false;

        let msgs: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({ "role": m.role.as_str(), "content": m.content });
                if let Some(ref tc) = m.tool_calls {
                    obj["tool_calls"] = serde_json::to_value(tc).unwrap();
                }
                if let Some(ref tci) = m.tool_call_id {
                    obj["tool_call_id"] = serde_json::Value::String(tci.clone());
                }
                obj
            })
            .collect();

        let mut body =
            serde_json::json!({ "model": request.model, "messages": msgs, "stream": false });
        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap();
        }
        if let Some(ref tc) = request.tool_choice {
            body["tool_choice"] = serde_json::Value::String(tc.clone());
        }
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::Value::from(t);
        }
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::Value::from(mt);
        }
        if let Some(ref re) = request.reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(re.clone());
            body["thinking"] = serde_json::json!({"type": "enabled"});
        }
        body
    }

    fn build_stream_body(&self, mut request: ChatCompletionRequest) -> serde_json::Value {
        request.model = self.model.clone();

        let msgs: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({ "role": m.role.as_str(), "content": m.content });
                if let Some(ref tc) = m.tool_calls {
                    obj["tool_calls"] = serde_json::to_value(tc).unwrap();
                }
                if let Some(ref tci) = m.tool_call_id {
                    obj["tool_call_id"] = serde_json::Value::String(tci.clone());
                }
                obj
            })
            .collect();

        let mut body =
            serde_json::json!({ "model": request.model, "messages": msgs, "stream": true });
        // Ask OpenAI-compatible providers to include the terminal usage
        // chunk. Deeplossless consumes that chunk for per-conversation cost
        // accounting; without it a live benchmark would silently report 0.
        body["stream_options"] = serde_json::json!({"include_usage": true});
        if let Some(ref tools) = request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap();
        }
        if let Some(ref tc) = request.tool_choice {
            body["tool_choice"] = serde_json::Value::String(tc.clone());
        }
        if let Some(t) = request.temperature {
            body["temperature"] = serde_json::Value::from(t);
        }
        if let Some(mt) = request.max_tokens {
            body["max_tokens"] = serde_json::Value::from(mt);
        }
        if let Some(ref re) = request.reasoning_effort {
            body["reasoning_effort"] = serde_json::Value::String(re.clone());
            body["thinking"] = serde_json::json!({"type": "enabled"});
        }
        body
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiProvider {
    async fn chat(&self, request: ChatCompletionRequest) -> anyhow::Result<ChatCompletionResponse> {
        let body = self.build_body(request);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("DeepSeek API request failed: {e}"))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response body: {e}"))?;

        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "DeepSeek API error {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }

        serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse DeepSeek response: {} — body: {}",
                e,
                text.chars().take(300).collect::<String>()
            )
        })
    }

    async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
        mut on_event: Box<dyn FnMut(StreamEvent) + Send>,
    ) -> anyhow::Result<()> {
        let body = self.build_stream_body(request);
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("DeepSeek stream request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "DeepSeek API error {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }

        let mut stream = response.bytes_stream();

        let mut content_buf = String::new();
        let mut tool_call_bufs: HashMap<u32, ToolCallAccum> = HashMap::new();
        let mut finish_reason = String::new();

        use futures::StreamExt;

        let mut decoder = SseLineDecoder::default();
        let mut stream_finished = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("Stream read error: {e}"))?;
            for data in decoder.push(&chunk) {
                if apply_sse_data(
                    &data,
                    &mut content_buf,
                    &mut tool_call_bufs,
                    &mut finish_reason,
                    &mut on_event,
                ) {
                    stream_finished = true;
                    break;
                }
            }
            if stream_finished {
                break;
            }
        }
        if !stream_finished {
            for data in decoder.finish() {
                if apply_sse_data(
                    &data,
                    &mut content_buf,
                    &mut tool_call_bufs,
                    &mut finish_reason,
                    &mut on_event,
                ) {
                    break;
                }
            }
        }

        let tool_calls = finish_tool_calls(tool_call_bufs)?;

        on_event(StreamEvent::Finished {
            finish_reason,
            content: content_buf,
            tool_calls,
        });

        Ok(())
    }

    fn change_model(&self, model: &str) -> Arc<dyn LlmProvider> {
        Arc::new(self.with_model(model))
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    async fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>> {
        let model = self
            .embedding_model
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no embedding model configured"))?;
        let url = format!("{}/embeddings", self.base_url);

        let body = serde_json::json!({
            "model": model,
            "input": input,
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("embedding request failed: {e}"))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("failed to read embedding response: {e}"))?;

        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "embedding API error {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }

        #[derive(serde::Deserialize)]
        struct EmbeddingResponse {
            data: Vec<EmbeddingData>,
        }
        #[derive(serde::Deserialize)]
        struct EmbeddingData {
            embedding: Vec<f32>,
        }

        let parsed: EmbeddingResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("failed to parse embedding response: {e}"))?;

        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| anyhow::anyhow!("embedding response contained no data"))
    }
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

#[derive(Default)]
struct SseLineDecoder {
    buffer: Vec<u8>,
}

impl SseLineDecoder {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut data = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            if let Some(value) = decode_sse_line(&line) {
                data.push(value);
            }
        }
        data
    }

    fn finish(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let line = std::mem::take(&mut self.buffer);
        decode_sse_line(&line).into_iter().collect()
    }
}

fn decode_sse_line(line: &[u8]) -> Option<String> {
    let line = std::str::from_utf8(line).ok()?.trim();
    line.strip_prefix("data:")
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(str::to_owned)
}

fn apply_sse_data<F>(
    data: &str,
    content_buf: &mut String,
    tool_call_bufs: &mut HashMap<u32, ToolCallAccum>,
    finish_reason: &mut String,
    on_event: &mut F,
) -> bool
where
    F: FnMut(StreamEvent) + ?Sized,
{
    if data == "[DONE]" {
        return true;
    }
    let delta: SSEChunk = match serde_json::from_str(data) {
        Ok(delta) => delta,
        Err(_) => return false,
    };
    for choice in &delta.choices {
        if let Some(content) = &choice.delta.content {
            content_buf.push_str(content);
            on_event(StreamEvent::TextDelta(content.clone()));
        }
        if let Some(reason) = &choice.finish_reason {
            finish_reason.clone_from(reason);
        }
        if let Some(tool_deltas) = &choice.delta.tool_calls {
            for tool_delta in tool_deltas {
                let index = tool_delta.index.unwrap_or(0);
                let entry = tool_call_bufs.entry(index).or_default();
                if let Some(id) = tool_delta.id.as_ref().filter(|id| !id.trim().is_empty()) {
                    // Some OpenAI-compatible providers repeat an empty id on
                    // argument continuation chunks. Never let that erase the
                    // usable id from the first chunk.
                    if entry.id.is_none() {
                        entry.id = Some(id.clone());
                    }
                }
                if let Some(function) = &tool_delta.function {
                    if let Some(name) = function
                        .name
                        .as_ref()
                        .filter(|name| !name.trim().is_empty())
                    {
                        merge_streamed_name(&mut entry.name, name);
                        let accumulated_name = entry.name.clone();
                        on_event(StreamEvent::ToolCallDelta {
                            index,
                            id: entry.id.clone(),
                            name: accumulated_name,
                            arguments: None,
                        });
                    }
                    if let Some(arguments) = &function.arguments {
                        entry.args.push_str(arguments);
                        on_event(StreamEvent::ToolCallDelta {
                            index,
                            id: entry.id.clone(),
                            name: None,
                            arguments: Some(arguments.clone()),
                        });
                    }
                }
            }
        }
    }
    false
}

fn merge_streamed_name(slot: &mut Option<String>, delta: &str) {
    match slot {
        None => *slot = Some(delta.to_owned()),
        Some(current) if current == delta || current.ends_with(delta) => {}
        Some(current) if delta.starts_with(current.as_str()) => *current = delta.to_owned(),
        Some(current) => current.push_str(delta),
    }
}

fn finish_tool_calls(
    mut accumulators: HashMap<u32, ToolCallAccum>,
) -> anyhow::Result<Vec<StreamToolCall>> {
    let mut indices: Vec<u32> = accumulators.keys().copied().collect();
    indices.sort_unstable();
    indices
        .into_iter()
        .map(|index| {
            let accumulated = accumulators
                .remove(&index)
                .expect("tool-call index came from the same map");
            let id = accumulated
                .id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("streamed tool call at index {index} omitted its id")
                })?;
            let name = accumulated
                .name
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("streamed tool call at index {index} omitted its function name")
                })?;
            Ok(StreamToolCall {
                id,
                name,
                arguments: accumulated.args,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::{ChatCompletionRequest, Message};

    #[test]
    fn streaming_requests_include_usage() {
        let provider = OpenAiProvider::new("test", "model");
        let body = provider.build_stream_body(ChatCompletionRequest {
            model: "model".into(),
            messages: vec![Message::user("hello")],
            tools: None,
            tool_choice: None,
            stream: true,
            temperature: None,
            max_tokens: None,
            reasoning_effort: None,
        });
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn fragmented_sse_line_preserves_tool_name_and_arguments() {
        let first = format!(
            "data: {}\n",
            serde_json::json!({
                "choices": [{
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": {"name": "list_dir", "arguments": "{\"path\":"}
                    }]},
                    "finish_reason": null
                }]
            })
        );
        let second = format!(
            "data: {}\n",
            serde_json::json!({
                "choices": [{
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "",
                        "function": {"name": "", "arguments": "\".\"}"}
                    }]},
                    "finish_reason": "tool_calls"
                }]
            })
        );
        let split = first.find("list_dir").unwrap() + 3;
        let mut decoder = SseLineDecoder::default();
        assert!(decoder.push(&first.as_bytes()[..split]).is_empty());
        let mut data = decoder.push(&first.as_bytes()[split..]);
        data.extend(decoder.push(second.as_bytes()));

        let mut content = String::new();
        let mut tools = HashMap::new();
        let mut finish_reason = String::new();
        let mut ignore_event = |_| {};
        for item in data {
            assert!(!apply_sse_data(
                &item,
                &mut content,
                &mut tools,
                &mut finish_reason,
                &mut ignore_event,
            ));
        }

        let tool = tools.get(&0).unwrap();
        assert_eq!(tool.id.as_deref(), Some("call-1"));
        assert_eq!(tool.name.as_deref(), Some("list_dir"));
        assert_eq!(tool.args, r#"{"path":"."}"#);
        assert_eq!(finish_reason, "tool_calls");
    }

    #[test]
    fn streamed_function_name_fragments_are_accumulated_without_duplication() {
        let mut name = None;
        merge_streamed_name(&mut name, "list_");
        merge_streamed_name(&mut name, "dir");
        merge_streamed_name(&mut name, "");
        merge_streamed_name(&mut name, "list_dir");
        assert_eq!(name.as_deref(), Some("list_dir"));
    }

    #[test]
    fn fragmented_sse_line_preserves_multibyte_content() {
        let line = format!(
            "data: {}\n",
            serde_json::json!({
                "choices": [{"delta": {"content": "中文验证"}, "finish_reason": "stop"}]
            })
        );
        let marker = "中".as_bytes();
        let start = line
            .as_bytes()
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        let mut decoder = SseLineDecoder::default();
        assert!(decoder.push(&line.as_bytes()[..start + 1]).is_empty());
        let data = decoder.push(&line.as_bytes()[start + 1..]);
        let mut content = String::new();
        let mut tools = HashMap::new();
        let mut finish_reason = String::new();
        let mut ignore_event = |_| {};

        assert!(!apply_sse_data(
            &data[0],
            &mut content,
            &mut tools,
            &mut finish_reason,
            &mut ignore_event,
        ));
        assert_eq!(content, "中文验证");
        assert_eq!(finish_reason, "stop");
    }

    #[test]
    fn incomplete_streamed_tool_call_fails_observably() {
        let tools = HashMap::from([(
            0,
            ToolCallAccum {
                id: Some("call-1".into()),
                name: None,
                args: r#"{"path":"."}"#.into(),
            },
        )]);

        let error = finish_tool_calls(tools).unwrap_err();
        assert!(error.to_string().contains("omitted its function name"));
    }
}
