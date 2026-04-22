use obi_core::config::ObiConfig;

use crate::anthropic::AnthropicCompletion;
use crate::ollama::{OllamaCompletion, OllamaEmbedding};
use crate::router::LlmRouter;
use crate::zai::ZaiCompletion;

/// Build an LlmRouter from the application config.
///
/// Reads `config.providers.completion` to select the completion provider and
/// `config.providers.embedding` for embeddings. Supports:
/// - `"ollama"` — local Ollama instance
/// - `"anthropic"` — Anthropic Claude API
/// - `"zai"` — Z.ai (Zhipu AI) Coding Plan API
pub fn build_router(config: &ObiConfig) -> LlmRouter {
    let completion = build_completion(config);
    let embedding = build_embedding(config);
    LlmRouter::new(completion, embedding)
}

fn build_completion(config: &ObiConfig) -> Box<dyn crate::provider::CompletionProvider> {
    let p = &config.providers;
    match p.completion.as_str() {
        "anthropic" => {
            let c = &p.anthropic;
            Box::new(AnthropicCompletion::new(
                &c.api_key,
                &c.completion.model,
                c.completion.max_context,
            ))
        }
        "zai" => {
            let c = &p.zai;
            Box::new(ZaiCompletion::with_base_url(
                &c.api_key,
                &c.completion.model,
                c.completion.max_context,
                &c.base_url,
            ))
        }
        _ => {
            let c = &p.ollama;
            Box::new(OllamaCompletion::new(
                &c.host,
                &c.completion.model,
                c.completion.max_context,
            ))
        }
    }
}

fn build_embedding(config: &ObiConfig) -> Box<dyn crate::provider::EmbeddingProvider> {
    let p = &config.providers;
    match p.embedding.as_str() {
        _ => {
            let c = &p.ollama;
            Box::new(OllamaEmbedding::new(
                &c.host,
                &c.embedding.model,
                c.embedding.dimensions,
            ))
        }
    }
}
