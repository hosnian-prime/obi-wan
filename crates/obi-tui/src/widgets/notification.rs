use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

const AUTO_DISMISS: Duration = Duration::from_secs(3);
const BG: Color = Color::Rgb(30, 30, 48);
const BORDER_COLOR: Color = Color::Rgb(120, 140, 255);
const TEXT_COLOR: Color = Color::White;
const CLOSE_COLOR: Color = Color::Rgb(180, 180, 200);
/// Thick left-border character for a modern accent look.
const LEFT_BORDER: &str = "▎";

pub struct NotificationWidget {
    message: Option<(String, Instant)>,
}

impl NotificationWidget {
    pub fn new() -> Self {
        Self { message: None }
    }

    /// Show a notification message. Replaces any existing one.
    pub fn show(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    /// Dismiss the current notification (e.g. user clicked ×).
    pub fn dismiss(&mut self) {
        self.message = None;
    }

    /// Whether a notification is currently visible.
    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        self.message.is_some()
    }

    /// Handle a mouse click — returns true if the click hit the × button.
    pub fn handle_click(&mut self, x: u16, y: u16, area: Rect) -> bool {
        if let Some(popup) = self.popup_rect(area) {
            if y == popup.y && x == popup.x + popup.width.saturating_sub(2) {
                self.dismiss();
                return true;
            }
        }
        false
    }

    /// Render the notification toast overlay (top-right corner).
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let (msg, at) = match self.message {
            Some((ref msg, at)) => (msg.clone(), at),
            None => return,
        };

        if at.elapsed() >= AUTO_DISMISS {
            self.message = None;
            return;
        }

        let popup = match self.popup_rect(area) {
            Some(r) => r,
            None => return,
        };

        frame.render_widget(Clear, popup);

        //  ▎ ✓ Copied to clipboard          ×
        let line = Line::from(vec![
            Span::styled(LEFT_BORDER, Style::default().fg(BORDER_COLOR)),
            Span::styled(" ✓ ", Style::default().fg(Color::Rgb(130, 220, 130)).add_modifier(Modifier::BOLD)),
            Span::styled(&msg, Style::default().fg(TEXT_COLOR)),
            Span::styled(
                Self::pad_to_close(msg.len(), popup.width),
                Style::default().fg(BG).bg(BG),
            ),
            Span::styled(" × ", Style::default().fg(CLOSE_COLOR)),
        ]);

        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(BG)),
            popup,
        );
    }

    /// Compute the popup rect in the top-right corner.
    fn popup_rect(&self, area: Rect) -> Option<Rect> {
        let msg = match self.message {
            Some((ref m, _)) => m,
            None => return None,
        };
        // ▎ + " ✓ " (3) + msg + padding + " × " (3)
        let content_w = 1 + 3 + msg.len() as u16 + 3;
        let w = content_w.max(20).min(area.width.saturating_sub(4));
        let x = area.width.saturating_sub(w + 2);
        Some(Rect::new(x, 1, w, 1))
    }

    /// Generate padding spaces between the message and × button.
    fn pad_to_close(msg_len: usize, popup_width: u16) -> String {
        // used = border(1) + " ✓ "(3) + msg + " × "(3)
        let used = 1 + 3 + msg_len + 3;
        let pad = (popup_width as usize).saturating_sub(used);
        " ".repeat(pad)
    }
}
