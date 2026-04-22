use std::collections::HashMap;
use std::time::Instant;

use obi_core::edge::Edge;
use obi_core::graph::KnowledgeGraph;
use obi_core::node::{NodeId, NodeType};

use super::math::Vec2;
use super::quadtree::QuadTree;
use super::snapshot::{ContextState, EdgeView, GraphSnapshot, NodeView, ViewMode};

/// Force-directed layout simulation parameters.
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    pub repulsion_constant: f64,
    pub attraction_constant: f64,
    pub ideal_edge_length: f64,
    pub damping: f64,
    pub cooling_rate: f64,
    pub min_movement: f64,
    pub theta: f64,
    pub tick_interval_ms: u64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            repulsion_constant: 100.0,
            attraction_constant: 0.01,
            ideal_edge_length: 50.0,
            damping: 0.95,
            cooling_rate: 0.999,
            min_movement: 0.01,
            theta: 0.8,
            tick_interval_ms: 50,
        }
    }
}

/// Force-directed layout engine (Fruchterman-Reingold + Barnes-Hut).
pub struct ForceLayout {
    config: LayoutConfig,
    positions: HashMap<NodeId, Vec2>,
    velocities: HashMap<NodeId, Vec2>,
    node_ids: Vec<NodeId>,
    edges: Vec<Edge>,
    max_displacement: f64,
    tick: u64,
    converged: bool,

    // Node metadata for snapshot building
    node_names: HashMap<NodeId, String>,
    node_types: HashMap<NodeId, NodeType>,
}

impl ForceLayout {
    pub fn new(config: LayoutConfig) -> Self {
        Self {
            config,
            positions: HashMap::new(),
            velocities: HashMap::new(),
            node_ids: Vec::new(),
            edges: Vec::new(),
            max_displacement: 100.0,
            tick: 0,
            converged: false,
            node_names: HashMap::new(),
            node_types: HashMap::new(),
        }
    }

    /// Load graph topology into the layout engine.
    /// Preserves existing positions for nodes that still exist.
    pub fn load_graph(&mut self, graph: &KnowledgeGraph) {
        let old_positions = std::mem::take(&mut self.positions);

        self.node_ids.clear();
        self.velocities.clear();
        self.edges.clear();
        self.node_names.clear();
        self.node_types.clear();
        self.positions.clear();

        // Collect node IDs and metadata
        let node_count = graph.node_count();
        let spread = (node_count as f64).sqrt() * self.config.ideal_edge_length;

        for (id, node) in graph.all_nodes() {
            self.node_ids.push(*id);
            self.node_names.insert(*id, node.name.clone());
            self.node_types.insert(*id, node.node_type);

            // Reuse old position or generate random initial position
            let pos = old_positions.get(id).copied().unwrap_or_else(|| {
                // Deterministic pseudo-random based on UUID bytes
                let bytes = id.as_bytes();
                let x = ((bytes[0] as f64 / 255.0) - 0.5) * spread;
                let y = ((bytes[1] as f64 / 255.0) - 0.5) * spread;
                Vec2::new(x, y)
            });
            self.positions.insert(*id, pos);
            self.velocities.insert(*id, Vec2::ZERO);
        }

        // Collect edges and compute degrees
        for node in graph.nodes() {
            let neighbors = graph.neighbors(&node.id, 1);
            for (_neighbor_id, edge, _depth) in &neighbors {
                self.edges.push(edge.clone());
            }
        }

        // Wake up the simulation
        self.converged = false;
        self.max_displacement = 100.0;
        self.tick = 0;
    }

    /// Check if the simulation has converged.
    pub fn is_converged(&self) -> bool {
        self.converged
    }

    /// Run one simulation tick. Returns true if positions changed significantly.
    pub fn tick(&mut self) -> bool {
        if self.converged || self.node_ids.is_empty() {
            return false;
        }

        self.tick += 1;

        // Build position array for quadtree
        let pos_array: Vec<Vec2> = self.node_ids.iter().map(|id| self.positions[id]).collect();

        // --- Step 1: Repulsive forces via Barnes-Hut ---
        let tree = QuadTree::build(&pos_array);
        let mut forces: HashMap<NodeId, Vec2> = HashMap::with_capacity(self.node_ids.len());

        for (i, &id) in self.node_ids.iter().enumerate() {
            let repulsive = tree.compute_force(
                pos_array[i],
                self.config.theta,
                self.config.repulsion_constant,
            );
            forces.insert(id, repulsive);
        }

        // --- Step 2: Attractive forces along edges ---
        for edge in &self.edges {
            let pos_a = match self.positions.get(&edge.source) {
                Some(p) => *p,
                None => continue,
            };
            let pos_b = match self.positions.get(&edge.target) {
                Some(p) => *p,
                None => continue,
            };

            let diff = pos_b - pos_a;
            let dist = diff.length().max(0.1);
            let direction = diff.normalized();

            // Spring force: F = k * log(d / ideal_length), scaled by edge weight
            let force_mag =
                self.config.attraction_constant * (dist / self.config.ideal_edge_length).ln();
            let force = direction * force_mag * edge.weight;

            forces.entry(edge.source).and_modify(|f| *f += force);
            forces
                .entry(edge.target)
                .and_modify(|f| *f += force * -1.0);
        }

        // --- Step 3: Apply forces, update velocities and positions ---
        let mut total_movement = 0.0;

        for &id in &self.node_ids {
            let force = forces.get(&id).copied().unwrap_or(Vec2::ZERO);
            let vel = self.velocities.get_mut(&id).unwrap();

            // Update velocity
            *vel = (*vel + force) * self.config.damping;

            // Clamp displacement
            let displacement = vel.length().min(self.max_displacement);
            let move_dir = vel.normalized();
            let delta = move_dir * displacement;

            // Update position
            let pos = self.positions.get_mut(&id).unwrap();
            *pos += delta;

            total_movement += displacement;
        }

        // --- Step 4: Cooling ---
        self.max_displacement *= self.config.cooling_rate;

        // --- Step 5: Convergence check ---
        let avg_movement = if self.node_ids.is_empty() {
            0.0
        } else {
            total_movement / self.node_ids.len() as f64
        };

        if avg_movement < self.config.min_movement || self.max_displacement < self.config.min_movement {
            self.converged = true;
        }

        true
    }

    /// Wake the simulation (e.g., after graph mutation or user drag).
    pub fn wake(&mut self) {
        self.converged = false;
        self.max_displacement = 20.0; // gentler restart than initial
    }

    /// Apply tree layout from a root node.
    fn compute_tree_layout(&mut self, root: NodeId) {
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut layers: Vec<Vec<NodeId>> = Vec::new();

        visited.insert(root);
        queue.push_back((root, 0usize));

        while let Some((id, depth)) = queue.pop_front() {
            while layers.len() <= depth {
                layers.push(Vec::new());
            }
            layers[depth].push(id);

            // Find neighbors (outgoing edges)
            for edge in &self.edges {
                let neighbor = if edge.source == id {
                    edge.target
                } else if edge.target == id {
                    edge.source
                } else {
                    continue;
                };
                if visited.insert(neighbor) && depth < 3 {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        // Place nodes: each layer at increasing y, nodes spread horizontally
        let spacing_y = self.config.ideal_edge_length;
        for (depth, layer) in layers.iter().enumerate() {
            for (i, &id) in layer.iter().enumerate() {
                let x = (i as f64 - layer.len() as f64 / 2.0) * self.config.ideal_edge_length
                    + self.config.ideal_edge_length / 2.0;
                let y = depth as f64 * spacing_y;
                self.positions.insert(id, Vec2::new(x, y));
            }

            // Nodes not reached stay at current position
        }

        // Place unreached nodes below
        let unreached_y = (layers.len() as f64 + 1.0) * spacing_y;
        let mut unreached_x = 0.0;
        for &id in &self.node_ids {
            if !visited.contains(&id) {
                self.positions
                    .insert(id, Vec2::new(unreached_x, unreached_y));
                unreached_x += self.config.ideal_edge_length * 0.5;
            }
        }
    }

    /// Apply radial layout centered on a node.
    fn compute_radial_layout(&mut self, center_id: NodeId) {
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut rings: Vec<Vec<NodeId>> = Vec::new();

        visited.insert(center_id);
        self.positions.insert(center_id, Vec2::ZERO);
        queue.push_back((center_id, 0usize));

        while let Some((id, depth)) = queue.pop_front() {
            while rings.len() <= depth {
                rings.push(Vec::new());
            }
            if depth > 0 {
                rings[depth].push(id);
            }

            for edge in &self.edges {
                let neighbor = if edge.source == id {
                    edge.target
                } else if edge.target == id {
                    edge.source
                } else {
                    continue;
                };
                if visited.insert(neighbor) && depth < 3 {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        // Place nodes in concentric rings
        for (depth, ring) in rings.iter().enumerate() {
            if ring.is_empty() || depth == 0 {
                continue;
            }
            let radius = depth as f64 * self.config.ideal_edge_length;
            let angle_step = std::f64::consts::TAU / ring.len() as f64;
            for (i, &id) in ring.iter().enumerate() {
                let angle = i as f64 * angle_step;
                let x = radius * angle.cos();
                let y = radius * angle.sin();
                self.positions.insert(id, Vec2::new(x, y));
            }
        }

        // Unreached nodes in outer ring
        let outer_radius = (rings.len() as f64 + 1.0) * self.config.ideal_edge_length;
        let unreached: Vec<NodeId> = self
            .node_ids
            .iter()
            .filter(|id| !visited.contains(id))
            .copied()
            .collect();
        if !unreached.is_empty() {
            let angle_step = std::f64::consts::TAU / unreached.len() as f64;
            for (i, id) in unreached.iter().enumerate() {
                let angle = i as f64 * angle_step;
                self.positions.insert(
                    *id,
                    Vec2::new(outer_radius * angle.cos(), outer_radius * angle.sin()),
                );
            }
        }
    }

    /// Switch to a specific view mode. For tree/radial, requires a selected node.
    pub fn apply_view_mode(&mut self, mode: ViewMode, selected: Option<NodeId>) {
        match mode {
            ViewMode::ForceDirected => {
                // Reset and let simulation run
                self.wake();
            }
            ViewMode::Tree => {
                let root = selected
                    .or_else(|| self.node_ids.first().copied())
                    .unwrap_or_default();
                self.compute_tree_layout(root);
                self.converged = true; // static layout
            }
            ViewMode::Radial => {
                let center = selected
                    .or_else(|| self.node_ids.first().copied())
                    .unwrap_or_default();
                self.compute_radial_layout(center);
                self.converged = true;
            }
        }
    }

    /// Build a snapshot for the renderer.
    pub fn snapshot(
        &self,
        pinned: &std::collections::HashSet<NodeId>,
        excluded: &std::collections::HashSet<NodeId>,
        anchors: &std::collections::HashSet<NodeId>,
        in_context: &std::collections::HashSet<NodeId>,
    ) -> GraphSnapshot {
        let mut node_views = HashMap::new();

        for &id in &self.node_ids {
            let pos = self.positions.get(&id).copied().unwrap_or(Vec2::ZERO);
            let name = self.node_names.get(&id).cloned().unwrap_or_default();
            let node_type = self.node_types.get(&id).copied().unwrap_or(NodeType::Function);
            let context_state = if excluded.contains(&id) {
                ContextState::Excluded
            } else if pinned.contains(&id) {
                ContextState::Pinned
            } else if anchors.contains(&id) {
                ContextState::Anchor
            } else if in_context.contains(&id) {
                ContextState::InContext
            } else {
                ContextState::Indexed
            };

            node_views.insert(
                id,
                NodeView {
                    id,
                    name,
                    node_type,
                    position: pos,
                    context_state,
                },
            );
        }

        let edge_views: Vec<EdgeView> = self
            .edges
            .iter()
            .map(|e| EdgeView {
                source: e.source,
                target: e.target,
                edge_type: e.edge_type,
                weight: e.weight,
            })
            .collect();

        GraphSnapshot {
            positions: self.positions.clone(),
            node_views,
            edges: edge_views,
            timestamp: Instant::now(),
            converged: self.converged,
            tick: self.tick,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use obi_core::graph::KnowledgeGraph;
    use obi_core::node::{Language, SemanticNode};
    use obi_core::edge::{Edge, EdgeType};

    fn make_test_graph(n: usize) -> KnowledgeGraph {
        let mut g = KnowledgeGraph::new();
        let mut ids = Vec::new();
        for i in 0..n {
            let node = SemanticNode::new(
                NodeType::Function,
                format!("func_{}", i),
                std::path::PathBuf::from("test.rs"),
                i..i + 1,
                Language::Rust,
                &format!("fn func_{}() {{}}", i),
            );
            ids.push(node.id);
            g.add_node(node);
        }
        // Chain edges: 0→1→2→...
        for i in 0..n.saturating_sub(1) {
            g.add_edge(Edge::new(ids[i], ids[i + 1], EdgeType::StaticCall));
        }
        g
    }

    #[test]
    fn test_layout_loads_graph() {
        let graph = make_test_graph(5);
        let mut layout = ForceLayout::new(LayoutConfig::default());
        layout.load_graph(&graph);
        assert_eq!(layout.node_ids.len(), 5);
    }

    #[test]
    fn test_layout_converges() {
        let graph = make_test_graph(10);
        let config = LayoutConfig {
            cooling_rate: 0.99, // faster cooling for test
            ..LayoutConfig::default()
        };
        let mut layout = ForceLayout::new(config);
        layout.load_graph(&graph);

        for _ in 0..10000 {
            if !layout.tick() {
                break;
            }
        }
        assert!(layout.is_converged());
    }

    #[test]
    fn test_snapshot() {
        let graph = make_test_graph(3);
        let mut layout = ForceLayout::new(LayoutConfig::default());
        layout.load_graph(&graph);
        layout.tick();

        let snap = layout.snapshot(&Default::default(), &Default::default(), &Default::default(), &Default::default());
        assert_eq!(snap.node_views.len(), 3);
        assert!(!snap.edges.is_empty());
    }
}
