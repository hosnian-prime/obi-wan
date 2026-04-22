use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

// ─── Theme constants ────────────────────────────────────────────────
const CLR_PROVIDER: Color = Color::Rgb(180, 140, 255);
const CLR_MODEL: Color = Color::Rgb(140, 200, 255);
const CLR_EMBED: Color = Color::Rgb(100, 170, 130);
const CLR_DIM: Color = Color::Rgb(80, 80, 90);
const CLR_WARNING: Color = Color::Rgb(200, 120, 80);
const CLR_TOML_SECTION: Color = Color::Rgb(180, 140, 255);
const CLR_TOML_KEY: Color = Color::Rgb(140, 200, 255);
const CLR_TOML_VALUE: Color = Color::Rgb(100, 170, 130);

const CLR_ASSISTANT_LABEL: Color = Color::Rgb(180, 140, 255);
const CLR_ASSISTANT_MSG: Color = Color::Rgb(160, 210, 160);
const CLR_SYSTEM_MSG: Color = Color::Rgb(140, 140, 160);
const CLR_THINKING: Color = Color::Rgb(140, 140, 160);
const CLR_TIP: Color = Color::Rgb(100, 110, 140);

const SPINNER_FRAMES: &[char] = &['\u{2807}', '\u{280B}', '\u{2819}', '\u{2838}', '\u{2834}', '\u{2826}', '\u{2847}', '\u{280F}'];

// ─── Tips ──────────────────────────────────────────────────────────

/// Tips shown in the input area while waiting for a response.
const TIPS: &[&str] = &[
    "Press Esc to interrupt",
];

// ─── Public types ───────────────────────────────────────────────────

/// LLM provider/model info for display.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub provider: String,
    pub chat_model: String,
    pub embed_model: String,
}

/// Context status info for display.
#[derive(Debug, Clone)]
pub struct ContextInfo {
    pub node_count: usize,
    pub total_tokens: usize,
    pub savings_percent: f64,
}

// ─── Internal types ─────────────────────────────────────────────────

struct ChatMessage {
    role: Role,
    content: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    User,
    Assistant,
    System,
}

struct ToolConfirmationInfo {
    tool_name: String,
    description: String,
    args_display: String,
}

// ─── Widget ─────────────────────────────────────────────────────────

/// AI chat panel with agent integration — streaming, tool confirmation, context display.
/// Threshold: pastes with more lines than this are shown in compact form.
const PASTE_COMPACT_THRESHOLD: usize = 3;

pub struct ChatWidget {
    input: String,
    /// Full pasted text stored separately — shown compactly in input, sent fully on Enter.
    pasted_text: Option<String>,
    messages: Vec<ChatMessage>,
    building_context: bool,
    streaming: bool,
    streaming_buffer: String,
    /// Separate buffer for thinking/reasoning content (rendered with distinct styling).
    thinking_buffer: String,
    context_info: Option<ContextInfo>,
    tool_confirmation: Option<ToolConfirmationInfo>,
    model_info: Option<ModelInfo>,
    show_setup: bool,
    scroll_offset: usize,
    /// Frame counter for spinner animation.
    tick: u64,
    /// Plain text of last rendered lines (for text selection/copy).
    rendered_lines: Vec<String>,
    /// Active text selection: (start_row, start_col, end_row, end_col) relative to chat inner area.
    pub selection: Option<(u16, u16, u16, u16)>,
    /// When the current streaming/context-building started (for elapsed time display).
    streaming_started_at: Option<Instant>,
}

impl ChatWidget {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            pasted_text: None,
            messages: Vec::new(),
            building_context: false,
            streaming: false,
            streaming_buffer: String::new(),
            thinking_buffer: String::new(),
            context_info: None,
            tool_confirmation: None,
            model_info: None,
            streaming_started_at: None,
            show_setup: false,
            scroll_offset: 0,
            tick: 0,
            rendered_lines: Vec::new(),
            selection: None,
        }
    }

    // ─── Input handling ─────────────────────────────────────────

    /// Handle keyboard input. Returns Some(query) when user submits a message.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<String> {
        if self.show_setup {
            if key.code == KeyCode::Esc {
                self.show_setup = false;
            }
            return None;
        }

        // Tool confirmation blocks all input (handled separately by app)
        if self.tool_confirmation.is_some() {
            return None;
        }

        match key.code {
            KeyCode::Char(c) => {
                // Any typing clears the pasted text — user is composing fresh input
                if self.pasted_text.is_some() {
                    self.pasted_text = None;
                }
                self.input.push(c);
                None
            }
            KeyCode::Backspace => {
                if self.pasted_text.is_some() {
                    // Backspace on pasted text clears the whole paste
                    self.pasted_text = None;
                    self.input.clear();
                } else {
                    self.input.pop();
                }
                None
            }
            KeyCode::Enter => self.submit_input(),
            _ => None,
        }
    }

    /// Handle a bracketed paste event.
    pub fn handle_paste(&mut self, text: String) {
        if self.streaming || self.building_context {
            return;
        }
        let line_count = text.lines().count();
        if line_count > PASTE_COMPACT_THRESHOLD {
            // Store full text, show compact preview in input
            let first_line = text.lines().next().unwrap_or("").chars().take(30).collect::<String>();
            let preview = if first_line.len() < text.lines().next().unwrap_or("").len() {
                format!("{}...", first_line)
            } else {
                first_line
            };
            self.input = format!("[Pasted: {} lines] {}", line_count, preview);
            self.pasted_text = Some(text);
        } else {
            // Short paste — inline it directly (replace newlines with spaces)
            let inline = text.replace('\n', " ");
            self.input.push_str(&inline);
        }
    }

    fn submit_input(&mut self) -> Option<String> {
        let has_paste = self.pasted_text.is_some();
        let is_empty = self.input.is_empty() && !has_paste;
        if is_empty || self.streaming || self.building_context {
            return None;
        }

        if self.model_info.is_none() {
            self.show_setup = true;
            return None;
        }

        // If we have pasted text, send the full content; otherwise send input
        let text = if let Some(pasted) = self.pasted_text.take() {
            self.input.clear();
            pasted
        } else {
            std::mem::take(&mut self.input)
        };

        self.messages.push(ChatMessage {
            role: Role::User,
            content: text.clone(),
        });
        self.streaming = true;
        self.streaming_started_at = Some(Instant::now());
        Some(text)
    }

    // ─── Scroll ─────────────────────────────────────────────────

    pub fn scroll_up(&mut self, lines: usize) {
        // Max scroll is capped at a reasonable limit (total rendered lines)
        // Exact cap is enforced during render, so just allow generous scrolling here
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    // ─── State mutations (called from App) ──────────────────────

    pub fn set_model_info(&mut self, info: ModelInfo) {
        self.model_info = Some(info);
    }

    pub fn append_streaming_text(&mut self, text: &str) {
        self.streaming = true;
        self.streaming_buffer.push_str(text);
        self.scroll_offset = 0;
    }

    pub fn append_thinking_text(&mut self, text: &str) {
        self.streaming = true;
        self.thinking_buffer.push_str(text);
        self.scroll_offset = 0;
    }

    pub fn finish_streaming(&mut self, final_text: String) {
        self.streaming = false;
        self.streaming_started_at = None;
        self.streaming_buffer.clear();
        self.thinking_buffer.clear();
        if !final_text.is_empty() {
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: final_text,
            });
        }
        self.scroll_offset = 0; // auto-scroll to bottom
    }

    pub fn finish_streaming_with_error(&mut self) {
        self.streaming = false;
        self.streaming_started_at = None;
        self.thinking_buffer.clear();
        if !self.streaming_buffer.is_empty() {
            let partial = std::mem::take(&mut self.streaming_buffer);
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: partial,
            });
        }
    }

    pub fn show_tool_confirmation(
        &mut self,
        tool_name: &str,
        description: &str,
        args_display: &str,
    ) {
        self.streaming = false;
        self.streaming_started_at = None;
        self.thinking_buffer.clear();
        if !self.streaming_buffer.is_empty() {
            let partial = std::mem::take(&mut self.streaming_buffer);
            self.messages.push(ChatMessage {
                role: Role::Assistant,
                content: partial,
            });
        }
        self.tool_confirmation = Some(ToolConfirmationInfo {
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            args_display: args_display.to_string(),
        });
    }

    pub fn dismiss_tool_confirmation(&mut self, approved: bool) {
        if let Some(info) = self.tool_confirmation.take() {
            let msg = if approved {
                format!("[Approved: {}]", info.tool_name)
            } else {
                format!("[Denied: {}]", info.tool_name)
            };
            self.messages.push(ChatMessage {
                role: Role::System,
                content: msg,
            });
        }
    }

    pub fn push_message(&mut self, role_str: &str, content: String) {
        let role = match role_str {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => Role::System,
        };
        self.messages.push(ChatMessage { role, content });
        self.scroll_offset = 0; // auto-scroll to bottom on new message
    }

    /// Whether the chat needs periodic redraws (spinner animation active).
    pub fn is_animating(&self) -> bool {
        self.streaming || self.building_context
    }

    /// Whether the agent is busy (streaming or building context).
    pub fn is_busy(&self) -> bool {
        self.streaming || self.building_context
    }

    pub fn set_building_context(&mut self, building: bool) {
        self.building_context = building;
        if building && self.streaming_started_at.is_none() {
            self.streaming_started_at = Some(Instant::now());
        } else if !building && !self.streaming {
            self.streaming_started_at = None;
        }
    }

    pub fn set_context_info(&mut self, info: ContextInfo) {
        self.context_info = Some(info);
    }

    pub fn context_info(&self) -> Option<&ContextInfo> {
        self.context_info.as_ref()
    }

    // ─── Rendering ──────────────────────────────────────────────

    // ─── Text selection ───────────────────────────────────────

    pub fn start_selection(&mut self, row: u16, col: u16) {
        self.selection = Some((row, col, row, col));
    }

    pub fn update_selection(&mut self, row: u16, col: u16) {
        if let Some(ref mut sel) = self.selection {
            sel.2 = row;
            sel.3 = col;
        }
    }

    /// Finish selection and return the selected text for clipboard.
    pub fn finish_selection(&mut self) -> Option<String> {
        let sel = self.selection.take()?;
        let (mut r1, mut c1, mut r2, mut c2) = sel;

        // Normalize: ensure start <= end
        if r1 > r2 || (r1 == r2 && c1 > c2) {
            std::mem::swap(&mut r1, &mut r2);
            std::mem::swap(&mut c1, &mut c2);
        }

        let mut result = String::new();
        for row in r1..=r2 {
            let line = match self.rendered_lines.get(row as usize) {
                Some(l) => l.as_str(),
                None => continue,
            };

            let start_col = if row == r1 { c1 as usize } else { 0 };
            let end_col = if row == r2 { c2 as usize } else { line.chars().count() };

            // Convert char (column) indices to byte indices safely
            let char_count = line.chars().count();
            let start_col = start_col.min(char_count);
            let end_col = end_col.min(char_count);

            if start_col < end_col {
                let start_byte = line.char_indices().nth(start_col).map(|(i, _)| i).unwrap_or(line.len());
                let end_byte = line.char_indices().nth(end_col).map(|(i, _)| i).unwrap_or(line.len());

                if start_byte < end_byte {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(&line[start_byte..end_byte]);
                }
            }
        }

        if result.is_empty() { None } else { Some(result) }
    }

    // ─── Rendering ──────────────────────────────────────────────

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if area.height < 2 {
            return;
        }

        self.tick = self.tick.wrapping_add(1);

        let mut lines: Vec<Line> = Vec::new();
        let mut header_lines = 0u16;

        self.render_header(&mut lines, &mut header_lines);

        // Overlay screens take over the body
        if self.show_setup {
            self.render_setup_screen(&mut lines, area);
            let plain = lines_to_plain(&lines);
            frame.render_widget(Paragraph::new(lines), area);
            self.rendered_lines = plain;
            return;
        }

        if let Some(ref confirm) = self.tool_confirmation {
            Self::render_tool_confirmation(confirm, &mut lines, area);
            let plain = lines_to_plain(&lines);
            frame.render_widget(Paragraph::new(lines), area);
            self.rendered_lines = plain;
            return;
        }

        // Normal chat body: header + messages + separator(1) + input(1)
        let max_body_lines = area.height.saturating_sub(header_lines + 2) as usize;

        // Reserve space for thinking indicator if streaming
        let thinking_lines = self.estimate_thinking_lines(area.width);
        let msg_budget = max_body_lines.saturating_sub(thinking_lines);

        self.render_messages(&mut lines, msg_budget, area.width);
        self.render_thinking_indicator(&mut lines, area.width);

        // Truncate if exceeded budget
        if lines.len() > (header_lines as usize + max_body_lines) {
            lines.truncate(header_lines as usize + max_body_lines);
        }

        // Pad to push input line to bottom (leave 2 rows: separator + input)
        let target_before_input = area.height.saturating_sub(2) as usize;
        while lines.len() < target_before_input {
            lines.insert(header_lines as usize, Line::from(""));
        }

        // Separator line between messages and input
        lines.push(Line::from(""));
        self.render_input_line(&mut lines);

        let plain = lines_to_plain(&lines);

        frame.render_widget(Paragraph::new(lines), area);

        self.rendered_lines = plain;

        // Apply selection highlight to the buffer after widget render
        if let Some((r1, c1, r2, c2)) = self.selection {
            let highlight = Style::default().bg(Color::Rgb(60, 60, 90)).fg(Color::White);
            let (mut sr, mut sc, mut er, mut ec) = (r1, c1, r2, c2);
            if sr > er || (sr == er && sc > ec) {
                std::mem::swap(&mut sr, &mut er);
                std::mem::swap(&mut sc, &mut ec);
            }
            let buf = frame.buffer_mut();
            for row in sr..=er.min(area.height.saturating_sub(1)) {
                let y = area.y + row;
                if y >= area.y + area.height { break; }
                let ls = if row == sr { sc } else { 0 };
                let le = if row == er { ec } else { area.width.saturating_sub(1) };
                for col in ls..=le.min(area.width.saturating_sub(1)) {
                    let x = area.x + col;
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_style(highlight);
                    }
                }
            }
        }
    }



    fn render_header(&self, lines: &mut Vec<Line<'_>>, header_lines: &mut u16) {
        if let Some(ref mi) = self.model_info {
            let mut header_spans = vec![
                Span::styled(" ", Style::default()),
                Span::styled(mi.provider.clone(), Style::default().fg(CLR_PROVIDER).add_modifier(Modifier::BOLD)),
                Span::styled("/", Style::default().fg(CLR_DIM)),
                Span::styled(mi.chat_model.clone(), Style::default().fg(CLR_MODEL)),
                Span::styled("  embed:", Style::default().fg(CLR_DIM)),
                Span::styled(mi.embed_model.clone(), Style::default().fg(CLR_EMBED)),
            ];
            if mi.embed_model == "off" {
                header_spans.push(Span::styled(
                    " [graph-only, no semantic search — higher token cost]",
                    Style::default().fg(Color::Yellow),
                ));
            }
            lines.push(Line::from(header_spans));
        } else {
            lines.push(Line::from(vec![
                Span::styled(" \u{26a0} ", Style::default().fg(Color::Yellow)),
                Span::styled("No provider configured", Style::default().fg(CLR_WARNING).add_modifier(Modifier::BOLD)),
            ]));
        }
        *header_lines += 1;

        if self.building_context {
            let spinner = self.spinner_char();
            lines.push(Line::from(Span::styled(
                format!(" {spinner} Building context..."),
                Style::default().fg(CLR_THINKING).add_modifier(Modifier::ITALIC),
            )));
            *header_lines += 1;
        } else if let Some(ref info) = self.context_info {
            lines.push(Line::from(vec![
                Span::styled(" [Context: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}n", info.node_count), Style::default().fg(Color::Cyan)),
                Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}tok", info.total_tokens), Style::default().fg(Color::Cyan)),
                Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("\u{2193}{:.0}% saved", info.savings_percent), Style::default().fg(Color::Green)),
                Span::styled("]", Style::default().fg(Color::DarkGray)),
            ]));
            *header_lines += 1;
        }
    }

    /// Render all messages into styled lines, then apply line-based scroll.
    fn render_messages(&self, lines: &mut Vec<Line<'_>>, max_visible: usize, width: u16) {
        let w = width.saturating_sub(2) as usize;

        // Step 1: Build ALL message lines
        let mut all_lines: Vec<Line> = Vec::new();
        let mut prev_role: Option<Role> = None;

        for msg in &self.messages {
            let needs_separator = prev_role.is_some() && prev_role != Some(msg.role);
            if needs_separator {
                all_lines.push(Line::from(""));
            }

            match msg.role {
                Role::User => {
                    let user_style = Style::default().fg(Color::Black).bg(Color::White);
                    let wrapped = wrap_text(&msg.content, w.saturating_sub(4));
                    for (j, line) in wrapped.iter().enumerate() {
                        let prefix = if j == 0 { " \u{276f} " } else { "   " };
                        // Pad to full width so background fills the line
                        let text = format!("{prefix}{line}");
                        let padded = format!("{:<width$}", text, width = w);
                        all_lines.push(Line::from(Span::styled(padded, user_style)));
                    }
                }
                Role::Assistant => {
                    let is_first_in_block = prev_role != Some(Role::Assistant);
                    if is_first_in_block {
                        all_lines.push(Line::from(Span::styled(
                            " \u{25cf} obi-wan",
                            Style::default().fg(CLR_ASSISTANT_LABEL).add_modifier(Modifier::BOLD),
                        )));
                    }
                    for line in wrap_text(&msg.content, w.saturating_sub(3)) {
                        all_lines.push(Line::from(Span::styled(
                            format!("   {line}"),
                            Style::default().fg(CLR_ASSISTANT_MSG),
                        )));
                    }
                }
                Role::System => {
                    for line in wrap_text(&msg.content, w.saturating_sub(3)) {
                        all_lines.push(Line::from(Span::styled(
                            format!("   {line}"),
                            Style::default().fg(CLR_SYSTEM_MSG).add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
            prev_role = Some(msg.role);
        }

        // Step 2: Line-based scroll from bottom
        // scroll_offset=0 means show the latest lines (bottom), >0 means scroll up
        let total = all_lines.len();
        let end = total.saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(max_visible);

        lines.extend(all_lines.into_iter().skip(start).take(end - start));
    }

    /// Estimate how many lines the thinking indicator will consume.
    fn estimate_thinking_lines(&self, width: u16) -> usize {
        if !self.streaming {
            return 0;
        }
        let wrap_width = width.saturating_sub(5) as usize;
        let mut count = 0;

        // Thinking content lines
        if !self.thinking_buffer.is_empty() {
            count += 2; // separator + "thinking" header
            count += wrap_text(&self.thinking_buffer, wrap_width).len();
        }

        if self.streaming_buffer.is_empty() && self.thinking_buffer.is_empty() {
            2 // separator + "thinking..." line
        } else if self.streaming_buffer.is_empty() {
            count
        } else {
            // separator + header + wrapped content lines + thinking lines
            count + 2 + wrap_text(&self.streaming_buffer, wrap_width).len()
        }
    }

    fn render_thinking_indicator(&self, lines: &mut Vec<Line<'_>>, width: u16) {
        if !self.streaming {
            return;
        }

        // Separator from previous messages
        if !self.messages.is_empty() {
            lines.push(Line::from(""));
        }

        let spinner = self.spinner_char();
        let wrap_width = width.saturating_sub(5) as usize; // 3 indent + 2 border

        // Render thinking content (grey italic) if present
        if !self.thinking_buffer.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(CLR_THINKING),
                ),
                Span::styled(
                    "thinking",
                    Style::default().fg(CLR_THINKING).add_modifier(Modifier::BOLD | Modifier::ITALIC),
                ),
            ]));
            for line in wrap_text(&self.thinking_buffer, wrap_width) {
                lines.push(Line::from(Span::styled(
                    format!("   {line}"),
                    Style::default().fg(CLR_THINKING).add_modifier(Modifier::ITALIC),
                )));
            }
        }

        // Render response content below thinking
        if self.streaming_buffer.is_empty() && self.thinking_buffer.is_empty() {
            // No content yet — show generic "thinking..." spinner
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(CLR_ASSISTANT_LABEL),
                ),
                Span::styled(
                    "obi-wan is thinking...",
                    Style::default().fg(CLR_THINKING).add_modifier(Modifier::ITALIC),
                ),
            ]));
        } else if !self.streaming_buffer.is_empty() {
            // Response text is streaming — show it with normal styling
            if !self.thinking_buffer.is_empty() {
                lines.push(Line::from("")); // separator between thinking and response
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(CLR_ASSISTANT_LABEL),
                ),
                Span::styled(
                    "obi-wan",
                    Style::default().fg(CLR_ASSISTANT_LABEL).add_modifier(Modifier::BOLD),
                ),
            ]));
            for line in wrap_text(&self.streaming_buffer, wrap_width) {
                lines.push(Line::from(Span::styled(
                    format!("   {line}"),
                    Style::default().fg(CLR_ASSISTANT_MSG),
                )));
            }
        }
    }

    fn render_input_line(&self, lines: &mut Vec<Line<'_>>) {
        let is_busy = self.streaming || self.building_context;

        if is_busy {
            // Show a rotating tip while waiting for response
            let tip_idx = (self.tick as usize / 20) % TIPS.len();
            let tip = TIPS[tip_idx];

            let mut spans = vec![
                Span::styled(" \u{2139} ", Style::default().fg(CLR_TIP)),
                Span::styled(tip, Style::default().fg(CLR_TIP).add_modifier(Modifier::ITALIC)),
            ];

            // Show elapsed time since request started
            if let Some(started) = self.streaming_started_at {
                let elapsed = started.elapsed().as_secs();
                let elapsed_str = if elapsed >= 60 {
                    format!(" ({} min {} sec)", elapsed / 60, elapsed % 60)
                } else {
                    format!(" ({} sec)", elapsed)
                };
                spans.push(Span::styled(elapsed_str, Style::default().fg(CLR_DIM)));
            }

            lines.push(Line::from(spans));
        } else {
            let prefix = Span::styled("\u{276f} ", Style::default().fg(Color::Yellow));
            if self.pasted_text.is_some() {
                // Compact paste preview with distinct styling
                let text = Span::styled(
                    self.input.clone(),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC),
                );
                lines.push(Line::from(vec![prefix, text]));
            } else {
                let text = Span::styled(self.input.clone(), Style::default().fg(Color::White));
                let cursor = Span::styled("\u{2588}", Style::default().fg(Color::DarkGray));
                lines.push(Line::from(vec![prefix, text, cursor]));
            }
        }
    }

    fn render_setup_screen(&self, lines: &mut Vec<Line<'_>>, area: Rect) {
        let toml_lines: &[(&str, Option<&str>)] = &[
            ("[llm]", None),
            ("primary = ", Some("\"ollama\"")),
            ("", None),
            ("[llm.ollama]", None),
            ("model = ", Some("\"gemma4:e4b\"")),
            ("host = ", Some("\"http://localhost:11434\"")),
            ("", None),
            ("[embedding]", None),
            ("provider = ", Some("\"ollama\"")),
            ("model = ", Some("\"nomic-embed-text\"")),
        ];

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Provider Setup Required",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Edit ~/.obi/config.toml:",
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(""));

        for &(key, value) in toml_lines {
            if key.is_empty() {
                lines.push(Line::from(""));
            } else if let Some(val) = value {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {key}"), Style::default().fg(CLR_TOML_KEY)),
                    Span::styled(val, Style::default().fg(CLR_TOML_VALUE)),
                ]));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("  {key}"),
                    Style::default().fg(CLR_TOML_SECTION),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Ensure your LLM provider is running, then restart obi-wan.",
            Style::default().fg(Color::DarkGray),
        )));

        while lines.len() < area.height.saturating_sub(1) as usize {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            " Press Esc to dismiss",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        )));
    }

    fn render_tool_confirmation<'a>(
        confirm: &'a ToolConfirmationInfo,
        lines: &mut Vec<Line<'a>>,
        area: Rect,
    ) {
        lines.push(Line::from(Span::styled(
            format!(" \u{26a0} {} ", confirm.description),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("   {}", confirm.args_display),
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(vec![
            Span::styled("   [", Style::default().fg(Color::DarkGray)),
            Span::styled("y", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("] Allow  [", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled("] Deny", Style::default().fg(Color::DarkGray)),
        ]));

        while lines.len() < area.height.saturating_sub(1) as usize {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "> waiting for confirmation...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // ─── Helpers ────────────────────────────────────────────────

    fn spinner_char(&self) -> char {
        SPINNER_FRAMES[(self.tick as usize / 2) % SPINNER_FRAMES.len()]
    }
}

/// Extract plain text from rendered Line objects.
fn lines_to_plain(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

/// Snap a byte index to the nearest char boundary at or before `pos`.
fn floor_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Word-aware text wrapping (UTF-8 safe, newline-aware).
fn wrap_text(text: &str, max_width: usize) -> Vec<&str> {
    if max_width == 0 {
        return vec![text];
    }

    let mut result = Vec::new();

    // First split on actual newlines, then wrap each line
    for line in text.split('\n') {
        if line.is_empty() {
            result.push("");
            continue;
        }
        if line.len() <= max_width {
            result.push(line);
            continue;
        }

        // Wrap this single line
        let mut start = 0;
        while start < line.len() {
            let end = floor_char_boundary(line, (start + max_width).min(line.len()));
            if end == line.len() {
                result.push(&line[start..]);
                break;
            }

            let chunk = &line[start..end];
            if let Some(last_space) = chunk.rfind(' ') {
                result.push(&line[start..start + last_space]);
                start += last_space + 1;
            } else {
                result.push(&line[start..end]);
                start = end;
            }
        }
    }

    result
}
