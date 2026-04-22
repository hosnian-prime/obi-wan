use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use crate::provider::{
    CompletionProvider, CompletionRequest, CompletionResponse, EmbeddingProvider, Role, ToolCall,
};
use crate::stream::{StreamEvent, StreamHandle};

// ─── Ollama Embedding ────────────────────────────────────────────────

/// Ollama embedding provider — calls the local Ollama API.
/// Default model: nomic-embed-text (768 dimensions).
pub struct OllamaEmbedding {
    host: String,
    model: String,
    dims: usize,
    client: reqwest::Client,
}

impl OllamaEmbedding {
    pub fn new(host: &str, model: &str, dims: usize) -> Self {
        Self {
            host: host.trim_end_matches('/').to_string(),
            model: model.to_string(),
            dims,
            client: reqwest::Client::new(),
        }
    }

    /// Default: localhost Ollama with nomic-embed-text (768d).
    pub fn default_local() -> Self {
        Self::new("http://localhost:11434", "nomic-embed-text", 768)
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbedding {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/api/embed", self.host);
        let body = EmbedRequest {
            model: &self.model,
            input: texts.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to reach Ollama")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama embed failed ({}): {}", status, text);
        }

        let data: EmbedResponse = resp
            .json()
            .await
            .context("failed to parse Ollama embed response")?;

        Ok(data.embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn max_batch_size(&self) -> usize {
        64
    }
}

// ─── Ollama Completion ───────────────────────────────────────────────

/// Ollama completion provider — calls the local Ollama /api/chat endpoint.
/// Supports streaming and tool use (model-dependent).
pub struct OllamaCompletion {
    host: String,
    model: String,
    max_context: usize,
    client: reqwest::Client,
}

impl OllamaCompletion {
    pub fn new(host: &str, model: &str, max_context: usize) -> Self {
        Self {
            host: host.trim_end_matches('/').to_string(),
            model: model.to_string(),
            max_context,
            client: reqwest::Client::new(),
        }
    }

    /// Default: localhost Ollama with qwen2.5:14b (32k context).
    pub fn default_local() -> Self {
        Self::new("http://localhost:11434", "qwen3.5:9b", 32768)
    }

    fn build_chat_body(&self, req: &CompletionRequest, stream: bool) -> serde_json::Value {
        let mut messages = Vec::new();

        // System message
        if let Some(ref sys) = req.system {
            messages.push(serde_json::json!({
                "role": "system",
                "content": sys
            }));
        }

        // Conversation messages
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
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::from_str::<serde_json::Value>(&tc.arguments)
                                            .unwrap_or(serde_json::Value::Object(Default::default()))
                                    }
                                })
                            })
                            .collect();
                        messages.push(serde_json::json!({
                            "role": "assistant",
                            "tool_calls": tool_calls
                        }));
                    }
                }
                Role::Tool => {
                    messages.push(serde_json::json!({
                        "role": "tool",
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
            "stream": stream,
            "options": {
                "temperature": req.temperature,
                "num_predict": req.max_tokens
            }
        });

        // Add tools if any
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
}

/// Ollama /api/chat response (non-streaming).
#[derive(Deserialize)]
struct OllamaChatResponse {
    message: OllamaChatMessage,
    #[serde(default)]
    prompt_eval_count: usize,
    #[serde(default)]
    eval_count: usize,
}

#[derive(Deserialize)]
struct OllamaChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Deserialize)]
struct OllamaToolCall {
    function: OllamaFunction,
}

#[derive(Deserialize)]
struct OllamaFunction {
    name: String,
    arguments: serde_json::Value,
}

/// Ollama /api/chat streaming chunk.
#[derive(Deserialize)]
struct OllamaStreamChunk {
    #[serde(default)]
    message: Option<OllamaStreamMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: usize,
    #[serde(default)]
    eval_count: usize,
}

#[derive(Deserialize)]
struct OllamaStreamMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

/// State machine for parsing `<think>...</think>` tags in streaming content.
/// Models like DeepSeek-R1 and QwQ emit thinking inside these tags.
struct ThinkTagParser {
    inside_think: bool,
    /// Partial tag buffer (e.g. we received "<thi" but not the full "<think>")
    tag_buf: String,
}

impl ThinkTagParser {
    fn new() -> Self {
        Self {
            inside_think: false,
            tag_buf: String::new(),
        }
    }

    /// Process a text chunk. Returns (text_chunks, thinking_chunks).
    fn process(&mut self, input: &str) -> (Vec<String>, Vec<String>) {
        let mut text_out = Vec::new();
        let mut think_out = Vec::new();
        let mut current = String::new();

        let combined = std::mem::take(&mut self.tag_buf) + input;
        let mut chars = combined.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '<' {
                // Try to match <think> or </think>
                let mut lookahead = String::from('<');
                let mut matched = false;

                // Peek enough chars for </think> (8 chars total)
                let needed: Vec<char> = chars.clone().take(7).collect();
                lookahead.extend(&needed);

                if lookahead.starts_with("<think>") {
                    // Flush current content
                    if !current.is_empty() {
                        if self.inside_think {
                            think_out.push(std::mem::take(&mut current));
                        } else {
                            text_out.push(std::mem::take(&mut current));
                        }
                    }
                    self.inside_think = true;
                    // Consume the matched chars
                    for _ in 0..6 { chars.next(); }
                    matched = true;
                } else if lookahead.starts_with("</think>") {
                    // Flush current thinking content
                    if !current.is_empty() {
                        if self.inside_think {
                            think_out.push(std::mem::take(&mut current));
                        } else {
                            text_out.push(std::mem::take(&mut current));
                        }
                    }
                    self.inside_think = false;
                    // Consume the matched chars
                    for _ in 0..7 { chars.next(); }
                    matched = true;
                }

                if !matched {
                    // Could be a partial tag at the end of input
                    if lookahead.len() < 8 && ("<think>".starts_with(&lookahead) || "</think>".starts_with(&lookahead)) {
                        // Buffer it for next chunk
                        self.tag_buf = lookahead;
                        // Consume the peeked chars
                        for _ in 0..needed.len() { chars.next(); }
                        break;
                    } else {
                        current.push(ch);
                    }
                }
            } else {
                current.push(ch);
            }
        }

        // Flush remaining
        if !current.is_empty() {
            if self.inside_think {
                think_out.push(current);
            } else {
                text_out.push(current);
            }
        }

        (text_out, think_out)
    }
}

fn convert_tool_calls(ollama_calls: &[OllamaToolCall]) -> Vec<ToolCall> {
    ollama_calls
        .iter()
        .enumerate()
        .map(|(i, tc)| ToolCall {
            id: format!("call_{}", i),
            name: tc.function.name.clone(),
            arguments: tc.function.arguments.to_string(),
        })
        .collect()
}

#[async_trait]
impl CompletionProvider for OllamaCompletion {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        let url = format!("{}/api/chat", self.host);
        let body = self.build_chat_body(&req, false);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to reach Ollama")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama chat failed ({}): {}", status, text);
        }

        let data: OllamaChatResponse = resp
            .json()
            .await
            .context("failed to parse Ollama chat response")?;

        let tool_calls = convert_tool_calls(&data.message.tool_calls);
        let has_tool_use = !tool_calls.is_empty();

        // Extract <think>...</think> from content
        let (content, thinking) = strip_think_tags(&data.message.content);

        Ok(CompletionResponse {
            content,
            thinking,
            tool_calls,
            input_tokens: data.prompt_eval_count,
            output_tokens: data.eval_count,
            has_tool_use,
        })
    }

    async fn complete_stream(&self, req: CompletionRequest) -> Result<StreamHandle> {
        let url = format!("{}/api/chat", self.host);
        let body = self.build_chat_body(&req, true);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("failed to reach Ollama for streaming")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama stream failed ({}): {}", status, text);
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Spawn task to consume the byte stream and emit StreamEvents
        let byte_stream = resp.bytes_stream();
        tokio::spawn(async move {
            let mut full_content = String::new();
            let mut full_thinking = String::new();
            let mut all_tool_calls: Vec<ToolCall> = Vec::new();
            let mut input_tokens = 0usize;
            let mut output_tokens = 0usize;
            let mut buffer = String::new();
            let mut think_parser = ThinkTagParser::new();

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

                // Process complete lines (NDJSON)
                while let Some(newline_pos) = buffer.find('\n') {
                    let line: String = buffer.drain(..=newline_pos).collect();
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    let chunk: OllamaStreamChunk = match serde_json::from_str(line) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    if let Some(ref msg) = chunk.message {
                        // Parse content through think-tag state machine
                        if !msg.content.is_empty() {
                            let (text_chunks, think_chunks) = think_parser.process(&msg.content);
                            for t in text_chunks {
                                full_content.push_str(&t);
                                let _ = tx.send(StreamEvent::TextDelta(t));
                            }
                            for t in think_chunks {
                                full_thinking.push_str(&t);
                                let _ = tx.send(StreamEvent::ThinkingDelta(t));
                            }
                        }

                        // Tool calls (usually come in the final chunk)
                        if !msg.tool_calls.is_empty() {
                            let calls = convert_tool_calls(&msg.tool_calls);
                            for tc in &calls {
                                let _ = tx.send(StreamEvent::ToolCallStart {
                                    name: tc.name.clone(),
                                    id: tc.id.clone(),
                                });
                                let _ = tx.send(StreamEvent::ToolCallDelta(tc.arguments.clone()));
                            }
                            all_tool_calls.extend(calls);
                        }
                    }

                    if chunk.done {
                        input_tokens = chunk.prompt_eval_count;
                        output_tokens = chunk.eval_count;
                    }
                }
            }

            let has_tool_use = !all_tool_calls.is_empty();
            let _ = tx.send(StreamEvent::Done(CompletionResponse {
                content: full_content,
                thinking: if full_thinking.is_empty() { None } else { Some(full_thinking) },
                tool_calls: all_tool_calls,
                input_tokens,
                output_tokens,
                has_tool_use,
            }));
        });

        Ok(StreamHandle { receiver: rx })
    }

    fn name(&self) -> &str {
        "ollama"
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

/// Strip `<think>...</think>` tags from content (non-streaming).
/// Returns (clean_content, Option<thinking_content>).
fn strip_think_tags(content: &str) -> (String, Option<String>) {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut rest = content;

    while let Some(start) = rest.find("<think>") {
        text.push_str(&rest[..start]);
        rest = &rest[start + 7..];
        if let Some(end) = rest.find("</think>") {
            thinking.push_str(&rest[..end]);
            rest = &rest[end + 8..];
        } else {
            // Unclosed tag — treat remainder as thinking
            thinking.push_str(rest);
            rest = "";
        }
    }
    text.push_str(rest);

    let thinking = if thinking.is_empty() { None } else { Some(thinking) };
    (text.trim().to_string(), thinking)
}
