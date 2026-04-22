use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::event::AppEvent;
use crate::layout::PanelFocus;
use crate::widgets::backlinks::BacklinkPanel;
use crate::widgets::chat::{ChatWidget, ContextInfo, ModelInfo};
use crate::widgets::command::CommandPalette;
use crate::widgets::editor::{EditorMode, EditorWidget};
use crate::widgets::file_tree::FileTreeWidget;
use crate::widgets::graph::GraphWidget;
use crate::widgets::indexing::{IndexingAction, IndexingDialog};
use crate::widgets::notification::NotificationWidget;
use crate::widgets::settings::{SettingsAction, SettingsWidget};

use obi_agent::agent::{spawn_agent, AgentCommand, AgentEvent};
use obi_agent::context::ContextWindow;
use obi_core::config::ObiConfig;
use obi_core::graph::KnowledgeGraph;


pub struct App {
    pub running: bool,
    pub focus: PanelFocus,
    pub file_tree: FileTreeWidget,
    pub editor: EditorWidget,
    pub graph: GraphWidget,
    pub chat: ChatWidget,
    pub command_palette: CommandPalette,
    pub backlink_panel: BacklinkPanel,
    pub project_root: PathBuf,
    pub show_file_tree: bool,
    pub show_graph: bool,
    pub file_tree_width: u16,
    pub graph_width: u16,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    event_rx: mpsc::UnboundedReceiver<AppEvent>,
    /// Latest context window from context builder.
    context_window: Option<ContextWindow>,
    /// Cached knowledge graph for context builder.
    knowledge_graph: Option<Arc<KnowledgeGraph>>,
    /// Agent command sender — sends queries and tool approvals to the agent task.
    agent_cmd_tx: mpsc::UnboundedSender<AgentCommand>,
    /// Pending tool confirmation call ID (if any).
    pending_tool_call_id: Option<String>,
    /// Chat panel height (resizable).
    pub chat_height: u16,
    /// Mouse drag state for panel resizing.
    drag_state: DragState,
    /// Cached panel boundary x-positions (updated each frame).
    pub panel_boundaries: PanelBoundaries,
    /// Settings panel overlay.
    pub settings: SettingsWidget,
    /// Notification toast overlay.
    pub notification: NotificationWidget,
    /// Indexing dialog overlay (shown on startup if index is missing).
    pub indexing: IndexingDialog,
    /// Text selection drag state in chat.
    text_selecting: bool,
    /// Pending clipboard text to write via OSC 52 after next draw.
    pending_clipboard: Option<String>,
}

/// Which border is being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DragTarget {
    /// File tree right border (adjust file_tree_width).
    FileTreeRight,
    /// Graph left border (adjust graph_width).
    GraphLeft,
    /// Chat top border (adjust chat_height).
    ChatTop,
}

#[derive(Debug, Default)]
struct DragState {
    active: Option<DragTarget>,
}

/// Cached panel boundary positions for hit-testing mouse clicks.
#[derive(Debug, Default, Clone)]
pub struct PanelBoundaries {
    /// X position of file tree right border.
    pub file_tree_right_x: u16,
    /// X position of graph left border.
    pub graph_left_x: u16,
    /// Y position of chat top border.
    pub chat_top_y: u16,
    /// Total terminal width.
    pub total_width: u16,
    /// Total terminal height.
    pub total_height: u16,
    /// Inner area of each panel (after borders), for click coordinate mapping.
    pub file_tree_inner: ratatui::layout::Rect,
    pub editor_inner: ratatui::layout::Rect,
    pub graph_inner: ratatui::layout::Rect,
    pub chat_inner: ratatui::layout::Rect,
}

impl App {
    pub fn new(project_root: PathBuf) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let mut graph = GraphWidget::new();
        let mut knowledge_graph = None;

        // Check .obi consistency: both graph.bin and db must exist
        let obi_dir = project_root.join(".obi");
        let graph_path = obi_dir.join("graph.bin");
        let db_path = obi_dir.join("db");
        let graph_exists = graph_path.exists();
        let db_exists = db_path.exists();

        let needs_index = if !graph_exists || !db_exists {
            // Inconsistent state: clean up partial .obi
            if obi_dir.exists() && (graph_exists != db_exists) {
                let _ = std::fs::remove_dir_all(&obi_dir);
            }
            true
        } else {
            false
        };

        // Try to load existing knowledge graph from .obi/graph.bin
        if graph_path.exists() {
            if let Ok(kg) = KnowledgeGraph::load_from_disk(&graph_path) {
                graph.load_graph(kg.clone());
                knowledge_graph = Some(Arc::new(kg));
            }
        }

        let mut indexing = IndexingDialog::new();
        if needs_index {
            indexing.show_prompt();
        }

        // Spawn the agent task
        let (agent_cmd_tx, agent_event_rx) =
            spawn_agent(project_root.clone(), knowledge_graph.clone());

        // Bridge agent events into the unified AppEvent channel
        let app_event_tx = event_tx.clone();
        tokio::spawn(async move {
            let mut agent_event_rx = agent_event_rx;
            while let Some(agent_event) = agent_event_rx.recv().await {
                let app_event = match agent_event {
                    AgentEvent::StreamChunk(text) => AppEvent::StreamChunk(text),
                    AgentEvent::ThinkingChunk(text) => AppEvent::ThinkingChunk(text),
                    AgentEvent::ResponseComplete(text) => AppEvent::ResponseComplete(text),
                    AgentEvent::ToolConfirmation {
                        tool_name,
                        description,
                        args_display,
                        call_id,
                    } => AppEvent::ToolConfirmation {
                        tool_name,
                        description,
                        args_display,
                        call_id,
                    },
                    AgentEvent::PlanGenerated(summary) => AppEvent::PlanGenerated(summary),
                    AgentEvent::ContextBuilding => AppEvent::ContextBuilding,
                    AgentEvent::ContextBuilt(window) => AppEvent::ContextBuilt(window),
                    AgentEvent::ModelInfo { provider, chat_model, embed_model } => {
                        AppEvent::ModelInfo { provider, chat_model, embed_model }
                    }
                    AgentEvent::Error(err) => AppEvent::AgentError(err),
                };
                if app_event_tx.send(app_event).is_err() {
                    break;
                }
            }
        });

        Self {
            running: true,
            focus: PanelFocus::Editor,
            file_tree: FileTreeWidget::new(&project_root),
            editor: EditorWidget::new(),
            graph,
            chat: ChatWidget::new(),
            command_palette: CommandPalette::new(),
            backlink_panel: BacklinkPanel::new(),
            project_root,
            show_file_tree: true,
            show_graph: true,
            file_tree_width: 24,
            graph_width: 0, // 0 = auto-calculate on first frame (50% of editor+graph area)
            event_tx,
            event_rx,
            context_window: None,
            knowledge_graph,
            agent_cmd_tx,
            pending_tool_call_id: None,
            chat_height: 16,
            drag_state: DragState::default(),
            panel_boundaries: PanelBoundaries::default(),
            settings: SettingsWidget::new(),
            notification: NotificationWidget::new(),
            indexing,
            text_selecting: false,
            pending_clipboard: None,
        }
    }

    pub async fn run(&mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            loop {
                if let Ok(ev) = event::read() {
                    let app_event = match ev {
                        Event::Key(key) => AppEvent::Key(key),
                        Event::Mouse(mouse) => {
                            // Filter out Moved events — they cause unnecessary redraws
                            // which make the graph simulation visible as "mouse-driven movement"
                            if matches!(mouse.kind, crossterm::event::MouseEventKind::Moved) {
                                continue;
                            }
                            AppEvent::Mouse(mouse)
                        }
                        Event::Resize(w, h) => AppEvent::Resize(w, h),
                        Event::Paste(text) => AppEvent::Paste(text),
                        _ => continue,
                    };
                    if tx.send(app_event).is_err() {
                        break;
                    }
                }
            }
        });

        while self.running {
            terminal.draw(|frame| crate::layout::draw(frame, self))?;

            // Clipboard: write selected text to a temp file for pbpaste-like access
            if let Some(text) = self.pending_clipboard.take() {
                let clip_path = std::env::temp_dir().join("obi-clipboard.txt");
                let _ = std::fs::write(&clip_path, &text);
                // Also try pbcopy in background (non-blocking, won't affect TUI)
                if cfg!(target_os = "macos") {
                    let text_owned = text;
                    std::thread::spawn(move || {
                        use std::process::{Command, Stdio};
                        use std::io::Write;
                        if let Ok(mut child) = Command::new("pbcopy")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                        {
                            if let Some(ref mut stdin) = child.stdin {
                                let _ = stdin.write_all(text_owned.as_bytes());
                            }
                            let _ = child.wait();
                        }
                    });
                }
            }

            let needs_animation = self.chat.is_animating();

            if needs_animation {
                // Tick at ~8fps for spinner animation during streaming/building
                let tick_duration = tokio::time::Duration::from_millis(120);
                tokio::select! {
                    event = self.event_rx.recv() => {
                        match event {
                            Some(event) => self.handle_event(event),
                            None => self.recover_from_agent_crash(),
                        }
                    }
                    _ = tokio::time::sleep(tick_duration) => {
                        // No event — just redraw for animation
                    }
                }
            } else {
                match self.event_rx.recv().await {
                    Some(event) => self.handle_event(event),
                    None => self.recover_from_agent_crash(),
                }
            }
        }

        Ok(())
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(mouse) => self.handle_mouse(mouse),
            AppEvent::Paste(text) => {
                if self.focus == PanelFocus::Chat {
                    self.chat.handle_paste(text);
                }
            }
            AppEvent::Quit => self.running = false,
            AppEvent::IndexingProgress { done, total } => {
                self.indexing.update_progress(done, total);
            }
            AppEvent::IndexingComplete => {
                let graph_path = self.project_root.join(".obi").join("graph.bin");
                if let Ok(kg) = KnowledgeGraph::load_from_disk(&graph_path) {
                    self.graph.update_graph(kg.clone());
                    self.knowledge_graph = Some(Arc::new(kg));
                    // Reload agent with the new graph
                    let _ = self.agent_cmd_tx.send(AgentCommand::ReloadConfig);
                }
                self.indexing.set_complete();
                self.notification.show("Indexing complete");
            }
            AppEvent::IndexingError(err) => {
                self.indexing.set_error(err);
            }
            AppEvent::IndexingStatus(text) => {
                self.indexing.set_status(text);
            }
            AppEvent::NodesUpdated(_ids) => {
                let graph_path = self.project_root.join(".obi").join("graph.bin");
                if let Ok(kg) = KnowledgeGraph::load_from_disk(&graph_path) {
                    self.graph.update_graph(kg.clone());
                    self.knowledge_graph = Some(Arc::new(kg));
                }
            }

            // ─── Agent events (Phase 5) ─────────────────────────
            AppEvent::StreamChunk(text) => {
                self.chat.append_streaming_text(&text);
            }
            AppEvent::ThinkingChunk(text) => {
                self.chat.append_thinking_text(&text);
            }
            AppEvent::ResponseComplete(text) => {
                self.chat.finish_streaming(text);
            }
            AppEvent::PlanGenerated(summary) => {
                self.chat
                    .push_message("system", format!("Plan:\n{}", summary));
            }
            AppEvent::ToolConfirmation {
                tool_name,
                description,
                args_display,
                call_id,
            } => {
                self.pending_tool_call_id = Some(call_id);
                self.chat.show_tool_confirmation(&tool_name, &description, &args_display);
            }
            AppEvent::AgentError(err) => {
                self.chat.set_building_context(false);
                self.chat.finish_streaming_with_error();
                self.chat
                    .push_message("system", format!("Error: {}", err));
            }
            AppEvent::ModelInfo { provider, chat_model, embed_model } => {
                self.chat.set_model_info(ModelInfo {
                    provider,
                    chat_model,
                    embed_model,
                });
            }

            // ─── Context events (Phase 4) ────────────────────────
            AppEvent::ContextBuilding => {
                self.chat.set_building_context(true);
            }
            AppEvent::ContextBuilt(window) => {
                self.chat.set_building_context(false);
                self.chat.set_context_info(ContextInfo {
                    node_count: window.nodes.len(),
                    total_tokens: window.total_tokens,
                    savings_percent: window.savings_percent(),
                });

                self.graph.set_context_nodes(
                    window.anchor_ids.iter().copied().collect(),
                    window.in_context_ids.iter().copied().collect(),
                );

                self.context_window = Some(window);
            }
            AppEvent::ContextError(err) => {
                self.chat.set_building_context(false);
                self.chat
                    .push_message("system", format!("Context error: {}", err));
            }
            AppEvent::ModelListFetched(models) => {
                if models.is_empty() {
                    self.settings.receive_model_list(vec![]);
                    self.notification.show("Failed to fetch models");
                } else {
                    self.settings.receive_model_list(models);
                }
            }
            AppEvent::ZaiModelListFetched(models) => {
                if models.is_empty() {
                    self.settings.receive_zai_model_list(vec![]);
                    self.notification.show("Failed to fetch Z.ai models");
                } else {
                    self.settings.receive_zai_model_list(models);
                }
            }
            _ => {}
        }
    }

    fn start_indexing(&mut self) {
        let cancel = self.indexing.start_progress();
        let tx = self.event_tx.clone();
        let root = self.project_root.clone();
        tokio::spawn(async move {
            crate::run_index_with_progress(root, tx, cancel).await;
        });
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Indexing dialog intercepts all input when visible
        if self.indexing.visible {
            match self.indexing.handle_key(key) {
                IndexingAction::StartIndex | IndexingAction::Retry => {
                    self.start_indexing();
                }
                IndexingAction::Cancel => {
                    self.notification.show("Indexing cancelled");
                }
                IndexingAction::Close | IndexingAction::None => {}
            }
            return;
        }

        // Settings overlay intercepts all input when visible
        if self.settings.visible {
            match self.settings.handle_key(key) {
                SettingsAction::Saved => {
                    let _ = self.agent_cmd_tx.send(AgentCommand::ReloadConfig);
                    self.notification.show("Settings saved");
                }
                SettingsAction::FetchAnthropicModels(api_key) => {
                    let tx = self.event_tx.clone();
                    tokio::spawn(async move {
                        match obi_llm::anthropic::list_models(&api_key).await {
                            Ok(models) => { let _ = tx.send(AppEvent::ModelListFetched(models)); }
                            Err(_) => { let _ = tx.send(AppEvent::ModelListFetched(vec![])); }
                        }
                    });
                }
                SettingsAction::FetchZaiModels(api_key) => {
                    let tx = self.event_tx.clone();
                    tokio::spawn(async move {
                        match obi_llm::zai::list_models(&api_key).await {
                            Ok(models) => { let _ = tx.send(AppEvent::ZaiModelListFetched(models)); }
                            Err(_) => { let _ = tx.send(AppEvent::ZaiModelListFetched(vec![])); }
                        }
                    });
                }
                SettingsAction::None => {}
            }
            return;
        }

        // Command palette intercepts all input when visible
        if self.command_palette.visible {
            if let Some(path) = self.command_palette.handle_key(key) {
                self.editor.open_file(&path);
                self.focus = PanelFocus::Editor;
            }
            return;
        }

        // Handle tool confirmation dialog (y/n)
        if self.pending_tool_call_id.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(call_id) = self.pending_tool_call_id.take() {
                        self.chat.dismiss_tool_confirmation(true);
                        let _ = self
                            .agent_cmd_tx
                            .send(AgentCommand::ApproveToolCall(call_id));
                    }
                    return;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    if let Some(call_id) = self.pending_tool_call_id.take() {
                        self.chat.dismiss_tool_confirmation(false);
                        let _ = self
                            .agent_cmd_tx
                            .send(AgentCommand::DenyToolCall(call_id));
                    }
                    return;
                }
                _ => return, // Ignore other keys during confirmation
            }
        }

        // In insert mode, only Ctrl shortcuts and Esc are global; everything else goes to editor
        if self.focus == PanelFocus::Editor && self.editor.mode == EditorMode::Insert {
            if key.code == KeyCode::Esc {
                self.editor.handle_key(key);
                return;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('q') => {
                        self.running = false;
                        return;
                    }
                    KeyCode::Char('p') => {
                        let files = self.file_tree.all_file_paths();
                        self.command_palette.open(files);
                        return;
                    }
                    KeyCode::Char('s') => {
                        self.editor.save_active();
                        return;
                    }
                    KeyCode::Char('w') => {
                        self.editor.close_active();
                        return;
                    }
                    _ => {}
                }
            }
            self.editor.handle_key(key);
            return;
        }

        // In chat panel, delegate text input directly
        if self.focus == PanelFocus::Chat {
            match key.code {
                KeyCode::Esc => {
                    if self.chat.is_busy() {
                        // Interrupt: stop waiting for agent response
                        self.chat.set_building_context(false);
                        self.chat.finish_streaming_with_error();
                        self.chat.push_message("system", "[Interrupted]".to_string());
                    } else {
                        self.focus = PanelFocus::Editor;
                    }
                }
                _ if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.handle_global_ctrl(key);
                }
                KeyCode::Tab => {
                    self.focus = self.next_visible_focus();
                }
                _ => {
                    if let Some(query) = self.chat.handle_key(key) {
                        // Sync pinned/excluded from graph widget to agent
                        let _ = self.agent_cmd_tx.send(AgentCommand::UpdateContextSets {
                            pinned: self.graph.pinned_ids().clone(),
                            excluded: self.graph.excluded_ids().clone(),
                        });
                        // Send query to the agent
                        let _ = self.agent_cmd_tx.send(AgentCommand::Query(query));
                    }
                }
            }
            return;
        }

        // Global Ctrl shortcuts (Normal mode)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.handle_global_ctrl(key) {
                return;
            }
        }

        // Alt shortcuts for tab switching
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left if self.focus == PanelFocus::Editor => {
                    self.editor.prev_tab();
                    return;
                }
                KeyCode::Right if self.focus == PanelFocus::Editor => {
                    self.editor.next_tab();
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = self.next_visible_focus();
            }
            KeyCode::Esc => {
                self.focus = PanelFocus::Editor;
            }
            _ => match self.focus {
                PanelFocus::FileTree => {
                    self.file_tree.handle_key(key, &mut self.editor);
                }
                PanelFocus::Editor => {
                    self.editor.handle_key(key);
                }
                PanelFocus::Graph => {
                    self.graph.handle_key(key);
                    // Update backlink panel when graph selection changes
                    if let Some(ref kg) = self.knowledge_graph {
                        self.backlink_panel
                            .update(self.graph.selected_node(), kg);
                    }
                }
                PanelFocus::Chat => {
                    let _ = self.chat.handle_key(key);
                }
            },
        }
    }

    /// Handle Ctrl+key shortcuts. Returns true if handled.
    fn handle_global_ctrl(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => {
                self.running = false;
                true
            }
            KeyCode::Char('p') => {
                let files = self.file_tree.all_file_paths();
                self.command_palette.open(files);
                true
            }
            KeyCode::Char('b') => {
                if self.focus == PanelFocus::FileTree {
                    self.show_file_tree = false;
                    self.focus = PanelFocus::Editor;
                } else {
                    self.show_file_tree = true;
                    self.focus = PanelFocus::FileTree;
                }
                true
            }
            KeyCode::Char('g') => {
                if self.focus == PanelFocus::Graph {
                    self.show_graph = false;
                    self.focus = PanelFocus::Editor;
                } else {
                    self.show_graph = true;
                    self.focus = PanelFocus::Graph;
                }
                true
            }
            KeyCode::Char('j') => {
                self.focus = match self.focus {
                    PanelFocus::Chat => PanelFocus::Editor,
                    _ => PanelFocus::Chat,
                };
                true
            }
            KeyCode::Char('n') => {
                self.create_note_from_template();
                true
            }
            KeyCode::Char('s') => {
                self.editor.save_active();
                true
            }
            KeyCode::Char('t') => {
                let config = ObiConfig::load(&self.project_root);
                self.settings.open(&config);
                true
            }
            KeyCode::Char('w') => {
                self.editor.close_active();
                true
            }
            KeyCode::Left => {
                match self.focus {
                    PanelFocus::FileTree => {
                        self.file_tree_width = self.file_tree_width.saturating_sub(2).max(16);
                    }
                    PanelFocus::Graph => {
                        self.graph_width = self.graph_width.saturating_sub(2).max(20);
                    }
                    _ => return false,
                }
                true
            }
            KeyCode::Right => {
                match self.focus {
                    PanelFocus::FileTree => {
                        self.file_tree_width = (self.file_tree_width + 2).min(50);
                    }
                    PanelFocus::Graph => {
                        self.graph_width = (self.graph_width + 2).min(60);
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    }

    /// Handle mouse events: panel resize (drag borders), click-to-focus,
    /// click-to-interact (file tree, editor tabs, editor cursor, graph scroll).
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};

        let pb = self.panel_boundaries.clone();
        const BORDER_HIT: u16 = 1;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let x = mouse.column;
                let y = mouse.row;

                // --- Notification × button (highest priority) ---
                let term_area = ratatui::layout::Rect::new(0, 0, pb.total_width, pb.total_height);
                if self.notification.handle_click(x, y, term_area) {
                    return;
                }

                // --- Border drag detection (prioritized) ---
                if y >= pb.chat_top_y.saturating_sub(BORDER_HIT)
                    && y <= pb.chat_top_y + BORDER_HIT
                {
                    self.drag_state.active = Some(DragTarget::ChatTop);
                    return;
                }
                if self.show_file_tree
                    && x >= pb.file_tree_right_x.saturating_sub(BORDER_HIT)
                    && x <= pb.file_tree_right_x + BORDER_HIT
                    && y < pb.chat_top_y
                {
                    self.drag_state.active = Some(DragTarget::FileTreeRight);
                    return;
                }
                if self.show_graph
                    && x >= pb.graph_left_x.saturating_sub(BORDER_HIT)
                    && x <= pb.graph_left_x + BORDER_HIT
                    && y < pb.chat_top_y
                {
                    self.drag_state.active = Some(DragTarget::GraphLeft);
                    return;
                }

                // --- Click inside panels → focus + interact ---
                let pb = &self.panel_boundaries;
                match self.panel_at(x, y) {
                    Some(PanelFocus::FileTree) => {
                        self.focus = PanelFocus::FileTree;
                        let row = y.saturating_sub(pb.file_tree_inner.y);
                        if self.file_tree.handle_click(row, &mut self.editor) {
                            self.focus = PanelFocus::Editor;
                        }
                    }
                    Some(PanelFocus::Editor) => {
                        self.focus = PanelFocus::Editor;
                        let rel_y = y.saturating_sub(pb.editor_inner.y);
                        let rel_x = x.saturating_sub(pb.editor_inner.x);
                        if rel_y == 0 {
                            self.editor.handle_tab_click(rel_x, pb.editor_inner.width);
                        } else {
                            let content_row = rel_y.saturating_sub(1);
                            self.editor.handle_content_click(content_row, rel_x);
                        }
                    }
                    Some(PanelFocus::Graph) => {
                        self.focus = PanelFocus::Graph;
                        let rel_col = x.saturating_sub(pb.graph_inner.x);
                        let rel_row = y.saturating_sub(pb.graph_inner.y);
                        self.graph.handle_click(rel_col, rel_row);
                        if let Some(ref kg) = self.knowledge_graph {
                            self.backlink_panel.update(self.graph.selected_node(), kg);
                        }
                    }
                    Some(PanelFocus::Chat) => {
                        self.focus = PanelFocus::Chat;
                        let rel_row = y.saturating_sub(pb.chat_inner.y);
                        let rel_col = x.saturating_sub(pb.chat_inner.x);
                        self.chat.start_selection(rel_row, rel_col);
                        self.text_selecting = true;
                    }
                    None => {} // clicked on border — do nothing
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.text_selecting {
                    let rel_row = mouse.row.saturating_sub(pb.chat_inner.y);
                    let rel_col = mouse.column.saturating_sub(pb.chat_inner.x);
                    self.chat.update_selection(rel_row, rel_col);
                } else {
                    match self.drag_state.active {
                        Some(DragTarget::FileTreeRight) => {
                            self.file_tree_width = mouse.column.max(16).min(pb.total_width / 2);
                        }
                        Some(DragTarget::GraphLeft) => {
                            self.graph_width = pb.total_width.saturating_sub(mouse.column).max(20).min(pb.total_width / 2);
                        }
                        Some(DragTarget::ChatTop) => {
                            self.chat_height = pb.total_height.saturating_sub(mouse.row).max(4).min(pb.total_height / 2);
                        }
                        None => {}
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.text_selecting {
                    self.text_selecting = false;
                    if let Some(text) = self.chat.finish_selection() {
                        self.copy_to_clipboard(&text);
                    }
                }
                self.drag_state.active = None;
            }
            MouseEventKind::ScrollUp => {
                match self.panel_at(mouse.column, mouse.row) {
                    Some(PanelFocus::Editor) => self.editor.scroll_up(3),
                    Some(PanelFocus::Graph) => self.graph.handle_scroll_up(),
                    Some(PanelFocus::FileTree) => self.file_tree.scroll_up(),
                    Some(PanelFocus::Chat) => self.chat.scroll_up(3),
                    _ => {}
                }
            }
            MouseEventKind::ScrollDown => {
                match self.panel_at(mouse.column, mouse.row) {
                    Some(PanelFocus::Editor) => self.editor.scroll_down(3),
                    Some(PanelFocus::Graph) => self.graph.handle_scroll_down(),
                    Some(PanelFocus::FileTree) => self.file_tree.scroll_down(),
                    Some(PanelFocus::Chat) => self.chat.scroll_down(3),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Determine which panel the mouse is over based on actual rendered rects.
    /// Returns None if mouse is on a border or outside any panel.
    fn panel_at(&self, x: u16, y: u16) -> Option<PanelFocus> {
        let pb = &self.panel_boundaries;
        // Check from most specific to least — order matters for overlapping borders
        if Self::in_rect(x, y, pb.chat_inner) {
            return Some(PanelFocus::Chat);
        }
        if self.show_graph && Self::in_rect(x, y, pb.graph_inner) {
            return Some(PanelFocus::Graph);
        }
        if self.show_file_tree && Self::in_rect(x, y, pb.file_tree_inner) {
            return Some(PanelFocus::FileTree);
        }
        if Self::in_rect(x, y, pb.editor_inner) {
            return Some(PanelFocus::Editor);
        }
        None
    }

    /// Check if (x, y) is inside a Rect.
    fn in_rect(x: u16, y: u16, r: ratatui::layout::Rect) -> bool {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    }

    /// Copy text to system clipboard. Stores text to be written via OSC 52
    /// after the next terminal draw (safe for raw mode).
    fn copy_to_clipboard(&mut self, text: &str) {
        self.pending_clipboard = Some(text.to_string());
        self.notification.show("Copied to clipboard");
    }

    /// Create a new note from the default (ADR) template and open it in the editor.
    /// Saves to .obi/notes/<timestamp>-note.md
    fn create_note_from_template(&mut self) {
        use obi_agent::templates;

        let notes_dir = self.project_root.join(".obi").join("notes");
        if std::fs::create_dir_all(&notes_dir).is_err() {
            self.chat
                .push_message("system", "Failed to create .obi/notes/ directory".to_string());
            return;
        }

        // Use ADR template as default
        let template = templates::get_template("ADR")
            .unwrap_or_else(|| templates::all_templates().remove(0));

        // Generate filename with timestamp
        let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let filename = format!("{}-note.md", timestamp);
        let note_path = notes_dir.join(&filename);

        if std::fs::write(&note_path, template.content).is_err() {
            self.chat
                .push_message("system", "Failed to write note file".to_string());
            return;
        }

        // Open in editor
        self.editor.open_file(&note_path);
        self.focus = PanelFocus::Editor;
        self.chat.push_message(
            "system",
            format!("Created note: .obi/notes/{} (ADR template)", filename),
        );
    }

    /// Reset chat state if the agent task has crashed (channel closed).
    fn recover_from_agent_crash(&mut self) {
        self.chat.set_building_context(false);
        self.chat.finish_streaming_with_error();
        self.chat.push_message("system", "Agent crashed. Save settings (Ctrl+T → s) to restart.".to_string());

        // Re-spawn agent
        let (agent_cmd_tx, agent_event_rx) =
            spawn_agent(self.project_root.clone(), self.knowledge_graph.clone());
        self.agent_cmd_tx = agent_cmd_tx;

        // Bridge new agent events into our event channel
        let app_event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let mut agent_event_rx = agent_event_rx;
            while let Some(agent_event) = agent_event_rx.recv().await {
                let app_event = match agent_event {
                    AgentEvent::StreamChunk(text) => AppEvent::StreamChunk(text),
                    AgentEvent::ThinkingChunk(text) => AppEvent::ThinkingChunk(text),
                    AgentEvent::ResponseComplete(text) => AppEvent::ResponseComplete(text),
                    AgentEvent::ToolConfirmation { tool_name, description, args_display, call_id } =>
                        AppEvent::ToolConfirmation { tool_name, description, args_display, call_id },
                    AgentEvent::PlanGenerated(summary) => AppEvent::PlanGenerated(summary),
                    AgentEvent::ContextBuilding => AppEvent::ContextBuilding,
                    AgentEvent::ContextBuilt(window) => AppEvent::ContextBuilt(window),
                    AgentEvent::ModelInfo { provider, chat_model, embed_model } =>
                        AppEvent::ModelInfo { provider, chat_model, embed_model },
                    AgentEvent::Error(err) => AppEvent::AgentError(err),
                };
                if app_event_tx.send(app_event).is_err() {
                    break;
                }
            }
        });
    }

    fn next_visible_focus(&self) -> PanelFocus {
        let order: Vec<PanelFocus> = [
            (PanelFocus::FileTree, self.show_file_tree),
            (PanelFocus::Editor, true),
            (PanelFocus::Graph, self.show_graph),
            (PanelFocus::Chat, true),
        ]
        .iter()
        .filter(|(_, visible)| *visible)
        .map(|(p, _)| *p)
        .collect();

        let current_idx = order.iter().position(|p| *p == self.focus).unwrap_or(0);
        let next_idx = (current_idx + 1) % order.len();
        order[next_idx]
    }
}

