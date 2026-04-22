pub mod braille;
pub mod layout;
pub mod layout_thread;
pub mod math;
pub mod quadtree;
pub mod render;
pub mod snapshot;
pub mod viewport;

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::Frame;

use obi_core::graph::KnowledgeGraph;
use obi_core::node::NodeId;

use self::layout::LayoutConfig;
use self::layout_thread::LayoutThread;
use self::snapshot::{FilterMode, GraphSnapshot, ViewMode};
use self::viewport::Viewport;

/// Interactive knowledge graph widget with force-directed layout,
/// Braille rendering, and Obsidian-style navigation.
pub struct GraphWidget {
    /// Background layout thread handle.
    layout_thread: Option<LayoutThread>,
    /// Camera viewport (pan, zoom, culling).
    viewport: Viewport,
    /// Latest snapshot from layout thread.
    snapshot: GraphSnapshot,
    /// Currently selected node.
    selected: Option<NodeId>,
    /// Selection index for j/k navigation.
    selection_index: usize,
    /// Current view mode (force/tree/radial).
    view_mode: ViewMode,
    /// Current filter mode.
    filter_mode: FilterMode,
    /// Fuzzy search query.
    search_query: String,
    /// Whether search input is active.
    search_active: bool,
    /// Pinned nodes (force include in context).
    pinned: HashSet<NodeId>,
    /// Excluded nodes (force exclude from context).
    excluded: HashSet<NodeId>,
    /// Whether graph has been loaded.
    loaded: bool,
    /// Max visible nodes before LOD kicks in.
    max_visible_nodes: usize,
    /// Sorted node IDs for j/k navigation (screen position order).
    sorted_node_ids: Vec<NodeId>,
    /// Anchor node IDs from context builder (direct query matches).
    context_anchors: HashSet<NodeId>,
    /// In-context node IDs from context builder.
    context_in_context: HashSet<NodeId>,
}

impl GraphWidget {
    pub fn new() -> Self {
        Self {
            layout_thread: None,
            viewport: Viewport::new(),
            snapshot: GraphSnapshot::default(),
            selected: None,
            selection_index: 0,
            view_mode: ViewMode::ForceDirected,
            filter_mode: FilterMode::All,
            search_query: String::new(),
            search_active: false,
            pinned: HashSet::new(),
            excluded: HashSet::new(),
            loaded: false,
            max_visible_nodes: 500,
            sorted_node_ids: Vec::new(),
            context_anchors: HashSet::new(),
            context_in_context: HashSet::new(),
        }
    }

    /// Load a knowledge graph for visualization.
    pub fn load_graph(&mut self, graph: KnowledgeGraph) {
        if self.layout_thread.is_none() {
            self.layout_thread = Some(LayoutThread::spawn(LayoutConfig::default()));
        }

        if let Some(ref lt) = self.layout_thread {
            lt.load_graph(graph);
        }
        self.loaded = true;
    }

    /// Update the graph (e.g., after indexer re-index).
    pub fn update_graph(&mut self, graph: KnowledgeGraph) {
        self.load_graph(graph);
    }

    /// Pull latest snapshot from layout thread and update viewport.
    fn refresh_snapshot(&mut self) {
        if let Some(ref lt) = self.layout_thread {
            self.snapshot = lt.snapshot();

            // Sort node IDs by screen position for j/k navigation
            let viewport = &self.viewport;
            let mut ids: Vec<(NodeId, (f64, f64))> = self
                .snapshot
                .node_views
                .values()
                .map(|nv| (nv.id, viewport.world_to_pixel(nv.position)))
                .collect();
            // Sort by y then x (top-to-bottom, left-to-right)
            ids.sort_by(|a, b| {
                a.1 .1
                    .partial_cmp(&b.1 .1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        a.1 .0
                            .partial_cmp(&b.1 .0)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            });
            self.sorted_node_ids = ids.into_iter().map(|(id, _)| id).collect();
        }
    }

    /// Render the graph widget.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        self.viewport.set_size(area.width, area.height);

        if !self.loaded {
            render_placeholder(frame, area);
            return;
        }

        // Pull latest layout data
        self.refresh_snapshot();

        // Advance viewport animation
        self.viewport.tick_animation();

        // Auto fit-all during early ticks while layout stabilizes
        if self.snapshot.tick <= 5 && !self.snapshot.positions.is_empty() {
            self.viewport.fit_all(&self.snapshot.positions, false);
        }

        render::render_graph(
            frame,
            area,
            &self.snapshot,
            &self.viewport,
            self.selected,
            self.filter_mode,
            self.view_mode,
            &self.search_query,
            self.search_active,
            self.max_visible_nodes,
        );
    }

    /// Handle keyboard input when graph panel is focused.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Search mode intercepts all input
        if self.search_active {
            match key.code {
                KeyCode::Esc => {
                    self.search_active = false;
                    self.search_query.clear();
                }
                KeyCode::Enter => {
                    // Select first matching node
                    if !self.search_query.is_empty() {
                        let query = self.search_query.to_lowercase();
                        if let Some(nv) = self
                            .snapshot
                            .node_views
                            .values()
                            .find(|nv| nv.name.to_lowercase().contains(&query))
                        {
                            self.selected = Some(nv.id);
                            self.viewport.focus_node(nv.position);
                        }
                    }
                    self.search_active = false;
                }
                KeyCode::Char(c) => {
                    self.search_query.push(c);
                }
                KeyCode::Backspace => {
                    self.search_query.pop();
                }
                _ => {}
            }
            return;
        }

        match key.code {
            // --- Viewport Navigation ---
            KeyCode::Char('h') => self.viewport.pan(-10.0, 0.0),
            KeyCode::Char('l') => self.viewport.pan(10.0, 0.0),
            KeyCode::Char('k') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.sorted_node_ids.is_empty() {
                    self.viewport.pan(0.0, -10.0);
                } else {
                    // Move selection up
                    if self.selection_index > 0 {
                        self.selection_index -= 1;
                    } else {
                        self.selection_index = self.sorted_node_ids.len().saturating_sub(1);
                    }
                    self.selected = self.sorted_node_ids.get(self.selection_index).copied();
                }
            }
            KeyCode::Char('j') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if self.sorted_node_ids.is_empty() {
                    self.viewport.pan(0.0, 10.0);
                } else {
                    // Move selection down
                    self.selection_index =
                        (self.selection_index + 1) % self.sorted_node_ids.len().max(1);
                    self.selected = self.sorted_node_ids.get(self.selection_index).copied();
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.viewport.zoom_by(1.2),
            KeyCode::Char('-') => self.viewport.zoom_by(1.0 / 1.2),
            KeyCode::Esc => {
                // Fit-all: reset viewport to show entire graph
                self.viewport.fit_all(&self.snapshot.positions, true);
                self.selected = None;
            }

            // --- Node Focus ---
            KeyCode::Enter => {
                if let Some(sel_id) = self.selected {
                    if let Some(nv) = self.snapshot.node_views.get(&sel_id) {
                        self.viewport.focus_node(nv.position);
                    }
                }
            }

            // --- Tab: cycle through nodes ---
            KeyCode::Tab => {
                if !self.sorted_node_ids.is_empty() {
                    self.selection_index =
                        (self.selection_index + 1) % self.sorted_node_ids.len();
                    self.selected = self.sorted_node_ids.get(self.selection_index).copied();
                    if let Some(sel_id) = self.selected {
                        if let Some(nv) = self.snapshot.node_views.get(&sel_id) {
                            self.viewport.focus_node(nv.position);
                        }
                    }
                }
            }

            // --- Search ---
            KeyCode::Char('/') => {
                self.search_active = true;
                self.search_query.clear();
            }

            // --- Filter ---
            KeyCode::Char('f') => {
                self.filter_mode = self.filter_mode.next();
            }

            // --- Layout Mode ---
            KeyCode::Char('g') => {
                self.view_mode = self.view_mode.next();
                if let Some(ref lt) = self.layout_thread {
                    lt.set_view_mode(self.view_mode, self.selected);
                }
            }

            // --- Pin/Exclude ---
            KeyCode::Char('p') if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                if let Some(sel_id) = self.selected {
                    if self.pinned.contains(&sel_id) {
                        self.pinned.remove(&sel_id);
                    } else {
                        self.excluded.remove(&sel_id);
                        self.pinned.insert(sel_id);
                    }
                    self.sync_context();
                }
            }
            KeyCode::Char('P') => {
                if let Some(sel_id) = self.selected {
                    if self.excluded.contains(&sel_id) {
                        self.excluded.remove(&sel_id);
                    } else {
                        self.pinned.remove(&sel_id);
                        self.excluded.insert(sel_id);
                    }
                    self.sync_context();
                }
            }

            _ => {}
        }
    }

    /// Sync pinned/excluded state to layout thread.
    fn sync_context(&self) {
        if let Some(ref lt) = self.layout_thread {
            lt.update_context_states(
                self.pinned.clone(),
                self.excluded.clone(),
                self.context_anchors.clone(),
                self.context_in_context.clone(),
            );
            lt.wake();
        }
    }

    /// Get the currently selected node ID (for backlink panel).
    pub fn selected_node(&self) -> Option<NodeId> {
        self.selected
    }

    /// Handle mouse click inside graph area — select nearest node.
    /// `rel_col`/`rel_row` are relative to the graph inner area.
    pub fn handle_click(&mut self, rel_col: u16, rel_row: u16) {
        // Convert terminal cell to braille pixel coordinates
        let px = rel_col as f64 * 2.0;
        let py = rel_row as f64 * 4.0;
        let world_pos = self.viewport.pixel_to_world(px, py);

        // Find nearest node within a threshold
        let threshold = 5.0 / self.viewport.zoom.max(0.1); // scale threshold by zoom
        let mut best: Option<(NodeId, f64)> = None;

        for (id, pos) in &self.snapshot.positions {
            let dist = (world_pos - *pos).length();
            if dist < threshold {
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((*id, dist));
                }
            }
        }

        if let Some((node_id, _)) = best {
            self.selected = Some(node_id);
            // Update selection index
            if let Some(idx) = self.sorted_node_ids.iter().position(|&id| id == node_id) {
                self.selection_index = idx;
            }
        }
    }

    /// Handle mouse scroll up (zoom in towards selected node).
    pub fn handle_scroll_up(&mut self) {
        if let Some(id) = self.selected {
            if let Some(&pos) = self.snapshot.positions.get(&id) {
                self.viewport.zoom_towards(pos, 1.15);
            } else {
                self.viewport.zoom_by(1.15);
            }
        } else {
            self.viewport.zoom_by(1.15);
        }
        if let Some(ref lt) = self.layout_thread {
            lt.wake();
        }
    }

    /// Handle mouse scroll down (zoom out from center, not locked to node).
    pub fn handle_scroll_down(&mut self) {
        self.viewport.zoom_by(0.87);
        if let Some(ref lt) = self.layout_thread {
            lt.wake();
        }
    }

    /// Get reference to pinned node IDs (for context builder).
    pub fn pinned_ids(&self) -> &HashSet<NodeId> {
        &self.pinned
    }

    /// Get reference to excluded node IDs (for context builder).
    pub fn excluded_ids(&self) -> &HashSet<NodeId> {
        &self.excluded
    }

    /// Update node context states from context builder results.
    /// Anchors get Yellow highlight, in-context nodes get bright colors.
    pub fn set_context_nodes(
        &mut self,
        anchors: HashSet<NodeId>,
        in_context: HashSet<NodeId>,
    ) {
        self.context_anchors = anchors;
        self.context_in_context = in_context;

        // Push updated context to layout thread for snapshot rendering
        if let Some(ref lt) = self.layout_thread {
            lt.update_context_states(
                self.pinned.clone(),
                self.excluded.clone(),
                self.context_anchors.clone(),
                self.context_in_context.clone(),
            );
            lt.wake();
        }
    }
}

/// Render placeholder when no graph is loaded.
fn render_placeholder(frame: &mut Frame, area: Rect) {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let placeholder = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Knowledge Graph",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Run `obi index` to build",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  the knowledge graph.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  h/j/k/l  Pan",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  +/-      Zoom",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  /        Search",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  g        Cycle layout",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  f        Filter",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  p/P      Pin/Exclude",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(placeholder, area);
}
