use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::stream::StreamHandle;

/// Completion provider trait — separated from embedding for flexibility.
/// E.g., use Ollama for embedding + Claude for completion.
#[async_trait]
pub trait CompletionProvider: Send + Sync {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse>;
    async fn complete_stream(&self, req: CompletionRequest) -> Result<StreamHandle>;
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn max_context_tokens(&self) -> usize;
    fn supports_tool_use(&self) -> bool;
}

/// Embedding provider trait — independent from completion.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
    fn name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn max_batch_size(&self) -> usize;
}

/// Tool definition for LLM function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: serde_json::Value,
}

/// A tool call returned by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON string of the arguments.
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: usize,
    pub tools: Vec<ToolDefinition>,
    pub temperature: f64,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// Tool calls made by the assistant (non-empty when role=Assistant and LLM wants to use tools).
    pub tool_calls: Vec<ToolCall>,
    /// Tool call ID this message is responding to (set when role=Tool).
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn tool_result(tool_call_id: String, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub content: String,
    /// Thinking/reasoning content from the LLM (if supported by provider).
    pub thinking: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: usize,
    pub output_tokens: usize,
    /// Whether the response ended because the LLM wants to call tools.
    pub has_tool_use: bool,
}
