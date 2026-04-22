use crate::provider::CompletionResponse;

pub struct StreamHandle {
    pub receiver: tokio::sync::mpsc::UnboundedReceiver<StreamEvent>,
}

#[derive(Debug)]
pub enum StreamEvent {
    /// Partial text chunk from the LLM.
    TextDelta(String),
    /// Partial thinking/reasoning chunk from the LLM (provider-agnostic).
    ThinkingDelta(String),
    /// Agent tool invocation begins.
    ToolCallStart { name: String, id: String },
    /// Partial tool call arguments (streamed JSON).
    ToolCallDelta(String),
    /// Final aggregated response.
    Done(CompletionResponse),
    /// Error during streaming.
    Error(String),
}
