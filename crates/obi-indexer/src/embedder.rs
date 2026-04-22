use std::collections::HashMap;

use anyhow::Result;
use obi_core::node::NodeId;
use obi_llm::provider::EmbeddingProvider;

/// Max characters to send for embedding (nomic-embed-text has ~8192 token context).
/// Rough estimate: 1 token ≈ 4 chars, so ~30k chars is safe.
const MAX_EMBED_CHARS: usize = 30_000;

/// Batch-embed node contents using the configured embedding provider.
/// Returns a map of NodeId -> embedding vector.
/// Truncates large nodes and retries failed batches individually.
pub async fn embed_nodes(
    provider: &dyn EmbeddingProvider,
    node_contents: &HashMap<NodeId, String>,
) -> Result<HashMap<NodeId, Vec<f32>>> {
    let mut embeddings = HashMap::new();

    if node_contents.is_empty() {
        return Ok(embeddings);
    }

    // Truncate oversized content before embedding
    let truncated: Vec<(NodeId, String)> = node_contents
        .iter()
        .map(|(id, content)| {
            let text = if content.len() > MAX_EMBED_CHARS {
                content[..MAX_EMBED_CHARS].to_string()
            } else {
                content.clone()
            };
            (*id, text)
        })
        .collect();

    let batch_size = provider.max_batch_size();

    for chunk in truncated.chunks(batch_size) {
        let texts: Vec<&str> = chunk.iter().map(|(_, content)| content.as_str()).collect();
        let ids: Vec<NodeId> = chunk.iter().map(|(id, _)| *id).collect();

        match provider.embed(&texts).await {
            Ok(vecs) => {
                for (id, vec) in ids.into_iter().zip(vecs.into_iter()) {
                    embeddings.insert(id, vec);
                }
            }
            Err(_) => {
                // Batch failed — retry each node individually
                for (id, content) in chunk {
                    match provider.embed(&[content.as_str()]).await {
                        Ok(vecs) if !vecs.is_empty() => {
                            embeddings.insert(*id, vecs.into_iter().next().unwrap());
                        }
                        _ => {} // Skip this node silently
                    }
                }
            }
        }
    }

    Ok(embeddings)
}
