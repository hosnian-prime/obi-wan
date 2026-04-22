use std::collections::HashMap;

use obi_core::node::NodeId;

use crate::parser::RawNode;

/// Result of diffing old vs new AST parse.
#[derive(Debug, Default)]
pub struct AstDiff {
    pub added: Vec<RawNode>,
    pub modified: Vec<(NodeId, RawNode)>,
    pub removed: Vec<NodeId>,
    pub unchanged: Vec<NodeId>,
}

/// Stored state for previously indexed nodes in a single file.
#[derive(Debug, Clone)]
pub struct IndexedNode {
    pub id: NodeId,
    pub name: String,
    pub content_hash: [u8; 32],
}

/// Diff new parse results against previously indexed nodes.
/// Matching is by name — if the name exists and content hash differs, it's modified.
pub fn diff_nodes(old_nodes: &[IndexedNode], new_nodes: &[RawNode]) -> AstDiff {
    let mut result = AstDiff::default();

    let old_by_name: HashMap<&str, &IndexedNode> =
        old_nodes.iter().map(|n| (n.name.as_str(), n)).collect();

    let mut seen_old: HashMap<&str, bool> = old_nodes.iter().map(|n| (n.name.as_str(), false)).collect();

    for new_node in new_nodes {
        if let Some(old_node) = old_by_name.get(new_node.name.as_str()) {
            seen_old.insert(&new_node.name, true);
            let new_hash: [u8; 32] = blake3::hash(new_node.content.as_bytes()).into();
            if new_hash == old_node.content_hash {
                result.unchanged.push(old_node.id);
            } else {
                result.modified.push((old_node.id, new_node.clone()));
            }
        } else {
            result.added.push(new_node.clone());
        }
    }

    // Nodes in old but not in new → removed
    for (name, was_seen) in &seen_old {
        if !was_seen {
            if let Some(old) = old_by_name.get(name) {
                result.removed.push(old.id);
            }
        }
    }

    result
}
