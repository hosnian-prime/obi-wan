use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

pub struct CommandPalette {
    pub visible: bool,
    input: String,
    items: Vec<PathBuf>,
    filtered: Vec<usize>,
    selected: usize,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            visible: false,
            input: String::new(),
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
        }
    }

    pub fn open(&mut self, file_paths: Vec<PathBuf>) {
        self.visible = true;
        self.input.clear();
        self.items = file_paths;
        self.selected = 0;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.input.clear();
        self.items.clear();
        self.filtered.clear();
    }

    /// Returns Some(path) if user confirmed a selection, None otherwise.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        match key.code {
            KeyCode::Esc => {
                self.close();
                None
            }
            KeyCode::Enter => {
                let result = self
                    .filtered
                    .get(self.selected)
                    .and_then(|&idx| self.items.get(idx))
                    .cloned();
                self.close();
                result
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.selected + 1 < self.filtered.len() {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                self.refilter();
                None
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.refilter();
                None
            }
            _ => None,
        }
    }

    fn refilter(&mut self) {
        let query = self.input.to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, path)| {
                if query.is_empty() {
                    return true;
                }
                let name = path.to_string_lossy().to_lowercase();
                fuzzy_match(&name, &query)
            })
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        // Center the palette: 60% width, top 40% of screen
        let popup_width = (area.width as f32 * 0.6) as u16;
        let popup_height = (area.height as f32 * 0.4).min(20.0) as u16;
        let x = (area.width - popup_width) / 2;
        let y = area.height / 6;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(" Quick Open (Ctrl+P) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        // Input line
        let input_line = Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::styled(self.input.as_str(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::DarkGray)),
        ]));
        frame.render_widget(input_line, chunks[0]);

        // Results
        let max_items = chunks[1].height as usize;
        let items: Vec<ListItem> = self
            .filtered
            .iter()
            .take(max_items)
            .enumerate()
            .map(|(vis_idx, &item_idx)| {
                let path = &self.items[item_idx];
                let display = path.to_string_lossy();
                let style = if vis_idx == self.selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(display.to_string(), style)))
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, chunks[1]);
    }
}

/// Simple fuzzy match: all characters of the query appear in order in the target.
fn fuzzy_match(target: &str, query: &str) -> bool {
    let mut target_chars = target.chars();
    for qc in query.chars() {
        loop {
            match target_chars.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}
