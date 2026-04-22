use std::collections::{HashMap, HashSet};

use anyhow::Result;

use obi_core::edge::EdgeType;
use obi_core::graph::KnowledgeGraph;
use obi_core::node::{NodeId, NodeType};
use obi_indexer::store::VectorStore;
use obi_llm::provider::EmbeddingProvider;

use crate::brain::DualBrain;
use crate::token::{estimate_tokens, MIN_REMAINING_TOKENS};

/// Depth decay factors for graph expansion.
/// Depth 0 (anchor itself): 1.0, depth 1: 0.6, depth 2: 0.3.
const DEPTH_DECAY: [f64; 3] = [1.0, 0.6, 0.3];

/// Max BFS expansion depth from anchor nodes.
const MAX_EXPANSION_DEPTH: usize = 2;

/// Number of anchor nodes to retrieve from vector search.
const DEFAULT_TOP_K: usize = 5;

/// A single node selected for the LLM context window.
#[derive(Debug, Clone)]
pub struct ContextNode {
    pub node_id: NodeId,
    pub name: String,
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub node_type: NodeType,
    pub content: String,
    pub score: f64,
    pub token_count: usize,
}

/// The assembled context window ready for LLM consumption.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    /// Nodes selected for context, ordered by type priority.
    pub nodes: Vec<ContextNode>,
    /// IDs of anchor nodes (direct vector search matches).
    pub anchor_ids: Vec<NodeId>,
    /// IDs of all nodes included in context.
    pub in_context_ids: Vec<NodeId>,
    /// Total tokens consumed by context.
    pub total_tokens: usize,
    /// Formatted context string for LLM system prompt.
    pub formatted: String,
    /// Token count of naive approach (all full files) for savings calculation.
    pub naive_tokens: usize,
}

impl ContextWindow {
    /// Calculate savings percentage vs naive approach.
    pub fn savings_percent(&self) -> f64 {
        if self.naive_tokens == 0 {
            return 0.0;
        }
        (1.0 - (self.total_tokens as f64 / self.naive_tokens as f64)) * 100.0
    }
}

/// Smart context builder — selects relevant nodes for the LLM context window.
///
/// 4-step pipeline (from docs/03-context-engine.md):
/// 1. Anchor nodes via vector search (dual-brain with project boost)
/// 2. Graph expansion (BFS, max depth=2)
/// 3. Greedy token packing (knapsack-inspired)
/// 4. Context assembly (type-sorted, formatted with headers)
pub struct ContextBuilder;

impl ContextBuilder {
    /// Build context using the DualBrain (Phase 6).
    /// Searches both project and global brains, merges with project boost,
    /// then runs the standard graph expansion + token packing pipeline.
    pub async fn build_dual(
        query: &str,
        token_budget: usize,
        dual_brain: &DualBrain,
        embedding_provider: &dyn EmbeddingProvider,
        pinned: &HashSet<NodeId>,
        excluded: &HashSet<NodeId>,
        max_node_tokens: usize,
    ) -> Result<ContextWindow> {
        let graph = dual_brain.merged_graph();
        let dims = embedding_provider.dimensions();

        // Embed the query
        let query_embeddings = embedding_provider.embed(&[query]).await?;
        let query_embedding = query_embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embedding provider returned empty result"))?;

        // Dual-brain search: searches both brains, project results boosted
        let search_results = dual_brain.search(&query_embedding, DEFAULT_TOP_K, dims).await?;

        let mut anchor_ids: Vec<NodeId> = Vec::new();
        let mut candidates: HashMap<NodeId, f64> = HashMap::new();

        for sr in &search_results {
            if let Ok(id) = sr.id.parse::<uuid::Uuid>() {
                let node_id: NodeId = id;
                if excluded.contains(&node_id) {
                    continue;
                }
                let score = sr.score as f64;
                anchor_ids.push(node_id);
                candidates.insert(node_id, score * DEPTH_DECAY[0]);
            }
        }

        // Step 2: Graph expansion on merged graph
        Self::expand_candidates(&anchor_ids, &mut candidates, graph, excluded);

        // Force pinned nodes
        Self::apply_pinned(pinned, excluded, &mut candidates);

        // Step 3-4: Token packing + assembly (with max_node_tokens truncation)
        Self::pack_and_assemble(candidates, token_budget, graph, dual_brain, dims, &anchor_ids, max_node_tokens).await
    }

    /// Original single-brain build method (backward compatible).
    pub async fn build(
        query: &str,
        token_budget: usize,
        graph: &KnowledgeGraph,
        embedding_provider: &dyn EmbeddingProvider,
        vector_store: &VectorStore,
        pinned: &HashSet<NodeId>,
        excluded: &HashSet<NodeId>,
    ) -> Result<ContextWindow> {
        // --- Step 1: Anchor nodes via vector search ---
        let query_embeddings = embedding_provider.embed(&[query]).await?;
        let query_embedding = query_embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("embedding provider returned empty result"))?;

        let search_results = vector_store.search(&query_embedding, DEFAULT_TOP_K).await?;

        let mut anchor_ids: Vec<NodeId> = Vec::new();
        let mut candidates: HashMap<NodeId, f64> = HashMap::new();

        for sr in &search_results {
            if let Ok(id) = sr.id.parse::<uuid::Uuid>() {
                let node_id: NodeId = id;
                if excluded.contains(&node_id) {
                    continue;
                }
                let score = sr.score as f64;
                anchor_ids.push(node_id);
                candidates.insert(node_id, score * DEPTH_DECAY[0]);
            }
        }

        // --- Step 2: Graph expansion ---
        Self::expand_candidates(&anchor_ids, &mut candidates, graph, excluded);

        // Force pinned nodes
        Self::apply_pinned(pinned, excluded, &mut candidates);

        // --- Step 3-4: Token packing + assembly (single store) ---
        let mut sorted_candidates: Vec<(NodeId, f64)> = candidates.into_iter().collect();
        sorted_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut selected: Vec<ContextNode> = Vec::new();
        let mut remaining = token_budget;
        let mut naive_tokens: usize = 0;

        for (node_id, score) in sorted_candidates {
            if remaining < MIN_REMAINING_TOKENS {
                break;
            }

            let content = match vector_store.get_content(&node_id).await? {
                Some(c) => c,
                None => continue,
            };

            let token_count = estimate_tokens(&content);
            naive_tokens += token_count;

            if token_count <= remaining {
                let node_meta = match graph.get_node(&node_id) {
                    Some(node) => node,
                    None => continue,
                };

                remaining -= token_count;
                selected.push(ContextNode {
                    node_id,
                    name: node_meta.name.clone(),
                    file_path: node_meta.file_path.to_string_lossy().to_string(),
                    line_start: node_meta.line_range.start,
                    line_end: node_meta.line_range.end,
                    node_type: node_meta.node_type,
                    content,
                    score,
                    token_count,
                });
            }
        }

        selected.sort_by_key(|n| type_priority(n.node_type));

        let in_context_ids: Vec<NodeId> = selected.iter().map(|n| n.node_id).collect();
        let total_tokens = token_budget - remaining;
        let formatted = format_context(&selected, graph);

        Ok(ContextWindow {
            nodes: selected,
            anchor_ids,
            in_context_ids,
            total_tokens,
            formatted,
            naive_tokens,
        })
    }

    /// BFS graph expansion from anchor nodes into candidates map.
    fn expand_candidates(
        anchor_ids: &[NodeId],
        candidates: &mut HashMap<NodeId, f64>,
        graph: &KnowledgeGraph,
        excluded: &HashSet<NodeId>,
    ) {
        for anchor_id in anchor_ids {
            let anchor_score = candidates.get(anchor_id).copied().unwrap_or(0.0);
            let neighbors = graph.neighbors(anchor_id, MAX_EXPANSION_DEPTH);

            for (neighbor_id, edge, depth) in neighbors {
                if excluded.contains(&neighbor_id) {
                    continue;
                }
                if depth > MAX_EXPANSION_DEPTH {
                    continue;
                }
                let decay = DEPTH_DECAY[depth.min(DEPTH_DECAY.len() - 1)];
                let score = anchor_score * edge.weight * decay;

                let entry = candidates.entry(neighbor_id).or_insert(0.0);
                if score > *entry {
                    *entry = score;
                }
            }
        }
    }

    /// Force pinned nodes into candidates with maximum score.
    fn apply_pinned(
        pinned: &HashSet<NodeId>,
        excluded: &HashSet<NodeId>,
        candidates: &mut HashMap<NodeId, f64>,
    ) {
        for &pinned_id in pinned {
            if excluded.contains(&pinned_id) {
                continue;
            }
            candidates.entry(pinned_id).or_insert(f64::MAX);
            if let Some(score) = candidates.get_mut(&pinned_id) {
                *score = f64::MAX;
            }
        }
    }

    /// Step 3-4: Greedy token packing + context assembly for dual-brain.
    /// Applies max_node_tokens truncation to large nodes (from brain.max_node_tokens config).
    /// Uses batch content fetching (single DB query) instead of per-node lookups.
    async fn pack_and_assemble(
        candidates: HashMap<NodeId, f64>,
        token_budget: usize,
        graph: &KnowledgeGraph,
        dual_brain: &DualBrain,
        embedding_dims: usize,
        anchor_ids: &[NodeId],
        max_node_tokens: usize,
    ) -> Result<ContextWindow> {
        let mut sorted_candidates: Vec<(NodeId, f64)> = candidates.into_iter().collect();
        sorted_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Batch fetch all candidate contents in one query (instead of N individual lookups)
        let all_ids: Vec<NodeId> = sorted_candidates.iter().map(|(id, _)| *id).collect();
        let content_map = dual_brain.get_contents_batch(&all_ids, embedding_dims).await?;

        let mut selected: Vec<ContextNode> = Vec::new();
        let mut remaining = token_budget;
        let mut naive_tokens: usize = 0;

        for (node_id, score) in sorted_candidates {
            if remaining < MIN_REMAINING_TOKENS {
                break;
            }

            let content = match content_map.get(&node_id) {
                Some(c) => c.clone(),
                None => continue,
            };

            // Truncate large nodes to max_node_tokens (docs/09-settings.md: brain.max_node_tokens)
            let (content, token_count) = if max_node_tokens > 0 {
                let raw_tokens = estimate_tokens(&content);
                naive_tokens += raw_tokens;
                if raw_tokens > max_node_tokens {
                    let truncated = truncate_to_tokens(&content, max_node_tokens);
                    let tc = estimate_tokens(&truncated);
                    (truncated, tc)
                } else {
                    (content, raw_tokens)
                }
            } else {
                let tc = estimate_tokens(&content);
                naive_tokens += tc;
                (content, tc)
            };

            if token_count <= remaining {
                let node_meta = match graph.get_node(&node_id) {
                    Some(node) => node,
                    None => continue,
                };

                remaining -= token_count;
                selected.push(ContextNode {
                    node_id,
                    name: node_meta.name.clone(),
                    file_path: node_meta.file_path.to_string_lossy().to_string(),
                    line_start: node_meta.line_range.start,
                    line_end: node_meta.line_range.end,
                    node_type: node_meta.node_type,
                    content,
                    score,
                    token_count,
                });
            }
        }

        selected.sort_by_key(|n| type_priority(n.node_type));

        let in_context_ids: Vec<NodeId> = selected.iter().map(|n| n.node_id).collect();
        let total_tokens = token_budget - remaining;
        let formatted = format_context(&selected, graph);

        Ok(ContextWindow {
            nodes: selected,
            anchor_ids: anchor_ids.to_vec(),
            in_context_ids,
            total_tokens,
            formatted,
            naive_tokens,
        })
    }
}

/// Truncate content to approximately `max_tokens` tokens.
/// Uses the same len/4 heuristic as estimate_tokens.
fn truncate_to_tokens(content: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if content.len() <= max_chars {
        return content.to_string();
    }
    // Truncate at char boundary, add ellipsis
    let truncated: String = content.chars().take(max_chars).collect();
    format!("{}...[truncated]", truncated)
}

/// Priority for sorting context nodes by type.
/// Lower number = appears earlier in context.
fn type_priority(node_type: NodeType) -> u8 {
    match node_type {
        NodeType::Struct => 0,
        NodeType::Trait => 1,
        NodeType::Constant => 2,
        NodeType::Function => 3,
        NodeType::Method => 4,
        NodeType::File => 5,
        NodeType::Note => 6,
    }
}

/// Format selected nodes into a context string for the LLM system prompt.
/// Each node gets a location header with metadata + edge descriptions.
fn format_context(nodes: &[ContextNode], graph: &KnowledgeGraph) -> String {
    let mut parts: Vec<String> = Vec::new();

    for node in nodes {
        let mut header = format!(
            "// {}:{}-{} | {} {}",
            node.file_path,
            node.line_start,
            node.line_end,
            type_label(node.node_type),
            node.name,
        );

        // Add edge relationship hints
        let relationships = collect_relationships(node.node_id, graph, nodes);
        if !relationships.is_empty() {
            header.push_str(" | ");
            header.push_str(&relationships);
        }

        parts.push(format!("{}\n{}", header, node.content));
    }

    parts.join("\n\n")
}

/// Collect human-readable relationship descriptions for a node.
fn collect_relationships(
    node_id: NodeId,
    graph: &KnowledgeGraph,
    context_nodes: &[ContextNode],
) -> String {
    let context_ids: HashSet<NodeId> = context_nodes.iter().map(|n| n.node_id).collect();
    let neighbors = graph.neighbors(&node_id, 1);

    let mut calls: Vec<String> = Vec::new();
    let mut called_by: Vec<String> = Vec::new();
    let mut imports: Vec<String> = Vec::new();

    for (neighbor_id, edge, _depth) in &neighbors {
        // Only mention relationships to other nodes in context
        if !context_ids.contains(neighbor_id) {
            continue;
        }
        let neighbor_name = context_nodes
            .iter()
            .find(|n| n.node_id == *neighbor_id)
            .map(|n| n.name.as_str())
            .unwrap_or("?");

        match edge.edge_type {
            EdgeType::StaticCall => {
                if edge.source == node_id {
                    calls.push(neighbor_name.to_string());
                } else {
                    called_by.push(neighbor_name.to_string());
                }
            }
            EdgeType::StaticImport => {
                imports.push(neighbor_name.to_string());
            }
            _ => {}
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if !calls.is_empty() {
        parts.push(format!("calls: {}", calls.join(", ")));
    }
    if !called_by.is_empty() {
        parts.push(format!("called by: {}", called_by.join(", ")));
    }
    if !imports.is_empty() {
        parts.push(format!("imports: {}", imports.join(", ")));
    }
    parts.join(", ")
}

/// Human-readable type label for context headers.
fn type_label(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Function => "fn",
        NodeType::Method => "method",
        NodeType::Struct => "struct",
        NodeType::Trait => "trait",
        NodeType::Constant => "const",
        NodeType::File => "file",
        NodeType::Note => "note",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obi_core::edge::Edge;
    use obi_core::node::{Language, SemanticNode};

    fn make_node(name: &str, node_type: NodeType) -> SemanticNode {
        SemanticNode::new(
            node_type,
            name.to_string(),
            "src/test.rs".into(),
            0..10,
            Language::Rust,
            &format!("fn {}() {{}}", name),
        )
    }

    #[test]
    fn test_type_priority_ordering() {
        assert!(type_priority(NodeType::Struct) < type_priority(NodeType::Function));
        assert!(type_priority(NodeType::Function) < type_priority(NodeType::Note));
        assert!(type_priority(NodeType::Trait) < type_priority(NodeType::Method));
    }

    #[test]
    fn test_format_context_single_node() {
        let graph = KnowledgeGraph::new();
        let node = ContextNode {
            node_id: uuid::Uuid::new_v4(),
            name: "parse".to_string(),
            file_path: "src/parser.rs".to_string(),
            line_start: 42,
            line_end: 67,
            node_type: NodeType::Function,
            content: "fn parse() { }".to_string(),
            score: 0.95,
            token_count: 4,
        };

        let formatted = format_context(&[node], &graph);
        assert!(formatted.contains("// src/parser.rs:42-67 | fn parse"));
        assert!(formatted.contains("fn parse() { }"));
    }

    #[test]
    fn test_format_context_with_relationships() {
        let mut graph = KnowledgeGraph::new();
        let n1 = make_node("caller", NodeType::Function);
        let n2 = make_node("callee", NodeType::Function);
        let id1 = n1.id;
        let id2 = n2.id;
        graph.add_node(n1);
        graph.add_node(n2);
        graph.add_edge(Edge::new(id1, id2, EdgeType::StaticCall));

        let nodes = vec![
            ContextNode {
                node_id: id1,
                name: "caller".to_string(),
                file_path: "src/a.rs".to_string(),
                line_start: 0,
                line_end: 10,
                node_type: NodeType::Function,
                content: "fn caller() { callee(); }".to_string(),
                score: 0.9,
                token_count: 6,
            },
            ContextNode {
                node_id: id2,
                name: "callee".to_string(),
                file_path: "src/b.rs".to_string(),
                line_start: 0,
                line_end: 10,
                node_type: NodeType::Function,
                content: "fn callee() { }".to_string(),
                score: 0.7,
                token_count: 4,
            },
        ];

        let formatted = format_context(&nodes, &graph);
        assert!(formatted.contains("calls: callee"));
        assert!(formatted.contains("called by: caller"));
    }

    #[test]
    fn test_context_window_savings() {
        let window = ContextWindow {
            nodes: vec![],
            anchor_ids: vec![],
            in_context_ids: vec![],
            total_tokens: 1240,
            formatted: String::new(),
            naive_tokens: 17000,
        };
        let savings = window.savings_percent();
        assert!(savings > 92.0 && savings < 93.0);
    }

    #[test]
    fn test_context_window_savings_zero_naive() {
        let window = ContextWindow {
            nodes: vec![],
            anchor_ids: vec![],
            in_context_ids: vec![],
            total_tokens: 0,
            formatted: String::new(),
            naive_tokens: 0,
        };
        assert_eq!(window.savings_percent(), 0.0);
    }

    #[test]
    fn test_depth_decay_values() {
        assert_eq!(DEPTH_DECAY[0], 1.0);
        assert_eq!(DEPTH_DECAY[1], 0.6);
        assert_eq!(DEPTH_DECAY[2], 0.3);
    }

    #[test]
    fn test_expand_candidates_empty_graph() {
        let graph = KnowledgeGraph::new();
        let anchor_ids = vec![];
        let mut candidates = HashMap::new();
        let excluded = HashSet::new();

        ContextBuilder::expand_candidates(&anchor_ids, &mut candidates, &graph, &excluded);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_apply_pinned() {
        let mut candidates = HashMap::new();
        let excluded = HashSet::new();
        let pinned_id = uuid::Uuid::new_v4();
        let mut pinned = HashSet::new();
        pinned.insert(pinned_id);

        ContextBuilder::apply_pinned(&pinned, &excluded, &mut candidates);
        assert_eq!(*candidates.get(&pinned_id).unwrap(), f64::MAX);
    }

    #[test]
    fn test_truncate_to_tokens_short() {
        let content = "fn short() {}";
        let result = truncate_to_tokens(content, 500);
        assert_eq!(result, content); // short content, no truncation
    }

    #[test]
    fn test_truncate_to_tokens_long() {
        let content = "a".repeat(4000); // 4000 chars = ~1000 tokens
        let result = truncate_to_tokens(&content, 100); // limit to 100 tokens = 400 chars
        assert!(result.len() < 4000);
        assert!(result.ends_with("...[truncated]"));
        // 100 tokens * 4 chars = 400 chars + "...[truncated]"
        assert!(result.starts_with(&"a".repeat(400)));
    }
}
