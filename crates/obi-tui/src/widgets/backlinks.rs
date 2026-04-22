use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use obi_core::edge::EdgeType;
use obi_core::graph::KnowledgeGraph;
use obi_core::node::NodeId;

/// Backlink panel — shows "Referenced by" for the selected node.
/// Queries incoming edges from the KnowledgeGraph.
pub struct BacklinkPanel {
    /// Currently displayed backlinks.
    entries: Vec<BacklinkEntry>,
    /// Name of the node we're showing backlinks for.
    node_name: Option<String>,
    /// Scroll offset.
    scroll: usize,
}

struct BacklinkEntry {
    source_name: String,
    edge_type: EdgeType,
    weight: f64,
    file_path: String,
}

impl BacklinkPanel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            node_name: None,
            scroll: 0,
        }
    }

    /// Update backlinks for a selected node.
    pub fn update(&mut self, node_id: Option<NodeId>, graph: &KnowledgeGraph) {
        self.entries.clear();
        self.scroll = 0;

        let node_id = match node_id {
            Some(id) => id,
            None => {
                self.node_name = None;
                return;
            }
        };

        let node = match graph.get_node(&node_id) {
            Some(n) => n,
            None => {
                self.node_name = None;
                return;
            }
        };

        self.node_name = Some(node.name.clone());

        // Get all incoming edges (nodes that reference this node)
        let incoming = graph.incoming_edges(&node_id);

        for edge in incoming {
            if let Some(source_node) = graph.get_node(&edge.source) {
                self.entries.push(BacklinkEntry {
                    source_name: source_node.name.clone(),
                    edge_type: edge.edge_type,
                    weight: edge.weight,
                    file_path: source_node.file_path.to_string_lossy().to_string(),
                });
            }
        }

        // Sort by weight descending (most relevant first)
        self.entries
            .sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Scroll up.
    #[allow(dead_code)]
    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// Scroll down.
    #[allow(dead_code)]
    pub fn scroll_down(&mut self) {
        if self.scroll + 1 < self.entries.len() {
            self.scroll += 1;
        }
    }

    /// Render the backlink panel.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let title = match &self.node_name {
            Some(name) => format!(" Referenced by [{}] ", name),
            None => " Backlinks ".to_string(),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.entries.is_empty() {
            let msg = if self.node_name.is_some() {
                "No references found"
            } else {
                "Select a node in the graph"
            };
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .render(inner, buf);
            return;
        }

        let lines: Vec<Line> = self
            .entries
            .iter()
            .skip(self.scroll)
            .take(inner.height as usize)
            .map(|entry| {
                let edge_label = edge_type_label(entry.edge_type);
                let edge_color = edge_type_color(entry.edge_type);

                Line::from(vec![
                    Span::styled(
                        format!("{} ", edge_label),
                        Style::default().fg(edge_color),
                    ),
                    Span::styled(
                        &entry.source_name,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", short_path(&entry.file_path)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            })
            .collect();

        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}

fn edge_type_label(edge_type: EdgeType) -> &'static str {
    match edge_type {
        EdgeType::UserLink => "[[link]]",
        EdgeType::StaticCall => "calls",
        EdgeType::StaticImport => "imports",
        EdgeType::StaticTypeRef => "type-ref",
        EdgeType::Semantic => "similar",
    }
}

fn edge_type_color(edge_type: EdgeType) -> Color {
    match edge_type {
        EdgeType::UserLink => Color::White,
        EdgeType::StaticCall => Color::Cyan,
        EdgeType::StaticImport => Color::Green,
        EdgeType::StaticTypeRef => Color::Yellow,
        EdgeType::Semantic => Color::Blue,
    }
}

/// Shorten file path for display (last 2 components).
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obi_core::edge::Edge;
    use obi_core::node::{Language, SemanticNode};

    fn make_node(name: &str) -> SemanticNode {
        SemanticNode::new(
            obi_core::node::NodeType::Function,
            name.to_string(),
            "src/test.rs".into(),
            0..10,
            Language::Rust,
            &format!("fn {}() {{}}", name),
        )
    }

    #[test]
    fn test_backlink_panel_empty() {
        let panel = BacklinkPanel::new();
        assert!(panel.entries.is_empty());
        assert!(panel.node_name.is_none());
    }

    #[test]
    fn test_backlink_panel_update() {
        let mut graph = KnowledgeGraph::new();
        let n1 = make_node("caller");
        let n2 = make_node("callee");
        let id1 = n1.id;
        let id2 = n2.id;
        graph.add_node(n1);
        graph.add_node(n2);
        graph.add_edge(Edge::new(id1, id2, EdgeType::StaticCall));

        let mut panel = BacklinkPanel::new();
        panel.update(Some(id2), &graph);

        assert_eq!(panel.node_name.as_deref(), Some("callee"));
        assert_eq!(panel.entries.len(), 1);
        assert_eq!(panel.entries[0].source_name, "caller");
    }

    #[test]
    fn test_backlink_panel_no_node() {
        let graph = KnowledgeGraph::new();
        let mut panel = BacklinkPanel::new();
        panel.update(None, &graph);
        assert!(panel.entries.is_empty());
        assert!(panel.node_name.is_none());
    }

    #[test]
    fn test_short_path() {
        assert_eq!(short_path("src/parser/rust.rs"), "parser/rust.rs");
        assert_eq!(short_path("test.rs"), "test.rs");
        assert_eq!(short_path("a/b"), "a/b");
    }

    #[test]
    fn test_edge_type_labels() {
        assert_eq!(edge_type_label(EdgeType::UserLink), "[[link]]");
        assert_eq!(edge_type_label(EdgeType::StaticCall), "calls");
        assert_eq!(edge_type_label(EdgeType::Semantic), "similar");
    }
}
