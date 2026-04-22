use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

/// Possible states for the indexing dialog.
#[derive(Debug, Clone, PartialEq)]
enum IndexingState {
    /// Asking the user whether to index.
    Prompt,
    /// Indexing is in progress.
    InProgress,
    /// An error occurred during indexing.
    Error(String),
}

/// Action returned from key handling.
pub enum IndexingAction {
    StartIndex,
    Cancel,
    Retry,
    Close,
    None,
}

/// Startup dialog that checks for a valid index and offers to create one.
pub struct IndexingDialog {
    pub visible: bool,
    state: IndexingState,
    progress_done: usize,
    progress_total: usize,
    status_text: String,
    cancelled: Arc<AtomicBool>,
}

impl IndexingDialog {
    pub fn new() -> Self {
        Self {
            visible: false,
            state: IndexingState::Prompt,
            progress_done: 0,
            progress_total: 0,
            status_text: String::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Show the initial prompt asking whether to index.
    pub fn show_prompt(&mut self) {
        self.visible = true;
        self.state = IndexingState::Prompt;
        self.progress_done = 0;
        self.progress_total = 0;
        self.status_text.clear();
    }

    /// Transition to in-progress state and return a cancel flag.
    pub fn start_progress(&mut self) -> Arc<AtomicBool> {
        self.state = IndexingState::InProgress;
        self.progress_done = 0;
        self.progress_total = 0;
        self.status_text = "Starting...".into();
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled.clone()
    }

    /// Update progress counters.
    pub fn update_progress(&mut self, done: usize, total: usize) {
        self.progress_done = done;
        self.progress_total = total;
    }

    /// Update the status text shown below the progress bar.
    pub fn set_status(&mut self, text: String) {
        self.status_text = text;
    }

    /// Mark indexing as complete and hide.
    pub fn set_complete(&mut self) {
        self.visible = false;
    }

    /// Show an error with retry option.
    pub fn set_error(&mut self, err: String) {
        self.state = IndexingState::Error(err);
    }

    /// Handle a key event while the dialog is visible.
    pub fn handle_key(&mut self, key: KeyEvent) -> IndexingAction {
        match &self.state {
            IndexingState::Prompt => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    IndexingAction::StartIndex
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.visible = false;
                    IndexingAction::Close
                }
                _ => IndexingAction::None,
            },
            IndexingState::InProgress => match key.code {
                KeyCode::Esc => {
                    self.cancelled.store(true, Ordering::Relaxed);
                    self.visible = false;
                    IndexingAction::Cancel
                }
                _ => IndexingAction::None,
            },
            IndexingState::Error(_) => match key.code {
                KeyCode::Char('r') | KeyCode::Char('R') => IndexingAction::Retry,
                KeyCode::Esc => {
                    self.visible = false;
                    IndexingAction::Close
                }
                _ => IndexingAction::None,
            },
        }
    }

    /// Render the dialog as a centered popup overlay.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        let width = 54_u16.min(area.width.saturating_sub(4));
        let height = 10_u16.min(area.height.saturating_sub(2));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);

        let title = match &self.state {
            IndexingState::Prompt => " Index Required ",
            IndexingState::InProgress => " Indexing... ",
            IndexingState::Error(_) => " Indexing Error ",
        };

        let border_color = match &self.state {
            IndexingState::Prompt => Color::Yellow,
            IndexingState::InProgress => Color::Cyan,
            IndexingState::Error(_) => Color::Red,
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let lines = match &self.state {
            IndexingState::Prompt => self.render_prompt_lines(),
            IndexingState::InProgress => self.render_progress_lines(inner.width as usize),
            IndexingState::Error(err) => self.render_error_lines(err),
        };

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }

    fn render_prompt_lines(&self) -> Vec<Line<'static>> {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No index found for this project.",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  Indexing is required for code-aware chat."),
            Line::from("  This may take a moment depending on project size."),
            Line::from(""),
            Line::from(vec![
                Span::styled("  [Y]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" Index now   "),
                Span::styled("[N/Esc]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" Skip"),
            ]),
        ]
    }

    fn render_progress_lines(&self, bar_width: usize) -> Vec<Line<'static>> {
        let pct = if self.progress_total > 0 {
            (self.progress_done as f64 / self.progress_total as f64 * 100.0) as u16
        } else {
            0
        };

        // Build progress bar: ████░░░░
        let usable = bar_width.saturating_sub(12); // space for "  [] 100%"
        let filled = if self.progress_total > 0 {
            (usable * self.progress_done) / self.progress_total.max(1)
        } else {
            0
        };
        let empty = usable.saturating_sub(filled);

        let bar = format!(
            "  [{}{}] {:>3}%",
            "█".repeat(filled),
            "░".repeat(empty),
            pct,
        );

        vec![
            Line::from(""),
            Line::from(Span::styled(
                bar,
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(format!(
                "  {} / {} nodes",
                self.progress_done, self.progress_total
            )),
            Line::from(Span::styled(
                format!("  {}", self.status_text),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  [Esc] Cancel",
                Style::default().fg(Color::Red),
            )),
        ]
    }

    fn render_error_lines(&self, err: &str) -> Vec<Line<'static>> {
        let err = err.to_string();
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Indexing failed:",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("  {}", err)),
            Line::from(""),
            Line::from(""),
            Line::from(vec![
                Span::styled("  [R]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" Retry   "),
                Span::styled("[Esc]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::raw(" Close"),
            ]),
        ]
    }
}
