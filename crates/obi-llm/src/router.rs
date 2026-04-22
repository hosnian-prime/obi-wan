use anyhow::Result;
use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{
    CompletionProvider, CompletionRequest, CompletionResponse, EmbeddingProvider,
};
use crate::stream::StreamHandle;

/// LLM Router — routes completion requests to primary provider with optional fallback.
/// Embedding provider is independent (can use a different provider than completion).
///
/// From docs/04-llm-abstraction.md:
/// - Primary provider handles all requests
/// - On rate limit (429) or server error (5xx), automatically falls back
/// - Embedding is always routed to the embedding provider (no fallback needed)
pub struct LlmRouter {
    completion: Box<dyn CompletionProvider>,
    completion_fallback: Option<Box<dyn CompletionProvider>>,
    embedding: Box<dyn EmbeddingProvider>,
}

impl LlmRouter {
    pub fn new(
        completion: Box<dyn CompletionProvider>,
        embedding: Box<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            completion,
            completion_fallback: None,
            embedding,
        }
    }

    pub fn with_fallback(mut self, fallback: Box<dyn CompletionProvider>) -> Self {
        self.completion_fallback = Some(fallback);
        self
    }

    /// Get the primary completion provider name.
    pub fn completion_name(&self) -> &str {
        self.completion.name()
    }

    /// Get the fallback provider name (if configured).
    pub fn fallback_name(&self) -> Option<&str> {
        self.completion_fallback.as_ref().map(|f| f.name())
    }

    /// Get the embedding provider name.
    pub fn embedding_name(&self) -> &str {
        self.embedding.name()
    }

    /// Get the completion model name.
    pub fn completion_model(&self) -> &str {
        self.completion.model_name()
    }

    /// Get the embedding model name.
    pub fn embedding_model(&self) -> &str {
        self.embedding.model_name()
    }

    /// Max context tokens from the primary provider.
    pub fn max_context_tokens(&self) -> usize {
        self.completion.max_context_tokens()
    }

    /// Whether the primary provider supports tool use.
    pub fn supports_tool_use(&self) -> bool {
        self.completion.supports_tool_use()
    }
}

#[async_trait]
impl CompletionProvider for LlmRouter {
    async fn complete(&self, req: CompletionRequest) -> Result<CompletionResponse> {
        match self.completion.complete(req.clone()).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // Check if this is a retriable error and we have a fallback
                if let Some(llm_err) = classify_error(&e) {
                    if llm_err.is_retriable() {
                        if let Some(ref fallback) = self.completion_fallback {
                            return fallback.complete(req).await;
                        }
                    }
                }
                Err(e)
            }
        }
    }

    async fn complete_stream(&self, req: CompletionRequest) -> Result<StreamHandle> {
        match self.completion.complete_stream(req.clone()).await {
            Ok(handle) => Ok(handle),
            Err(e) => {
                if let Some(llm_err) = classify_error(&e) {
                    if llm_err.is_retriable() {
                        if let Some(ref fallback) = self.completion_fallback {
                            return fallback.complete_stream(req).await;
                        }
                    }
                }
                Err(e)
            }
        }
    }

    fn name(&self) -> &str {
        self.completion.name()
    }

    fn model_name(&self) -> &str {
        self.completion.model_name()
    }

    fn max_context_tokens(&self) -> usize {
        self.completion.max_context_tokens()
    }

    fn supports_tool_use(&self) -> bool {
        self.completion.supports_tool_use()
    }
}

#[async_trait]
impl EmbeddingProvider for LlmRouter {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embedding.embed(texts).await
    }

    fn dimensions(&self) -> usize {
        self.embedding.dimensions()
    }

    fn name(&self) -> &str {
        self.embedding.name()
    }

    fn model_name(&self) -> &str {
        self.embedding.model_name()
    }

    fn max_batch_size(&self) -> usize {
        self.embedding.max_batch_size()
    }
}

/// Try to classify an anyhow::Error as an LlmError for fallback decisions.
fn classify_error(err: &anyhow::Error) -> Option<&LlmError> {
    err.downcast_ref::<LlmError>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Message, Role};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Mock provider that always fails with a retriable error.
    struct FailingProvider {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CompletionProvider for FailingProvider {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(LlmError::RateLimit("429 Too Many Requests".into()).into())
        }

        async fn complete_stream(&self, _req: CompletionRequest) -> Result<StreamHandle> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Err(LlmError::RateLimit("429".into()).into())
        }

        fn name(&self) -> &str {
            "failing"
        }
        fn model_name(&self) -> &str {
            "mock-fail"
        }
        fn max_context_tokens(&self) -> usize {
            8000
        }
        fn supports_tool_use(&self) -> bool {
            false
        }
    }

    /// Mock provider that always succeeds.
    struct SuccessProvider {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CompletionProvider for SuccessProvider {
        async fn complete(&self, _req: CompletionRequest) -> Result<CompletionResponse> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                content: "fallback response".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                input_tokens: 10,
                output_tokens: 5,
                has_tool_use: false,
            })
        }

        async fn complete_stream(&self, _req: CompletionRequest) -> Result<StreamHandle> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let _ = tx.send(crate::stream::StreamEvent::Done(CompletionResponse {
                content: "fallback".to_string(),
                thinking: None,
                tool_calls: Vec::new(),
                input_tokens: 0,
                output_tokens: 0,
                has_tool_use: false,
            }));
            Ok(StreamHandle { receiver: rx })
        }

        fn name(&self) -> &str {
            "success"
        }
        fn model_name(&self) -> &str {
            "mock-success"
        }
        fn max_context_tokens(&self) -> usize {
            8000
        }
        fn supports_tool_use(&self) -> bool {
            true
        }
    }

    /// Mock embedding provider.
    struct MockEmbedding;

    #[async_trait]
    impl EmbeddingProvider for MockEmbedding {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0; 768]).collect())
        }
        fn dimensions(&self) -> usize {
            768
        }
        fn name(&self) -> &str {
            "mock"
        }
        fn model_name(&self) -> &str {
            "mock-embed"
        }
        fn max_batch_size(&self) -> usize {
            64
        }
    }

    fn make_request() -> CompletionRequest {
        CompletionRequest {
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            }],
            max_tokens: 100,
            tools: Vec::new(),
            temperature: 0.1,
        }
    }

    #[tokio::test]
    async fn test_router_fallback_on_rate_limit() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));

        let router = LlmRouter::new(
            Box::new(FailingProvider {
                call_count: primary_calls.clone(),
            }),
            Box::new(MockEmbedding),
        )
        .with_fallback(Box::new(SuccessProvider {
            call_count: fallback_calls.clone(),
        }));

        let resp = router.complete(make_request()).await.unwrap();
        assert_eq!(resp.content, "fallback response");
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_router_no_fallback_propagates_error() {
        let primary_calls = Arc::new(AtomicUsize::new(0));

        let router = LlmRouter::new(
            Box::new(FailingProvider {
                call_count: primary_calls.clone(),
            }),
            Box::new(MockEmbedding),
        );
        // No fallback configured

        let result = router.complete(make_request()).await;
        assert!(result.is_err());
        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_router_embedding_passthrough() {
        let router = LlmRouter::new(
            Box::new(SuccessProvider {
                call_count: Arc::new(AtomicUsize::new(0)),
            }),
            Box::new(MockEmbedding),
        );

        let embeddings = router.embed(&["test"]).await.unwrap();
        assert_eq!(embeddings.len(), 1);
        assert_eq!(embeddings[0].len(), 768);
    }
}
