use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::edge::{Edge, EdgeType};
use crate::node::{NodeId, SemanticNode};

/// In-memory knowledge graph storing topology only.
/// Content and embeddings live in LanceDB to avoid memory duplication.
/// Serializable via bincode for persistence to disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    nodes: HashMap<NodeId, SemanticNode>,
    /// Adjacency list: node_id -> list of edges originating from that node.
    outgoing: HashMap<NodeId, Vec<Edge>>,
    /// Reverse adjacency: node_id -> list of edges pointing to that node.
    incoming: HashMap<NodeId, Vec<Edge>>,
}

impl KnowledgeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, node: SemanticNode) {
        let id = node.id;
        self.nodes.insert(id, node);
        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();
    }

    pub fn remove_node(&mut self, id: &NodeId) -> Option<SemanticNode> {
        self.outgoing.remove(id);
        self.incoming.remove(id);
        // Remove edges referencing this node from other adjacency lists
        for edges in self.outgoing.values_mut() {
            edges.retain(|e| &e.target != id);
        }
        for edges in self.incoming.values_mut() {
            edges.retain(|e| &e.source != id);
        }
        self.nodes.remove(id)
    }

    pub fn add_edge(&mut self, edge: Edge) {
        self.outgoing.entry(edge.source).or_default().push(edge.clone());
        self.incoming.entry(edge.target).or_default().push(edge);
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&SemanticNode> {
        self.nodes.get(id)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.outgoing.values().map(|edges| edges.len()).sum()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &SemanticNode> {
        self.nodes.values()
    }

    /// Get all neighbors (both directions) with their connecting edges, up to max_depth.
    pub fn neighbors(&self, id: &NodeId, max_depth: usize) -> Vec<(NodeId, Edge, usize)> {
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        visited.insert(*id);

        let mut frontier = vec![(*id, 0usize)];

        while let Some((current, depth)) = frontier.pop() {
            if depth >= max_depth {
                continue;
            }
            let next_depth = depth + 1;

            // Outgoing edges
            if let Some(edges) = self.outgoing.get(&current) {
                for edge in edges {
                    if visited.insert(edge.target) {
                        result.push((edge.target, edge.clone(), next_depth));
                        frontier.push((edge.target, next_depth));
                    }
                }
            }
            // Incoming edges
            if let Some(edges) = self.incoming.get(&current) {
                for edge in edges {
                    if visited.insert(edge.source) {
                        result.push((edge.source, edge.clone(), next_depth));
                        frontier.push((edge.source, next_depth));
                    }
                }
            }
        }

        result
    }

    pub fn all_nodes(&self) -> &HashMap<NodeId, SemanticNode> {
        &self.nodes
    }

    /// Get all incoming edges for a node (for backlink panel).
    /// Returns edges that point TO this node, with source node info.
    pub fn incoming_edges(&self, id: &NodeId) -> Vec<&Edge> {
        self.incoming
            .get(id)
            .map(|edges| edges.iter().collect())
            .unwrap_or_default()
    }

    /// Get all outgoing edges for a node.
    pub fn outgoing_edges(&self, id: &NodeId) -> Vec<&Edge> {
        self.outgoing
            .get(id)
            .map(|edges| edges.iter().collect())
            .unwrap_or_default()
    }

    /// Merge another graph into this one. Used for dual-brain query-time merge.
    /// Nodes from `other` are added if not already present (by ID).
    /// Edges from `other` are always added (deduplicated by source+target+type).
    pub fn merge(&mut self, other: &KnowledgeGraph) {
        // Add nodes that don't exist yet
        for (id, node) in &other.nodes {
            if !self.nodes.contains_key(id) {
                self.add_node(node.clone());
            }
        }

        // Add edges, deduplicating by (source, target, edge_type)
        let existing_edges: std::collections::HashSet<(NodeId, NodeId, std::mem::Discriminant<EdgeType>)> = self
            .outgoing
            .values()
            .flat_map(|edges| edges.iter())
            .map(|e| (e.source, e.target, std::mem::discriminant(&e.edge_type)))
            .collect();

        for edges in other.outgoing.values() {
            for edge in edges {
                let key = (edge.source, edge.target, std::mem::discriminant(&edge.edge_type));
                if !existing_edges.contains(&key) {
                    self.add_edge(edge.clone());
                }
            }
        }
    }

    /// Clear all edges (used before re-resolving).
    pub fn clear_edges(&mut self) {
        self.outgoing.clear();
        self.incoming.clear();
        // Re-init empty entries for all nodes
        for id in self.nodes.keys() {
            self.outgoing.entry(*id).or_default();
            self.incoming.entry(*id).or_default();
        }
    }

    /// Serialize graph to disk via bincode.
    pub fn save_to_disk(&self, path: &Path) -> anyhow::Result<()> {
        let data = bincode::serialize(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Load graph from disk.
    pub fn load_from_disk(path: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read(path)?;
        let graph: Self = bincode::deserialize(&data)?;
        Ok(graph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Language, SemanticNode};

    fn make_node(name: &str) -> SemanticNode {
        SemanticNode::new(
            crate::node::NodeType::Function,
            name.to_string(),
            "test.rs".into(),
            0..10,
            Language::Rust,
            &format!("fn {}() {{}}", name),
        )
    }

    #[test]
    fn test_incoming_edges() {
        let mut graph = KnowledgeGraph::new();
        let n1 = make_node("caller");
        let n2 = make_node("callee");
        let id1 = n1.id;
        let id2 = n2.id;
        graph.add_node(n1);
        graph.add_node(n2);
        graph.add_edge(Edge::new(id1, id2, EdgeType::StaticCall));

        let incoming = graph.incoming_edges(&id2);
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source, id1);

        let outgoing = graph.outgoing_edges(&id1);
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].target, id2);
    }

    #[test]
    fn test_merge_graphs() {
        let mut g1 = KnowledgeGraph::new();
        let n1 = make_node("func_a");
        let id1 = n1.id;
        g1.add_node(n1);

        let mut g2 = KnowledgeGraph::new();
        let n2 = make_node("func_b");
        let id2 = n2.id;
        g2.add_node(n2);
        g2.add_edge(Edge::new(id2, id2, EdgeType::Semantic).with_weight(0.9));

        assert_eq!(g1.node_count(), 1);
        g1.merge(&g2);
        assert_eq!(g1.node_count(), 2);
        assert!(g1.get_node(&id2).is_some());
        // Edge from g2 should be merged
        assert_eq!(g1.edge_count(), 1);
    }

    #[test]
    fn test_merge_deduplicates_nodes() {
        let mut g1 = KnowledgeGraph::new();
        let node = make_node("shared");
        let id = node.id;
        g1.add_node(node.clone());

        let mut g2 = KnowledgeGraph::new();
        g2.add_node(node);

        g1.merge(&g2);
        assert_eq!(g1.node_count(), 1);
        assert!(g1.get_node(&id).is_some());
    }

    #[test]
    fn test_merge_deduplicates_edges() {
        let mut g1 = KnowledgeGraph::new();
        let n1 = make_node("a");
        let n2 = make_node("b");
        let id1 = n1.id;
        let id2 = n2.id;
        g1.add_node(n1.clone());
        g1.add_node(n2.clone());
        g1.add_edge(Edge::new(id1, id2, EdgeType::StaticCall));

        let mut g2 = KnowledgeGraph::new();
        g2.add_node(n1);
        g2.add_node(n2);
        g2.add_edge(Edge::new(id1, id2, EdgeType::StaticCall));

        g1.merge(&g2);
        assert_eq!(g1.edge_count(), 1); // no duplicates
    }
}
