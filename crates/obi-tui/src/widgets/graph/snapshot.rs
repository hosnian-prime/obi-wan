use std::collections::HashMap;
use std::time::Instant;

use obi_core::edge::EdgeType;
use obi_core::node::{NodeId, NodeType};

use super::math::Vec2;

/// State of a node relative to AI context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextState {
    /// Currently in AI context window.
    InContext,
    /// Indexed but not in current context.
    Indexed,
    /// User manually pinned (force include).
    Pinned,
    /// User manually excluded (force exclude).
    Excluded,
    /// Anchor node — direct vector search match to query.
    Anchor,
}

/// Display info for a single node.
#[derive(Debug, Clone)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    pub node_type: NodeType,
    pub position: Vec2,
    pub context_state: ContextState,
}

/// Display info for a single edge.
#[derive(Debug, Clone)]
pub struct EdgeView {
    pub source: NodeId,
    pub target: NodeId,
    pub edge_type: EdgeType,
    pub weight: f64,
}

/// Complete snapshot of graph state for rendering.
/// Sent from layout thread to TUI thread every ~50ms.
#[derive(Debug, Clone)]
pub struct GraphSnapshot {
    pub positions: HashMap<NodeId, Vec2>,
    pub node_views: HashMap<NodeId, NodeView>,
    pub edges: Vec<EdgeView>,
    pub timestamp: Instant,
    pub converged: bool,
    pub tick: u64,
}

impl Default for GraphSnapshot {
    fn default() -> Self {
        Self {
            positions: HashMap::new(),
            node_views: HashMap::new(),
            edges: Vec::new(),
            timestamp: Instant::now(),
            converged: false,
            tick: 0,
        }
    }
}

/// View mode for graph layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Organic force-directed (Obsidian-style).
    ForceDirected,
    /// Hierarchical tree from selected node.
    Tree,
    /// Concentric rings from selected node.
    Radial,
}

impl ViewMode {
    pub fn next(self) -> Self {
        match self {
            Self::ForceDirected => Self::Tree,
            Self::Tree => Self::Radial,
            Self::Radial => Self::ForceDirected,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ForceDirected => "Force",
            Self::Tree => "Tree",
            Self::Radial => "Radial",
        }
    }
}

/// Filter mode for visible nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    CodeOnly,
    NotesOnly,
    InContextOnly,
}

impl FilterMode {
    pub fn next(self) -> Self {
        match self {
            Self::All => Self::CodeOnly,
            Self::CodeOnly => Self::NotesOnly,
            Self::NotesOnly => Self::InContextOnly,
            Self::InContextOnly => Self::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::CodeOnly => "Code",
            Self::NotesOnly => "Notes",
            Self::InContextOnly => "Context",
        }
    }

    pub fn matches(&self, node: &NodeView) -> bool {
        match self {
            Self::All => true,
            Self::CodeOnly => !matches!(node.node_type, NodeType::Note),
            Self::NotesOnly => matches!(node.node_type, NodeType::Note),
            Self::InContextOnly => matches!(
                node.context_state,
                ContextState::InContext | ContextState::Pinned | ContextState::Anchor
            ),
        }
    }
}
