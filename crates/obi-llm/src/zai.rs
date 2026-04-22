use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;

use crate::error::LlmError;
use crate::provider::{
    CompletionProvider, CompletionRequest, CompletionResponse, Role, ToolCall,
};
use crate::stream::{StreamEvent, StreamHandle};

const CODING_PLAN_BASE: &str = "https://api.z.ai/api/coding/paas/v4";

// ─── Z.ai Completion ───────────────────────────────────────────────

pub struct ZaiCompletion {
    api_key: String,
    model: String,
    max_context: usize,
    base_url: String,
    client: reqwest::Client,
}

impl ZaiCompletion {
    pub fn new(api_key: &str, model: &str, max_context: usize) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            max_context,
            base_url: CODING_PLAN_BASE.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(api_key: &str, model: &str, max_context: usize, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            max_context,
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    fn build_body(&self, req: &CompletionRequest, stream: bool) -> serde_json::Value {
        let mut messages = Vec::new();

        // System message as first message (OpenAI-compatible)
        if let Some(ref sys) = req.system {
            messages.push(serde_json::json!({
                "role": "system",
                "content": sys
            }));
        }

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
                        let tool_calls: Vec<serde_json::Value> = msg
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments
                                    }
                                })
                            })
                            .collect();
                        let mut m = serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls
                        });
                        if !msg.content.is_empty() {
                            m["content"] = serde_json::json!(msg.content);
                        }
                        messages.push(m);
                    }
                }
                Role::Tool => {
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": msg.tool_call_id.as_deref().unwrap_or(""),
                        "content": msg.content
                    }));
                }
                Role::System => {
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": msg.content
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": req.max_tokens,
            "stream": stream
        });

        if req.temperature > 0.0 {
            body["temperature"] = serde_json::json!(req.temperature);
        }

        if !req.tools.is_empty() {
            let tools: Vec<serde_json::Value> = req
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    fn classify_status(status: u16, body: String) -> anyhow::Error {
        LlmError::from_status(status, body).into()
    }
}

// ─── Response types (OpenAI-compatible) ────────────────────────────

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    /// Reasoning/thinking content (OpenAI o1/o3, DeepSeek reasoning models).
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    function: ResponseFunction,
}

#[derive(Deserialize)]
struct ResponseFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct UsageInfo {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
}

// ─── SSE streaming types (OpenAI-compatible) ───────────────────────

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    /// Reasoning/thinking content delta (OpenAI o1/o3, DeepSeek reasoning models).
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<StreamToolCall>,
}

#[derive(Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunction>,
}

#[derive(Deserialize)]
struct StreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// ─── CompletionProvider impl ───────────────────────────────────────

#[async_trait]
impl CompletionProvider for ZaiCompletion {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_body(&req, false);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to reach Z.ai API")?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(Self::classify_status(status, text));
        }

        let data: ChatResponse = resp
            .json()
            .await
            .context("failed to parse Z.ai response")?;

        let choice = data.choices.into_iter().next().unwrap_or(Choice {
            message: ResponseMessage {
                content: None,
                reasoning_content: None,
                tool_calls: Vec::new(),
            },
            finish_reason: None,
        });

        let content = choice.message.content.unwrap_or_default();
        let thinking = choice.message.reasoning_content;
        let tool_calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: tc.function.arguments,
            })
            .collect();

        let has_tool_use = choice.finish_reason.as_deref() == Some("tool_calls")
            || !tool_calls.is_empty();

        let (input_tokens, output_tokens) = data
            .usage
            .map(|u| (u.prompt_tokens, u.completion_tokens))
            .unwrap_or((0, 0));

        Ok(CompletionResponse {
            content,
            thinking,
            tool_calls,
            input_tokens,
            output_tokens,
            has_tool_use,
        })
    }

    async fn complete_stream(&self, req: CompletionRequest) -> Result<StreamHandle> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_body(&req, true);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to reach Z.ai API for streaming")?;

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
            let mut tool_arg_bufs: Vec<String> = Vec::new();
            let mut input_tokens = 0usize;
            let mut output_tokens = 0usize;
            let mut buffer = String::new();
            let mut has_tool_use = false;

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

                // Parse SSE lines: "data: <json>\n\n"
                while let Some(double_newline) = buffer.find("\n\n") {
                    let block: String = buffer.drain(..=double_newline + 1).collect();

                    for line in block.lines() {
                        let data_str = match line.strip_prefix("data: ") {
                            Some(d) => d.trim(),
                            None => continue,
                        };

                        if data_str == "[DONE]" {
                            continue;
                        }

                        let chunk: StreamChunk = match serde_json::from_str(data_str) {
                            Ok(c) => c,
                            Err(_) => continue,
                        };

                        // Capture usage if present (some providers send it in the last chunk)
                        if let Some(ref usage) = chunk.usage {
                            input_tokens = usage.prompt_tokens;
                            output_tokens = usage.completion_tokens;
                        }

                        for choice in &chunk.choices {
                            // Reasoning/thinking content delta
                            if let Some(ref reasoning) = choice.delta.reasoning_content {
                                if !reasoning.is_empty() {
                                    full_thinking.push_str(reasoning);
                                    let _ = tx.send(StreamEvent::ThinkingDelta(reasoning.clone()));
                                }
                            }

                            // Text content delta
                            if let Some(ref text) = choice.delta.content {
                                if !text.is_empty() {
                                    full_content.push_str(text);
                                    let _ = tx.send(StreamEvent::TextDelta(text.clone()));
                                }
                            }

                            // Tool call deltas
                            for tc_delta in &choice.delta.tool_calls {
                                let idx = tc_delta.index;

                                // Ensure vectors are large enough
                                while tool_calls.len() <= idx {
                                    tool_calls.push(ToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                    tool_arg_bufs.push(String::new());
                                }

                                // First chunk for this tool call has id + name
                                if let Some(ref id) = tc_delta.id {
                                    tool_calls[idx].id = id.clone();
                                }
                                if let Some(ref func) = tc_delta.function {
                                    if let Some(ref name) = func.name {
                                        tool_calls[idx].name = name.clone();
                                        let _ = tx.send(StreamEvent::ToolCallStart {
                                            name: name.clone(),
                                            id: tool_calls[idx].id.clone(),
                                        });
                                    }
                                    if let Some(ref args) = func.arguments {
                                        tool_arg_bufs[idx].push_str(args);
                                        let _ = tx.send(StreamEvent::ToolCallDelta(args.clone()));
                                    }
                                }
                            }

                            // Check finish reason
                            if choice.finish_reason.as_deref() == Some("tool_calls")
                                || choice.finish_reason.as_deref() == Some("function_call")
                            {
                                has_tool_use = true;
                            }
                        }
                    }
                }
            }

            // Finalize tool call arguments
            for (i, tc) in tool_calls.iter_mut().enumerate() {
                if let Some(buf) = tool_arg_bufs.get(i) {
                    if !buf.is_empty() {
                        tc.arguments = buf.clone();
                    }
                }
            }

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
        "zai"
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

// ─── Model listing ─────────────────────────────────────────────────

/// Fetch available model IDs from the Z.ai API.
pub async fn list_models(api_key: &str) -> Result<Vec<String>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/models", CODING_PLAN_BASE))
        .bearer_auth(api_key)
        .send()
        .await
        .context("failed to reach Z.ai models API")?;

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
