use serde::{Deserialize, Serialize};

use crate::node::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum EdgeType {
    /// Manual [[link]] from markdown notes. Weight: 1.0
    UserLink,
    /// Tree-sitter: function call resolution. Weight: 0.8
    StaticCall,
    /// Tree-sitter: use/import statements. Weight: 0.7
    StaticImport,
    /// Tree-sitter: type references. Weight: 0.7
    StaticTypeRef,
    /// Cosine similarity > 0.85, weight = similarity * 0.9 (range: 0.77–0.9)
    Semantic,
}

impl EdgeType {
    pub fn default_weight(&self) -> f64 {
        match self {
            EdgeType::UserLink => 1.0,
            EdgeType::StaticCall => 0.8,
            EdgeType::StaticImport => 0.7,
            EdgeType::StaticTypeRef => 0.7,
            EdgeType::Semantic => 0.8, // median; actual weight set per-edge
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub edge_type: EdgeType,
    pub weight: f64,
}

impl Edge {
    pub fn new(source: NodeId, target: NodeId, edge_type: EdgeType) -> Self {
        let weight = edge_type.default_weight();
        Self {
            source,
            target,
            edge_type,
            weight,
        }
    }

    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight;
        self
    }
}
