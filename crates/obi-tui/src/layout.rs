use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    FileTree,
    Editor,
    Graph,
    Chat,
}

impl PanelFocus {}

fn focused_border(focus: PanelFocus, panel: PanelFocus) -> Style {
    if focus == panel {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Main layout drawing function.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let size = frame.area();

    // Vertical: status bar | main content | chat panel
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),           // status bar
            Constraint::Min(10),            // main content
            Constraint::Length(app.chat_height), // chat panel (resizable)
        ])
        .split(size);

    draw_status_bar(frame, vertical[0], app);

    // Update panel boundaries for mouse hit-testing
    app.panel_boundaries.total_width = size.width;
    app.panel_boundaries.total_height = size.height;
    app.panel_boundaries.chat_top_y = vertical[2].y;

    // Auto-calculate graph_width on first frame (50% of editor+graph area)
    if app.show_graph && app.graph_width == 0 {
        let tree_w = if app.show_file_tree { app.file_tree_width } else { 0 };
        app.graph_width = size.width.saturating_sub(tree_w) / 2;
    }

    // Build horizontal constraints based on visible panels
    let mut h_constraints = Vec::new();
    let mut panel_slots: Vec<PanelSlot> = Vec::new();

    if app.show_file_tree {
        h_constraints.push(Constraint::Length(app.file_tree_width));
        panel_slots.push(PanelSlot::FileTree);
    }
    h_constraints.push(Constraint::Min(30)); // editor always visible
    panel_slots.push(PanelSlot::Editor);
    if app.show_graph {
        h_constraints.push(Constraint::Length(app.graph_width));
        panel_slots.push(PanelSlot::Graph);
    }

    let main_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(h_constraints)
        .split(vertical[1]);

    // Update horizontal panel boundaries for mouse drag
    for (i, slot) in panel_slots.iter().enumerate() {
        match slot {
            PanelSlot::FileTree => {
                app.panel_boundaries.file_tree_right_x = main_areas[i].x + main_areas[i].width;
            }
            PanelSlot::Graph => {
                app.panel_boundaries.graph_left_x = main_areas[i].x;
            }
            _ => {}
        }
    }

    for (i, slot) in panel_slots.iter().enumerate() {
        match slot {
            PanelSlot::FileTree => {
                let block = Block::default()
                    .title(" Explorer ")
                    .borders(Borders::ALL)
                    .border_style(focused_border(app.focus, PanelFocus::FileTree));
                let inner = block.inner(main_areas[i]);
                app.panel_boundaries.file_tree_inner = inner;
                frame.render_widget(block, main_areas[i]);
                app.file_tree.render(frame, inner);
            }
            PanelSlot::Editor => {
                let block = Block::default()
                    .title(app.editor.title())
                    .borders(Borders::ALL)
                    .border_style(focused_border(app.focus, PanelFocus::Editor));
                let inner = block.inner(main_areas[i]);
                app.panel_boundaries.editor_inner = inner;
                frame.render_widget(block, main_areas[i]);
                app.editor.render(frame, inner);
            }
            PanelSlot::Graph => {
                // Split graph area: graph (top) + backlinks (bottom)
                let graph_split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(10),     // graph
                        Constraint::Length(8),   // backlink panel
                    ])
                    .split(main_areas[i]);

                let block = Block::default()
                    .title(" Knowledge Graph ")
                    .borders(Borders::ALL)
                    .border_style(focused_border(app.focus, PanelFocus::Graph));
                let inner = block.inner(graph_split[0]);
                app.panel_boundaries.graph_inner = inner;
                frame.render_widget(block, graph_split[0]);
                app.graph.render(frame, inner);

                // Backlink panel
                app.backlink_panel.render(graph_split[1], frame.buffer_mut());
            }
        }
    }

    // Chat panel
    let chat_block = Block::default()
        .title(" obi-wan code ")
        .borders(Borders::ALL)
        .border_style(focused_border(app.focus, PanelFocus::Chat));
    let inner = chat_block.inner(vertical[2]);
    app.panel_boundaries.chat_inner = inner;
    frame.render_widget(chat_block, vertical[2]);
    app.chat.render(frame, inner);

    // Overlays (rendered last, on top)
    app.command_palette.render(frame, size);
    app.settings.render(frame, size);

    // Notification toast overlay
    app.notification.render(frame, size);
}

fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    use ratatui::text::{Line, Span};

    let project_name = app
        .project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "obi-wan".into());

    let mode_str = match app.editor.mode {
        crate::widgets::editor::EditorMode::Normal => "",
        crate::widgets::editor::EditorMode::Insert => " | INSERT",
    };

    let mut spans = vec![
        Span::styled(
            format!("  Obi-Wan -- {}{}", project_name, mode_str),
            Style::default().fg(Color::White),
        ),
    ];

    // Context bar: show when context info is available
    if let Some(info) = app.chat.context_info() {
        spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled("[Context: ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("{}n", info.node_count),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("{}tok", info.total_tokens),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)));
        spans.push(Span::styled(
            format!("\u{2193}{:.0}% saved", info.savings_percent),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::styled("]", Style::default().fg(Color::DarkGray)));
    }

    spans.push(Span::styled(
        " | Ctrl+P Open | Ctrl+B Explorer | Ctrl+G Graph | Ctrl+N Note | Ctrl+T Settings",
        Style::default().fg(Color::DarkGray),
    ));

    let status = ratatui::widgets::Paragraph::new(Line::from(spans))
        .style(Style::default().bg(Color::DarkGray));

    frame.render_widget(status, area);
}

#[derive(Debug)]
enum PanelSlot {
    FileTree,
    Editor,
    Graph,
}
