use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use crate::error::LlmError;
use crate::provider::{
    CompletionProvider, CompletionRequest, CompletionResponse, Role, ToolCall,
};
use crate::stream::{StreamEvent, StreamHandle};

const API_BASE: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

// ─── Anthropic Completion ───────────────────────────────────────────

pub struct AnthropicCompletion {
    api_key: String,
    model: String,
    max_context: usize,
    client: reqwest::Client,
}

impl AnthropicCompletion {
    pub fn new(api_key: &str, model: &str, max_context: usize) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            max_context,
            client: reqwest::Client::new(),
        }
    }

    fn build_body(&self, req: &CompletionRequest, stream: bool) -> serde_json::Value {
        let mut messages = Vec::new();

        for msg in &req.messages {
            match msg.role {
                Role::User => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": msg.content
                    }));
                }
                Role::Assistant => {
                    if msg.tool_calls.is_empty() {
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": msg.content
                        }));
                    } else {
                        // Assistant message with tool_use blocks
                        let mut content: Vec<serde_json::Value> = Vec::new();
                        if !msg.content.is_empty() {
                            content.push(serde_json::json!({
                                "type": "text",
                                "text": msg.content
                            }));
                        }
                        for tc in &msg.tool_calls {
                            content.push(serde_json::json!({
                                "type": "tool_use",
                                "id": tc.id,
                                "name": tc.name,
                                "input": serde_json::from_str::<serde_json::Value>(&tc.arguments)
                                    .unwrap_or(serde_json::Value::Object(Default::default()))
                            }));
                        }
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "content": content
                        }));
                    }
                }
                Role::Tool => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": msg.tool_call_id.as_deref().unwrap_or(""),
                            "content": msg.content
                        }]
                    }));
                }
                Role::System => {
                    // Anthropic doesn't have system role in messages; skip
                    // (system is handled via top-level `system` field)
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "messages": messages,
            "stream": stream
        });

        if let Some(ref sys) = req.system {
            body["system"] = serde_json::json!(sys);
        }

        if req.temperature > 0.0 {
            body["temperature"] = serde_json::json!(req.temperature);
        }

        if !req.tools.is_empty() {
            let tools: Vec<serde_json::Value> = req
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("x-api-key", self.api_key.clone()),
            ("anthropic-version", API_VERSION.to_string()),
            ("content-type", "application/json".to_string()),
        ]
    }

    /// Map HTTP error response to classified LlmError.
    fn classify_status(status: u16, body: String) -> anyhow::Error {
        LlmError::from_status(status, body).into()
    }
}

// ─── Response types (non-streaming) ─────────────────────────────────

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: Usage,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: usize,
    #[serde(default)]
    output_tokens: usize,
}

// ─── SSE streaming types ────────────────────────────────────────────

#[derive(Deserialize)]
struct StreamMessageStart {
    message: StreamMessageMeta,
}

#[derive(Deserialize)]
struct StreamMessageMeta {
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
struct StreamContentBlockStart {
    #[serde(default)]
    index: usize,
    content_block: ContentBlock,
}

#[derive(Deserialize)]
struct StreamContentBlockDelta {
    #[serde(default)]
    index: usize,
    delta: DeltaBlock,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum DeltaBlock {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Deserialize)]
struct StreamMessageDelta {
    #[serde(default)]
    usage: Option<DeltaUsage>,
    #[serde(default)]
    delta: Option<MessageDeltaBody>,
}

#[derive(Deserialize)]
struct DeltaUsage {
    #[serde(default)]
    output_tokens: usize,
}

#[derive(Deserialize)]
struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

// ─── CompletionProvider impl ────────────────────────────────────────

#[async_trait]
impl CompletionProvider for AnthropicCompletion {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let body = self.build_body(&req, false);

        let mut request = self.client.post(API_BASE).json(&body);
        for (key, value) in self.auth_headers() {
            request = request.header(key, value);
        }

        let resp = request.send().await.context("failed to reach Anthropic API")?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(Self::classify_status(status, text));
        }

        let data: ApiResponse = resp
            .json()
            .await
            .context("failed to parse Anthropic response")?;

        let (content, tool_calls) = extract_content_blocks(&data.content);
        let has_tool_use = data
            .stop_reason
            .as_deref()
            .map(|r| r == "tool_use")
            .unwrap_or(!tool_calls.is_empty());

        let (thinking, _) = extract_thinking_blocks(&data.content);
        Ok(CompletionResponse {
            content,
            thinking: if thinking.is_empty() { None } else { Some(thinking) },
            tool_calls,
            input_tokens: data.usage.input_tokens,
            output_tokens: data.usage.output_tokens,
            has_tool_use,
        })
    }

    async fn complete_stream(&self, req: CompletionRequest) -> Result<StreamHandle> {
        let body = self.build_body(&req, true);

        let mut request = self.client.post(API_BASE).json(&body);
        for (key, value) in self.auth_headers() {
            request = request.header(key, value);
        }

        let resp = request.send().await.context("failed to reach Anthropic API for streaming")?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(Self::classify_status(status, text));
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let byte_stream = resp.bytes_stream();
        tokio::spawn(async move {
            let mut full_content = String::new();
            let mut full_thinking = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            // Accumulate partial JSON for each tool call index
            let mut tool_json_bufs: Vec<String> = Vec::new();
            let mut input_tokens = 0usize;
            let mut output_tokens = 0usize;
            let mut buffer = String::new();
            let mut has_tool_use = false;
            // Track which content block indices are thinking blocks
            let mut thinking_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

            futures::pin_mut!(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(StreamEvent::Error(e.to_string()));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&bytes));

                // Parse SSE lines: "event: <type>\ndata: <json>\n\n"
                while let Some(double_newline) = buffer.find("\n\n") {
                    let block: String = buffer.drain(..=double_newline + 1).collect();
                    let mut event_type = "";
                    let mut data_str = String::new();

                    for line in block.lines() {
                        if let Some(ev) = line.strip_prefix("event: ") {
                            event_type = ev.trim();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            data_str = d.to_string();
                        }
                    }

                    // Need to re-borrow event_type as &str for matching
                    let event_type_str = event_type.to_string();
                    match event_type_str.as_str() {
                        "message_start" => {
                            if let Ok(ms) = serde_json::from_str::<StreamMessageStart>(&data_str) {
                                input_tokens = ms.message.usage.input_tokens;
                            }
                        }
                        "content_block_start" => {
                            if let Ok(cbs) = serde_json::from_str::<StreamContentBlockStart>(&data_str) {
                                match &cbs.content_block {
                                    ContentBlock::ToolUse { id, name, .. } => {
                                        let _ = tx.send(StreamEvent::ToolCallStart {
                                            name: name.clone(),
                                            id: id.clone(),
                                        });
                                        // Ensure tool_calls vec has an entry for this index
                                        while tool_calls.len() <= cbs.index {
                                            tool_calls.push(ToolCall {
                                                id: String::new(),
                                                name: String::new(),
                                                arguments: String::new(),
                                            });
                                            tool_json_bufs.push(String::new());
                                        }
                                        tool_calls[cbs.index] = ToolCall {
                                            id: id.clone(),
                                            name: name.clone(),
                                            arguments: String::new(),
                                        };
                                    }
                                    ContentBlock::Thinking { .. } => {
                                        thinking_indices.insert(cbs.index);
                                    }
                                    ContentBlock::Text { .. } => {}
                                }
                            }
                        }
                        "content_block_delta" => {
                            if let Ok(cbd) = serde_json::from_str::<StreamContentBlockDelta>(&data_str) {
                                match cbd.delta {
                                    DeltaBlock::TextDelta { text } => {
                                        full_content.push_str(&text);
                                        let _ = tx.send(StreamEvent::TextDelta(text));
                                    }
                                    DeltaBlock::ThinkingDelta { thinking } => {
                                        full_thinking.push_str(&thinking);
                                        let _ = tx.send(StreamEvent::ThinkingDelta(thinking));
                                    }
                                    DeltaBlock::InputJsonDelta { partial_json } => {
                                        if let Some(buf) = tool_json_bufs.get_mut(cbd.index) {
                                            buf.push_str(&partial_json);
                                        }
                                        let _ = tx.send(StreamEvent::ToolCallDelta(partial_json));
                                    }
                                }
                            }
                        }
                        "message_delta" => {
                            if let Ok(md) = serde_json::from_str::<StreamMessageDelta>(&data_str) {
                                if let Some(ref u) = md.usage {
                                    output_tokens = u.output_tokens;
                                }
                                if let Some(ref d) = md.delta {
                                    if d.stop_reason.as_deref() == Some("tool_use") {
                                        has_tool_use = true;
                                    }
                                }
                            }
                        }
                        _ => {} // ping, content_block_stop, message_stop, etc.
                    }
                }
            }

            // Finalize tool call arguments from accumulated JSON buffers
            for (i, tc) in tool_calls.iter_mut().enumerate() {
                if let Some(buf) = tool_json_bufs.get(i) {
                    if !buf.is_empty() {
                        tc.arguments = buf.clone();
                    }
                }
            }

            // Filter out placeholder entries (text blocks get index slots too)
            let final_tool_calls: Vec<ToolCall> = tool_calls
                .into_iter()
                .filter(|tc| !tc.name.is_empty())
                .collect();

            if !final_tool_calls.is_empty() {
                has_tool_use = true;
            }

            let _ = tx.send(StreamEvent::Done(CompletionResponse {
                content: full_content,
                thinking: if full_thinking.is_empty() { None } else { Some(full_thinking) },
                tool_calls: final_tool_calls,
                input_tokens,
                output_tokens,
                has_tool_use,
            }));
        });

        Ok(StreamHandle { receiver: rx })
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn max_context_tokens(&self) -> usize {
        self.max_context
    }

    fn supports_tool_use(&self) -> bool {
        true
    }
}

// ─── Model listing ──────────────────────────────────────────────────

/// Fetch available model IDs from the Anthropic API.
/// Returns a sorted list of model ID strings (e.g. "claude-sonnet-4-6").
pub async fn list_models(api_key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .send()
        .await
        .context("failed to reach Anthropic models API")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(LlmError::from_status(status, text).into());
    }

    let body: serde_json::Value = resp.json().await.context("failed to parse models response")?;
    let mut models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    Ok(models)
}

/// Extract text content and tool calls from Anthropic content blocks.
fn extract_content_blocks(blocks: &[ContentBlock]) -> (String, Vec<ToolCall>) {
    let mut text = String::new();
    let mut tool_calls = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::Thinking { .. } => {
                // Thinking blocks are extracted separately via extract_thinking_blocks
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.to_string(),
                });
            }
        }
    }

    (text, tool_calls)
}

/// Extract thinking content from Anthropic content blocks (non-streaming).
fn extract_thinking_blocks(blocks: &[ContentBlock]) -> (String, usize) {
    let mut thinking = String::new();
    let mut count = 0;

    for block in blocks {
        if let ContentBlock::Thinking { thinking: t } = block {
            if !thinking.is_empty() {
                thinking.push('\n');
            }
            thinking.push_str(t);
            count += 1;
        }
    }

    (thinking, count)
}
