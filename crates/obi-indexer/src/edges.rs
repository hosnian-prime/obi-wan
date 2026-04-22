use std::collections::HashMap;

use obi_core::edge::{Edge, EdgeType};
use obi_core::node::{NodeId, SemanticNode};

use crate::parser::CallSite;

/// Resolve static edges across files using call sites and name matching.
pub fn resolve_static_edges(
    nodes: &HashMap<NodeId, SemanticNode>,
    call_sites: &[(NodeId, Vec<CallSite>)],
) -> Vec<Edge> {
    let mut edges = Vec::new();

    // Build name -> [NodeId] lookup
    let mut name_to_ids: HashMap<&str, Vec<NodeId>> = HashMap::new();
    for (id, node) in nodes {
        name_to_ids.entry(node.name.as_str()).or_default().push(*id);
    }

    for (caller_id, calls) in call_sites {
        for call in calls {
            if let Some(target_ids) = name_to_ids.get(call.callee_name.as_str()) {
                if target_ids.len() == 1 {
                    // Unambiguous call
                    let target = target_ids[0];
                    if target != *caller_id {
                        edges.push(Edge::new(*caller_id, target, EdgeType::StaticCall));
                    }
                } else {
                    // Ambiguous: create edges to all candidates with weight=0.5
                    for &target in target_ids {
                        if target != *caller_id {
                            edges.push(
                                Edge::new(*caller_id, target, EdgeType::StaticCall)
                                    .with_weight(0.5),
                            );
                        }
                    }
                }
            }
        }
    }

    edges
}

/// Resolve import/use edges by matching import names to node names.
/// `file_node_ids` are all nodes extracted from the file containing the imports.
/// Each node in the file gets an import edge to the resolved imported symbols.
pub fn resolve_import_edges(
    all_nodes: &HashMap<NodeId, SemanticNode>,
    file_node_ids: &[NodeId],
    imported_names: &[String],
) -> Vec<Edge> {
    let mut edges = Vec::new();

    if file_node_ids.is_empty() || imported_names.is_empty() {
        return edges;
    }

    let name_to_ids: HashMap<&str, Vec<NodeId>> = {
        let mut map = HashMap::new();
        for (id, node) in all_nodes {
            map.entry(node.name.as_str())
                .or_insert_with(Vec::new)
                .push(*id);
        }
        map
    };

    // Use the first node in the file as the source for import edges
    let source_id = file_node_ids[0];

    for name in imported_names {
        if let Some(ids) = name_to_ids.get(name.as_str()) {
            for &id in ids {
                if id != source_id {
                    edges.push(Edge::new(source_id, id, EdgeType::StaticImport));
                }
            }
        }
    }

    edges
}

/// Resolve type reference edges by checking if node contents mention other node names.
/// E.g., `fn foo(x: &MyStruct)` → edge from foo to MyStruct.
pub fn resolve_type_ref_edges(
    nodes: &HashMap<NodeId, SemanticNode>,
    node_contents: &HashMap<NodeId, String>,
) -> Vec<Edge> {
    use obi_core::node::NodeType;
    let mut edges = Vec::new();

    // Collect names of struct/trait/class-like nodes as type reference targets
    let type_names: Vec<(NodeId, &str)> = nodes
        .iter()
        .filter(|(_, n)| matches!(n.node_type, NodeType::Struct | NodeType::Trait))
        .map(|(id, n)| (*id, n.name.as_str()))
        .collect();

    // For each function/method node, check if its content references any type name
    for (id, node) in nodes {
        if !matches!(node.node_type, NodeType::Function | NodeType::Method) {
            continue;
        }
        let content = match node_contents.get(id) {
            Some(c) => c,
            None => continue,
        };
        for &(type_id, type_name) in &type_names {
            if type_id == *id || type_name.len() < 2 {
                continue;
            }
            // Check for type name as a word boundary (not substring of a longer word)
            if content_contains_word(content, type_name) {
                edges.push(Edge::new(*id, type_id, EdgeType::StaticTypeRef));
            }
        }
    }

    edges
}

/// Check if content contains the word as a type reference (simple word boundary check).
fn content_contains_word(content: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = content[start..].find(word) {
        let abs_pos = start + pos;
        let before_ok = abs_pos == 0
            || !content.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                && content.as_bytes()[abs_pos - 1] != b'_';
        let after_pos = abs_pos + word.len();
        let after_ok = after_pos >= content.len()
            || !content.as_bytes()[after_pos].is_ascii_alphanumeric()
                && content.as_bytes()[after_pos] != b'_';
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + 1;
    }
    false
}

/// Create semantic edges between nodes whose embeddings have cosine similarity > threshold.
pub fn resolve_semantic_edges(
    node_ids: &[NodeId],
    embeddings: &HashMap<NodeId, Vec<f32>>,
    threshold: f64,
) -> Vec<Edge> {
    let mut edges = Vec::new();

    for (i, &id_a) in node_ids.iter().enumerate() {
        let emb_a = match embeddings.get(&id_a) {
            Some(e) => e,
            None => continue,
        };

        for &id_b in node_ids.iter().skip(i + 1) {
            let emb_b = match embeddings.get(&id_b) {
                Some(e) => e,
                None => continue,
            };

            let similarity = cosine_similarity(emb_a, emb_b);
            if similarity > threshold {
                let weight = similarity * 0.9;
                edges.push(
                    Edge::new(id_a, id_b, EdgeType::Semantic).with_weight(weight),
                );
            }
        }
    }

    edges
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let mag_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}
