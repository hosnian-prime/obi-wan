use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Tabs};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Normal,
    Insert,
}

pub struct EditorWidget {
    buffers: Vec<Buffer>,
    active: usize,
    pub mode: EditorMode,
}

struct Buffer {
    path: PathBuf,
    name: String,
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    scroll_offset: usize,
    modified: bool,
    lang: LangHint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LangHint {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Toml,
    Markdown,
    Unknown,
}

impl LangHint {
    fn from_extension(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "py" => Self::Python,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" => Self::JavaScript,
            "go" => Self::Go,
            "toml" => Self::Toml,
            "md" => Self::Markdown,
            _ => Self::Unknown,
        }
    }

    fn keywords(&self) -> &[&str] {
        match self {
            Self::Rust => &[
                "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait",
                "where", "for", "in", "if", "else", "match", "return", "self", "Self", "super",
                "crate", "async", "await", "move", "ref", "type", "const", "static", "unsafe",
                "loop", "while", "break", "continue", "as", "true", "false", "Some", "None",
                "Ok", "Err", "Result", "Option", "Vec", "String", "Box", "dyn",
            ],
            Self::Python => &[
                "def", "class", "import", "from", "return", "if", "elif", "else", "for", "in",
                "while", "try", "except", "finally", "with", "as", "yield", "lambda", "pass",
                "break", "continue", "True", "False", "None", "and", "or", "not", "is", "self",
                "async", "await", "raise",
            ],
            Self::TypeScript | Self::JavaScript => &[
                "function", "const", "let", "var", "return", "if", "else", "for", "while", "do",
                "switch", "case", "break", "continue", "class", "extends", "import", "export",
                "from", "default", "new", "this", "super", "try", "catch", "finally", "throw",
                "async", "await", "yield", "true", "false", "null", "undefined", "typeof",
                "interface", "type", "enum", "implements",
            ],
            Self::Go => &[
                "func", "package", "import", "return", "if", "else", "for", "range", "switch",
                "case", "default", "break", "continue", "go", "defer", "select", "chan", "map",
                "struct", "interface", "type", "const", "var", "true", "false", "nil", "make",
                "len", "cap", "append", "new",
            ],
            _ => &[],
        }
    }

    fn comment_prefix(&self) -> &str {
        match self {
            Self::Rust | Self::Go | Self::TypeScript | Self::JavaScript => "//",
            Self::Python | Self::Toml => "#",
            Self::Markdown | Self::Unknown => "",
        }
    }
}

impl EditorWidget {
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
            active: 0,
            mode: EditorMode::Normal,
        }
    }

    pub fn title(&self) -> String {
        if self.buffers.is_empty() {
            " Editor ".to_string()
        } else {
            let buf = &self.buffers[self.active];
            let modified = if buf.modified { " [+]" } else { "" };
            let mode = match self.mode {
                EditorMode::Normal => "NOR",
                EditorMode::Insert => "INS",
            };
            format!(" {} {} {}", buf.name, modified, mode)
        }
    }

    #[allow(dead_code)]
    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn open_file(&mut self, path: &Path) {
        if let Some(idx) = self.buffers.iter().position(|b| b.path == path) {
            self.active = idx;
            return;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return,
        };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut lines: Vec<String> = content.lines().map(String::from).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }

        self.buffers.push(Buffer {
            path: path.to_path_buf(),
            name,
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            modified: false,
            lang: LangHint::from_extension(&ext),
        });
        self.active = self.buffers.len() - 1;
    }

    pub fn close_active(&mut self) {
        if self.buffers.is_empty() {
            return;
        }
        self.buffers.remove(self.active);
        if self.active > 0 && self.active >= self.buffers.len() {
            self.active = self.buffers.len() - 1;
        }
    }

    pub fn next_tab(&mut self) {
        if self.buffers.len() > 1 {
            self.active = (self.active + 1) % self.buffers.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if self.buffers.len() > 1 {
            self.active = if self.active == 0 {
                self.buffers.len() - 1
            } else {
                self.active - 1
            };
        }
    }

    #[allow(dead_code)]
    pub fn goto_tab(&mut self, idx: usize) {
        if idx < self.buffers.len() {
            self.active = idx;
        }
    }

    pub fn save_active(&mut self) {
        if let Some(buf) = self.buffers.get_mut(self.active) {
            let content = buf.lines.join("\n");
            if std::fs::write(&buf.path, &content).is_ok() {
                buf.modified = false;
            }
        }
    }

    /// Handle mouse click on the tab bar (row 0 relative to editor area).
    /// Rough hit-test: divide tab bar width evenly by tab count.
    pub fn handle_tab_click(&mut self, col: u16, area_width: u16) {
        if self.buffers.is_empty() {
            return;
        }
        // Approximate: each tab takes equal width
        let tab_width = area_width / self.buffers.len().max(1) as u16;
        let idx = (col / tab_width.max(1)) as usize;
        if idx < self.buffers.len() {
            self.active = idx;
        }
    }

    /// Handle mouse click in the editor content area.
    /// `row` and `col` are relative to the content area (below tab bar).
    /// Scroll editor content up by n lines.
    pub fn scroll_up(&mut self, lines: usize) {
        if let Some(buf) = self.buffers.get_mut(self.active) {
            buf.scroll_offset = buf.scroll_offset.saturating_sub(lines);
        }
    }

    /// Scroll editor content down by n lines.
    pub fn scroll_down(&mut self, lines: usize) {
        if let Some(buf) = self.buffers.get_mut(self.active) {
            let max_scroll = buf.lines.len().saturating_sub(1);
            buf.scroll_offset = (buf.scroll_offset + lines).min(max_scroll);
        }
    }

    pub fn handle_content_click(&mut self, row: u16, col: u16) {
        if let Some(buf) = self.buffers.get_mut(self.active) {
            let target_line = buf.scroll_offset + row as usize;
            if target_line < buf.lines.len() {
                buf.cursor_row = target_line;
                buf.cursor_col = (col as usize).min(buf.lines[target_line].len());
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.buffers.is_empty() {
            return;
        }

        // Ctrl shortcuts work in both modes
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('w') => {
                    self.close_active();
                    return;
                }
                KeyCode::Char('s') => {
                    self.save_active();
                    return;
                }
                _ => {}
            }
        }

        // Alt shortcuts for tab switching
        if key.modifiers.contains(KeyModifiers::ALT) {
            match key.code {
                KeyCode::Left => {
                    self.prev_tab();
                    return;
                }
                KeyCode::Right => {
                    self.next_tab();
                    return;
                }
                _ => {}
            }
        }

        match self.mode {
            EditorMode::Normal => self.handle_normal_key(key),
            EditorMode::Insert => self.handle_insert_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) {
        let buf = &mut self.buffers[self.active];
        let line_count = buf.lines.len();

        match key.code {
            // Enter insert mode
            KeyCode::Char('i') => {
                self.mode = EditorMode::Insert;
            }
            KeyCode::Char('a') => {
                self.mode = EditorMode::Insert;
                let max_col = buf.lines.get(buf.cursor_row).map(|l| l.len()).unwrap_or(0);
                buf.cursor_col = (buf.cursor_col + 1).min(max_col);
            }
            KeyCode::Char('A') => {
                self.mode = EditorMode::Insert;
                buf.cursor_col = buf.lines.get(buf.cursor_row).map(|l| l.len()).unwrap_or(0);
            }
            KeyCode::Char('o') => {
                self.mode = EditorMode::Insert;
                let new_row = buf.cursor_row + 1;
                buf.lines.insert(new_row, String::new());
                buf.cursor_row = new_row;
                buf.cursor_col = 0;
                buf.modified = true;
            }
            KeyCode::Char('O') => {
                self.mode = EditorMode::Insert;
                buf.lines.insert(buf.cursor_row, String::new());
                buf.cursor_col = 0;
                buf.modified = true;
            }
            // Navigation
            KeyCode::Char('h') | KeyCode::Left => {
                buf.cursor_col = buf.cursor_col.saturating_sub(1);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if buf.cursor_row + 1 < line_count {
                    buf.cursor_row += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                buf.cursor_row = buf.cursor_row.saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let max_col = buf
                    .lines
                    .get(buf.cursor_row)
                    .map(|l| l.len().saturating_sub(1))
                    .unwrap_or(0);
                if buf.cursor_col < max_col {
                    buf.cursor_col += 1;
                }
            }
            KeyCode::Char('0') => {
                buf.cursor_col = 0;
            }
            KeyCode::Char('$') => {
                buf.cursor_col = buf
                    .lines
                    .get(buf.cursor_row)
                    .map(|l| l.len().saturating_sub(1).max(0))
                    .unwrap_or(0);
            }
            KeyCode::Char('w') => {
                // Jump to next word
                if let Some(line) = buf.lines.get(buf.cursor_row) {
                    let chars: Vec<char> = line.chars().collect();
                    let mut col = buf.cursor_col + 1;
                    // Skip current word
                    while col < chars.len() && !chars[col].is_whitespace() {
                        col += 1;
                    }
                    // Skip whitespace
                    while col < chars.len() && chars[col].is_whitespace() {
                        col += 1;
                    }
                    buf.cursor_col = col.min(chars.len().saturating_sub(1));
                }
            }
            KeyCode::Char('b') => {
                // Jump to previous word
                if let Some(line) = buf.lines.get(buf.cursor_row) {
                    let chars: Vec<char> = line.chars().collect();
                    let mut col = buf.cursor_col.saturating_sub(1);
                    // Skip whitespace backwards
                    while col > 0 && chars.get(col).map_or(false, |c| c.is_whitespace()) {
                        col -= 1;
                    }
                    // Skip word backwards
                    while col > 0 && chars.get(col - 1).map_or(false, |c| !c.is_whitespace()) {
                        col -= 1;
                    }
                    buf.cursor_col = col;
                }
            }
            KeyCode::Char('G') => {
                buf.cursor_row = line_count.saturating_sub(1);
            }
            KeyCode::Char('g') => {
                buf.cursor_row = 0;
                buf.scroll_offset = 0;
            }
            // Delete character under cursor
            KeyCode::Char('x') => {
                if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                    if buf.cursor_col < line.len() {
                        line.remove(buf.cursor_col);
                        buf.modified = true;
                    }
                }
            }
            // Delete line
            KeyCode::Char('d') => {
                if line_count > 1 {
                    buf.lines.remove(buf.cursor_row);
                    if buf.cursor_row >= buf.lines.len() {
                        buf.cursor_row = buf.lines.len() - 1;
                    }
                    buf.modified = true;
                } else {
                    buf.lines[0].clear();
                    buf.cursor_col = 0;
                    buf.modified = true;
                }
            }
            // Page up/down
            KeyCode::PageUp => {
                buf.cursor_row = buf.cursor_row.saturating_sub(20);
            }
            KeyCode::PageDown => {
                buf.cursor_row = (buf.cursor_row + 20).min(line_count.saturating_sub(1));
            }
            // Tab number switching: 1-9
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.buffers.len() {
                    self.active = idx;
                }
                return; // early return — don't clamp cursor on different buffer
            }
            _ => {}
        }

        // Clamp cursor_col
        if let Some(buf) = self.buffers.get(self.active) {
            let max_col = buf
                .lines
                .get(buf.cursor_row)
                .map(|l| if l.is_empty() { 0 } else { l.len().saturating_sub(1) })
                .unwrap_or(0);
            if self.buffers[self.active].cursor_col > max_col {
                self.buffers[self.active].cursor_col = max_col;
            }
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) {
        let buf = &mut self.buffers[self.active];

        match key.code {
            KeyCode::Esc => {
                self.mode = EditorMode::Normal;
                // Move cursor back one in normal mode (vim behavior)
                buf.cursor_col = buf.cursor_col.saturating_sub(1);
            }
            KeyCode::Char(c) => {
                if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                    let col = buf.cursor_col.min(line.len());
                    line.insert(col, c);
                    buf.cursor_col = col + 1;
                    buf.modified = true;
                }
            }
            KeyCode::Enter => {
                if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                    let col = buf.cursor_col.min(line.len());
                    let rest = line[col..].to_string();
                    line.truncate(col);
                    // Auto-indent: copy leading whitespace from current line
                    let indent: String = buf.lines[buf.cursor_row]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect();
                    let new_line = format!("{}{}", indent, rest);
                    buf.cursor_row += 1;
                    buf.cursor_col = indent.len();
                    buf.lines.insert(buf.cursor_row, new_line);
                    buf.modified = true;
                }
            }
            KeyCode::Backspace => {
                let col = buf.cursor_col;
                if col > 0 {
                    if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                        line.remove(col - 1);
                        buf.cursor_col -= 1;
                        buf.modified = true;
                    }
                } else if buf.cursor_row > 0 {
                    // Join with previous line
                    let current_line = buf.lines.remove(buf.cursor_row);
                    buf.cursor_row -= 1;
                    buf.cursor_col = buf.lines[buf.cursor_row].len();
                    buf.lines[buf.cursor_row].push_str(&current_line);
                    buf.modified = true;
                }
            }
            KeyCode::Delete => {
                if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                    if buf.cursor_col < line.len() {
                        line.remove(buf.cursor_col);
                        buf.modified = true;
                    } else if buf.cursor_row + 1 < buf.lines.len() {
                        // Join with next line
                        let next_line = buf.lines.remove(buf.cursor_row + 1);
                        buf.lines[buf.cursor_row].push_str(&next_line);
                        buf.modified = true;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(line) = buf.lines.get_mut(buf.cursor_row) {
                    let col = buf.cursor_col.min(line.len());
                    line.insert_str(col, "    ");
                    buf.cursor_col = col + 4;
                    buf.modified = true;
                }
            }
            // Arrow keys work in insert mode too
            KeyCode::Up => buf.cursor_row = buf.cursor_row.saturating_sub(1),
            KeyCode::Down => {
                if buf.cursor_row + 1 < buf.lines.len() {
                    buf.cursor_row += 1;
                }
            }
            KeyCode::Left => buf.cursor_col = buf.cursor_col.saturating_sub(1),
            KeyCode::Right => {
                let max = buf.lines.get(buf.cursor_row).map(|l| l.len()).unwrap_or(0);
                if buf.cursor_col < max {
                    buf.cursor_col += 1;
                }
            }
            KeyCode::Home => buf.cursor_col = 0,
            KeyCode::End => {
                buf.cursor_col = buf.lines.get(buf.cursor_row).map(|l| l.len()).unwrap_or(0);
            }
            _ => {}
        }

        // Clamp cursor_col for insert mode (can be at line.len(), not len-1)
        if let Some(line) = buf.lines.get(buf.cursor_row) {
            if buf.cursor_col > line.len() {
                buf.cursor_col = line.len();
            }
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if self.buffers.is_empty() {
            render_help_screen(frame, area);
            return;
        }

        // Split: tab bar (1) | content | mode bar (1)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.render_tabs(frame, chunks[0]);
        self.render_content(frame, chunks[1]);
        self.render_mode_bar(frame, chunks[2]);
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, buf)| {
                let modified = if buf.modified { "*" } else { "" };
                let label = format!("{}{}{}", i + 1, modified, buf.name);
                let style = if i == self.active {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::styled(label, style)
            })
            .collect();

        let tabs = Tabs::new(titles)
            .divider(Span::styled(" | ", Style::default().fg(Color::DarkGray)))
            .highlight_style(Style::default().fg(Color::Cyan));

        frame.render_widget(tabs, area);
    }

    fn render_mode_bar(&self, frame: &mut Frame, area: Rect) {
        let buf = &self.buffers[self.active];
        let (mode_label, mode_color) = match self.mode {
            EditorMode::Normal => ("NORMAL", Color::Blue),
            EditorMode::Insert => ("INSERT", Color::Green),
        };
        let modified = if buf.modified { " [+]" } else { "" };

        let bar = ratatui::widgets::Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} ", mode_label),
                Style::default().fg(Color::Black).bg(mode_color),
            ),
            Span::styled(
                format!(
                    " {}:{}{} | {} ",
                    buf.cursor_row + 1,
                    buf.cursor_col + 1,
                    modified,
                    buf.name
                ),
                Style::default().fg(Color::White).bg(Color::Rgb(30, 30, 30)),
            ),
        ]));
        frame.render_widget(bar, area);
    }

    fn render_content(&self, frame: &mut Frame, area: Rect) {
        let buf = &self.buffers[self.active];
        let visible_height = area.height as usize;

        // Auto-scroll
        let scroll = if buf.cursor_row < buf.scroll_offset {
            buf.cursor_row
        } else if buf.cursor_row >= buf.scroll_offset + visible_height {
            buf.cursor_row - visible_height + 1
        } else {
            buf.scroll_offset
        };

        let start = scroll;
        let end = (start + visible_height).min(buf.lines.len());
        let gutter_width = format!("{}", buf.lines.len()).len().max(3);

        let items: Vec<ListItem> = buf.lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let line_num = start + i + 1;
                let is_cursor_line = start + i == buf.cursor_row;

                let num_style = if is_cursor_line {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let num_span = Span::styled(
                    format!("{:>width$} ", line_num, width = gutter_width),
                    num_style,
                );

                let mut spans = vec![num_span];

                if is_cursor_line {
                    // Render with cursor position
                    let col = buf.cursor_col.min(line.len());
                    let before = &line[..col];
                    let cursor_char = line.get(col..col + 1).unwrap_or(" ");
                    let after = if col < line.len() {
                        &line[col + 1..]
                    } else {
                        ""
                    };

                    spans.extend(highlight_line(before, buf.lang, false));
                    spans.push(Span::styled(
                        cursor_char.to_string(),
                        Style::default().fg(Color::Black).bg(Color::White),
                    ));
                    spans.extend(highlight_line(after, buf.lang, false));
                } else {
                    spans.extend(highlight_line(line, buf.lang, false));
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, area);
    }
}

/// Keyword-based syntax highlighting. Will be replaced by tree-sitter in Phase 2.
fn highlight_line(line: &str, lang: LangHint, _is_cursor: bool) -> Vec<Span<'static>> {
    if line.is_empty() {
        return vec![Span::raw("")];
    }

    let comment_prefix = lang.comment_prefix();

    // Check for full-line comment
    let trimmed = line.trim_start();
    if !comment_prefix.is_empty() && trimmed.starts_with(comment_prefix) {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let keywords = lang.keywords();
    if keywords.is_empty() {
        return vec![Span::styled(
            line.to_string(),
            Style::default().fg(Color::White),
        )];
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut current = String::new();

    while i < chars.len() {
        let ch = chars[i];

        // String literals
        if ch == '"' || ch == '\'' {
            if !current.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current),
                    Style::default().fg(Color::White),
                ));
            }
            let quote = ch;
            let mut s = String::new();
            s.push(ch);
            i += 1;
            while i < chars.len() {
                s.push(chars[i]);
                if chars[i] == quote && (i == 0 || chars[i - 1] != '\\') {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(s, Style::default().fg(Color::Yellow)));
            continue;
        }

        // Inline comment
        if !comment_prefix.is_empty() && i + comment_prefix.len() <= chars.len() {
            let slice: String = chars[i..i + comment_prefix.len()].iter().collect();
            if slice == comment_prefix {
                if !current.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut current),
                        Style::default().fg(Color::White),
                    ));
                }
                let rest: String = chars[i..].iter().collect();
                spans.push(Span::styled(rest, Style::default().fg(Color::DarkGray)));
                return spans;
            }
        }

        // Numbers
        if ch.is_ascii_digit() && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            if !current.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current),
                    Style::default().fg(Color::White),
                ));
            }
            let mut num = String::new();
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '_') {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(num, Style::default().fg(Color::Magenta)));
            continue;
        }

        // Word boundaries — check for keywords
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
            i += 1;
            continue;
        }

        // Non-word character — flush current word
        if !current.is_empty() {
            let word = std::mem::take(&mut current);
            if keywords.contains(&word.as_str()) {
                spans.push(Span::styled(
                    word,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(word, Style::default().fg(Color::White)));
            }
        }

        // Punctuation / operators
        let punc_color = match ch {
            '(' | ')' | '[' | ']' | '{' | '}' => Color::LightYellow,
            ':' | ';' | ',' | '.' => Color::DarkGray,
            '=' | '+' | '-' | '*' | '/' | '<' | '>' | '!' | '&' | '|' => Color::Red,
            _ => Color::White,
        };
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(punc_color),
        ));
        i += 1;
    }

    // Flush remaining word
    if !current.is_empty() {
        if keywords.contains(&current.as_str()) {
            spans.push(Span::styled(
                current,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(current, Style::default().fg(Color::White)));
        }
    }

    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    spans
}

/// Render the full help screen when no files are open.
fn render_help_screen(frame: &mut Frame, area: Rect) {
    use ratatui::widgets::Paragraph;

    let title_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let heading_style = Style::default().fg(Color::Rgb(255, 180, 50)).add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(Color::Rgb(120, 220, 120));
    let desc_style = Style::default().fg(Color::Rgb(180, 180, 190));
    let dim_style = Style::default().fg(Color::Rgb(100, 100, 110));

    /// Helper to create a shortcut line: "  key          description"
    fn shortcut<'a>(key: &'a str, desc: &'a str, ks: Style, ds: Style) -> Line<'a> {
        Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("{:<14}", key), ks),
            Span::styled(desc, ds),
        ])
    }

    let mut lines: Vec<Line> = Vec::new();

    // ── Title ──
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled("⚔  Obi-Wan", title_style),
        Span::styled("  —  AI-Native TUI IDE", dim_style),
    ]));
    lines.push(Line::from(""));

    // ── Global ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("Global", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("Ctrl+Q", "Quit", key_style, desc_style));
    lines.push(shortcut("Ctrl+P", "Quick open (command palette)", key_style, desc_style));
    lines.push(shortcut("Ctrl+B", "Toggle file explorer", key_style, desc_style));
    lines.push(shortcut("Ctrl+G", "Toggle knowledge graph", key_style, desc_style));
    lines.push(shortcut("Ctrl+J", "Toggle AI chat", key_style, desc_style));
    lines.push(shortcut("Ctrl+N", "Create new note (ADR template)", key_style, desc_style));
    lines.push(shortcut("Ctrl+S", "Save current file", key_style, desc_style));
    lines.push(shortcut("Ctrl+W", "Close current tab", key_style, desc_style));
    lines.push(shortcut("Ctrl+←/→", "Resize focused panel", key_style, desc_style));
    lines.push(shortcut("Tab", "Cycle focus between panels", key_style, desc_style));
    lines.push(shortcut("Esc", "Return focus to editor", key_style, desc_style));
    lines.push(Line::from(""));

    // ── Editor ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("Editor — Normal Mode", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("i", "Insert mode (before cursor)", key_style, desc_style));
    lines.push(shortcut("a / A", "Insert after cursor / end of line", key_style, desc_style));
    lines.push(shortcut("o / O", "New line below / above", key_style, desc_style));
    lines.push(shortcut("h j k l", "Move left / down / up / right", key_style, desc_style));
    lines.push(shortcut("w / b", "Next word / previous word", key_style, desc_style));
    lines.push(shortcut("0 / $", "Start / end of line", key_style, desc_style));
    lines.push(shortcut("g / G", "First line / last line", key_style, desc_style));
    lines.push(shortcut("x", "Delete character under cursor", key_style, desc_style));
    lines.push(shortcut("d", "Delete line", key_style, desc_style));
    lines.push(shortcut("1-9", "Switch to tab N", key_style, desc_style));
    lines.push(shortcut("Alt+←/→", "Previous / next tab", key_style, desc_style));
    lines.push(shortcut("PageUp/Down", "Scroll by 20 lines", key_style, desc_style));
    lines.push(Line::from(""));

    // ── Editor Insert ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("Editor — Insert Mode", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("Esc", "Back to normal mode", key_style, desc_style));
    lines.push(shortcut("Enter", "New line (auto-indent)", key_style, desc_style));
    lines.push(shortcut("Tab", "Insert 4 spaces", key_style, desc_style));
    lines.push(shortcut("Backspace", "Delete backward", key_style, desc_style));
    lines.push(shortcut("Arrow keys", "Navigate", key_style, desc_style));
    lines.push(shortcut("Home / End", "Start / end of line", key_style, desc_style));
    lines.push(Line::from(""));

    // ── File Tree ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("File Explorer", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("j / k", "Move down / up", key_style, desc_style));
    lines.push(shortcut("Enter / →", "Open file / expand directory", key_style, desc_style));
    lines.push(shortcut("←", "Collapse directory", key_style, desc_style));
    lines.push(Line::from(""));

    // ── Graph ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("Knowledge Graph", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("h / l", "Pan left / right", key_style, desc_style));
    lines.push(shortcut("j / k", "Select next / previous node", key_style, desc_style));
    lines.push(shortcut("+ / -", "Zoom in / out", key_style, desc_style));
    lines.push(shortcut("Enter", "Focus on selected node", key_style, desc_style));
    lines.push(shortcut("Tab", "Cycle through nodes", key_style, desc_style));
    lines.push(shortcut("Esc", "Fit all nodes / deselect", key_style, desc_style));
    lines.push(shortcut("/", "Search nodes", key_style, desc_style));
    lines.push(shortcut("f", "Cycle filter (All/Code/Notes/Context)", key_style, desc_style));
    lines.push(shortcut("g", "Cycle layout (Force/Tree/Radial)", key_style, desc_style));
    lines.push(shortcut("p", "Pin node (force include)", key_style, desc_style));
    lines.push(shortcut("P (Shift)", "Exclude node (force exclude)", key_style, desc_style));
    lines.push(Line::from(""));

    // ── Chat ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("AI Chat", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("Type + Enter", "Send query to AI agent", key_style, desc_style));
    lines.push(shortcut("y / n", "Approve / deny tool call", key_style, desc_style));
    lines.push(shortcut("Esc", "Return to editor", key_style, desc_style));
    lines.push(Line::from(""));

    // ── Mouse ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("Mouse", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("Click", "Focus panel + interact", key_style, desc_style));
    lines.push(shortcut("Drag border", "Resize panels", key_style, desc_style));
    lines.push(shortcut("Scroll", "Scroll content in hovered panel", key_style, desc_style));
    lines.push(shortcut("Click file", "Open file / toggle directory", key_style, desc_style));
    lines.push(shortcut("Click tab", "Switch editor tab", key_style, desc_style));
    lines.push(shortcut("Click graph", "Select nearest node", key_style, desc_style));
    lines.push(Line::from(""));

    // ── CLI ──
    lines.push(Line::from(vec![Span::raw("   "), Span::styled("CLI Commands", heading_style)]));
    lines.push(Line::from(vec![Span::styled("   ─────────────────────────────────────────", dim_style)]));
    lines.push(shortcut("obi", "Launch TUI IDE", key_style, desc_style));
    lines.push(shortcut("obi index", "Build knowledge graph index", key_style, desc_style));
    lines.push(shortcut("obi status", "Show index stats", key_style, desc_style));
    lines.push(shortcut("obi brain", "Show dual-brain status", key_style, desc_style));
    lines.push(Line::from(""));

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
