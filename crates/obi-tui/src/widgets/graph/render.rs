use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use obi_core::edge::EdgeType;
use obi_core::node::{NodeId, NodeType};

use super::braille::BrailleCanvas;
use super::snapshot::{ContextState, FilterMode, GraphSnapshot, NodeView, ViewMode};
use super::viewport::{Viewport, ZoomLevel};

// ─── Node symbols ───────────────────────────────────────────────────────────

/// Unicode symbol for each node type — large, clearly visible.
fn node_symbol(node_type: NodeType, is_selected: bool) -> &'static str {
    if is_selected {
        return "◉";
    }
    match node_type {
        NodeType::Function => "●",
        NodeType::Method => "●",
        NodeType::Struct => "◆",
        NodeType::Trait => "▲",
        NodeType::Constant => "■",
        NodeType::File => "◈",
        NodeType::Note => "◎",
    }
}

// ─── Color palette ──────────────────────────────────────────────────────────
// Vibrant, modern colors inspired by Obsidian's graph view.

fn node_color(node: &NodeView) -> Color {
    match node.context_state {
        ContextState::Pinned => Color::Rgb(255, 255, 255),     // bright white
        ContextState::Excluded => Color::Rgb(255, 80, 80),     // soft red
        ContextState::Anchor => Color::Rgb(255, 215, 0),       // gold
        ContextState::InContext => match node.node_type {
            NodeType::Note => Color::Rgb(80, 250, 123),        // neon green
            _ => Color::Rgb(100, 180, 255),                    // bright sky blue
        },
        ContextState::Indexed => match node.node_type {
            NodeType::Note => Color::Rgb(60, 130, 80),         // muted green
            _ => Color::Rgb(90, 120, 200),                     // muted blue
        },
    }
}

fn edge_color(edge_type: EdgeType) -> Color {
    match edge_type {
        EdgeType::UserLink => Color::Rgb(160, 160, 180),      // light gray-blue
        EdgeType::StaticCall => Color::Rgb(60, 60, 80),       // subtle dark
        EdgeType::StaticImport => Color::Rgb(60, 60, 80),
        EdgeType::StaticTypeRef => Color::Rgb(60, 60, 80),
        EdgeType::Semantic => Color::Rgb(70, 100, 160),       // soft blue
    }
}

// ─── Main render function ───────────────────────────────────────────────────

pub fn render_graph(
    frame: &mut Frame,
    area: Rect,
    snapshot: &GraphSnapshot,
    viewport: &Viewport,
    selected: Option<NodeId>,
    filter: FilterMode,
    view_mode: ViewMode,
    search_query: &str,
    search_active: bool,
    max_visible_nodes: usize,
) {
    if area.width < 4 || area.height < 4 {
        return;
    }

    let canvas_w = area.width as usize;
    let canvas_h = area.height.saturating_sub(1) as usize; // 1 row for status
    if canvas_w == 0 || canvas_h == 0 {
        return;
    }

    let mut canvas = BrailleCanvas::new(canvas_w, canvas_h);
    let visible_rect = viewport.visible_rect();
    let zoom_level = viewport.zoom_level();

    // ── Draw edges (braille layer — behind everything) ──────────────────

    for edge in &snapshot.edges {
        let pos_a = match snapshot.positions.get(&edge.source) {
            Some(p) => *p,
            None => continue,
        };
        let pos_b = match snapshot.positions.get(&edge.target) {
            Some(p) => *p,
            None => continue,
        };

        if !visible_rect.contains(pos_a) && !visible_rect.contains(pos_b) {
            continue;
        }

        let (px_a, py_a) = viewport.world_to_pixel(pos_a);
        let (px_b, py_b) = viewport.world_to_pixel(pos_b);
        let color = edge_color(edge.edge_type);

        match edge.edge_type {
            EdgeType::Semantic => {
                canvas.draw_dashed_line(px_a, py_a, px_b, py_b, color, 4, 3);
            }
            EdgeType::UserLink => {
                canvas.draw_thick_line(px_a, py_a, px_b, py_b, color);
            }
            _ => {
                if edge.weight > 0.75 {
                    canvas.draw_thick_line(px_a, py_a, px_b, py_b, color);
                } else {
                    canvas.draw_line(px_a, py_a, px_b, py_b, color);
                }
            }
        }
    }

    // ── Collect and filter visible nodes ─────────────────────────────────

    let mut visible_nodes: Vec<&NodeView> = snapshot
        .node_views
        .values()
        .filter(|nv| {
            if !visible_rect.contains(nv.position) {
                return false;
            }
            if !filter.matches(nv) {
                return false;
            }
            if search_active && !search_query.is_empty() {
                return nv.name.to_lowercase().contains(&search_query.to_lowercase());
            }
            true
        })
        .collect();

    let show_labels = visible_nodes.len() <= max_visible_nodes;

    visible_nodes.sort_by(|a, b| {
        let da = a.position.distance(viewport.center);
        let db = b.position.distance(viewport.center);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });

    // ── Render braille canvas (edges only) to frame ─────────────────────

    let braille_rows = canvas.to_spans();
    for (row_idx, spans) in braille_rows.iter().enumerate() {
        let y = area.y + row_idx as u16;
        if y >= area.y + area.height.saturating_sub(1) {
            break;
        }
        let line = Line::from(spans.clone());
        frame.render_widget(Paragraph::new(vec![line]), Rect::new(area.x, y, area.width, 1));
    }

    // ── Render nodes as Unicode symbols + labels (text layer) ───────────
    // This goes ON TOP of braille, giving nodes clear visibility.

    for nv in &visible_nodes {
        let (cell_x, cell_y) = viewport.world_to_cell(nv.position);
        if cell_x < 0 || cell_y < 0 {
            continue;
        }
        let abs_x = area.x + cell_x as u16;
        let abs_y = area.y + cell_y as u16;

        // Don't render outside the graph area (leave last row for status)
        if abs_y >= area.y + area.height.saturating_sub(1) || abs_x >= area.x + area.width {
            continue;
        }

        let is_selected = selected == Some(nv.id);
        let color = node_color(nv);
        let symbol = node_symbol(nv.node_type, is_selected);

        // Build the node display string
        let max_remaining = (area.x + area.width).saturating_sub(abs_x) as usize;

        let display = if !show_labels {
            // Too many nodes — just show the symbol
            symbol.to_string()
        } else {
            match zoom_level {
                ZoomLevel::Far => {
                    // Symbol only — labels clutter at this zoom
                    symbol.to_string()
                }
                ZoomLevel::Medium => {
                    // Symbol + short name
                    if nv.name.len() > 10 {
                        format!("{} {}…", symbol, &nv.name[..9])
                    } else {
                        format!("{} {}", symbol, nv.name)
                    }
                }
                ZoomLevel::Close => {
                    // Symbol + full name
                    format!("{} {}", symbol, nv.name)
                }
            }
        };

        let display: String = display.chars().take(max_remaining).collect();

        // Style: selected nodes are bold with brighter color
        let style = if is_selected {
            Style::default()
                .fg(Color::Rgb(255, 215, 0)) // gold
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };

        // Render the node symbol + label
        let span = Span::styled(display, style);
        frame.render_widget(
            Paragraph::new(Line::from(vec![span])),
            Rect::new(abs_x, abs_y, max_remaining as u16, 1),
        );
    }

    // ── Status line ─────────────────────────────────────────────────────

    render_status_line(
        frame,
        Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1),
        snapshot,
        viewport,
        selected,
        filter,
        view_mode,
        search_query,
        search_active,
    );
}

// ─── Status line ────────────────────────────────────────────────────────────

fn render_status_line(
    frame: &mut Frame,
    area: Rect,
    snapshot: &GraphSnapshot,
    viewport: &Viewport,
    selected: Option<NodeId>,
    filter: FilterMode,
    view_mode: ViewMode,
    search_query: &str,
    search_active: bool,
) {
    let mut parts: Vec<Span> = Vec::new();

    // Node/edge count
    parts.push(Span::styled(
        format!(" {}n {}e", snapshot.node_views.len(), snapshot.edges.len()),
        Style::default().fg(Color::Rgb(100, 100, 120)),
    ));

    // Zoom
    parts.push(Span::styled(
        format!(" z:{:.1}", viewport.zoom),
        Style::default().fg(Color::Rgb(100, 100, 120)),
    ));

    // Convergence
    if snapshot.converged {
        parts.push(Span::styled(
            " ✓stable",
            Style::default().fg(Color::Rgb(80, 200, 120)),
        ));
    } else {
        let age_ms = snapshot.timestamp.elapsed().as_millis();
        parts.push(Span::styled(
            format!(" ⟳t:{} {}ms", snapshot.tick, age_ms),
            Style::default().fg(Color::Rgb(255, 180, 50)),
        ));
    }

    // View mode
    parts.push(Span::styled(
        format!(" [{}]", view_mode.label()),
        Style::default().fg(Color::Rgb(100, 100, 120)),
    ));

    // Filter
    if filter != FilterMode::All {
        parts.push(Span::styled(
            format!(" [{}]", filter.label()),
            Style::default().fg(Color::Rgb(80, 200, 230)),
        ));
    }

    // Selected node
    if let Some(sel_id) = selected {
        if let Some(nv) = snapshot.node_views.get(&sel_id) {
            parts.push(Span::styled(
                format!(" ▸ {}", nv.name),
                Style::default()
                    .fg(Color::Rgb(255, 215, 0))
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    // Search
    if search_active {
        parts.push(Span::styled(
            format!(" /{}", search_query),
            Style::default().fg(Color::Rgb(80, 200, 230)),
        ));
    }

    let line = Line::from(parts);
    frame.render_widget(
        Paragraph::new(vec![line]).style(Style::default().bg(Color::Rgb(20, 20, 30))),
        area,
    );
}
