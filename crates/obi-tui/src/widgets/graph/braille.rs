use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// Braille-based high-resolution canvas for terminal rendering.
///
/// Each terminal cell maps to a 2×4 dot matrix (Unicode U+2800–U+28FF),
/// giving us 2× horizontal and 4× vertical resolution over character cells.
/// A 200×50 terminal becomes a 400×200 "pixel" canvas.
pub struct BrailleCanvas {
    /// Terminal columns.
    width: usize,
    /// Terminal rows.
    height: usize,
    /// Braille codepoint offsets (width × height). Each u16 stores OR'd dot bits.
    buffer: Vec<u16>,
    /// Per-cell foreground color (width × height). Last write wins.
    colors: Vec<Color>,
}

/// Braille dot bit mapping.
/// Cell layout:    Bit positions:
///   1 4             0x01  0x08
///   2 5             0x02  0x10
///   3 6             0x04  0x20
///   7 8             0x40  0x80
const BRAILLE_BITS: [[u16; 4]; 2] = [
    [0x01, 0x02, 0x04, 0x40], // left column  (px=0)
    [0x08, 0x10, 0x20, 0x80], // right column (px=1)
];

/// Unicode Braille base codepoint (U+2800 = empty Braille pattern).
const BRAILLE_BASE: u16 = 0x2800;

impl BrailleCanvas {
    pub fn new(width: usize, height: usize) -> Self {
        let size = width * height;
        Self {
            width,
            height,
            buffer: vec![0; size],
            colors: vec![Color::White; size],
        }
    }

    /// Pixel resolution (2× width, 4× height).
    pub fn pixel_width(&self) -> usize {
        self.width * 2
    }

    pub fn pixel_height(&self) -> usize {
        self.height * 4
    }

    /// Set a single pixel in Braille space.
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        let cx = x / 2;
        let cy = y / 4;
        if cx >= self.width || cy >= self.height {
            return;
        }
        let px = x % 2;
        let py = y % 4;
        let idx = cy * self.width + cx;
        self.buffer[idx] |= BRAILLE_BITS[px][py];
        self.colors[idx] = color;
    }

    /// Draw a line using Bresenham's algorithm in Braille pixel space.
    pub fn draw_line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, color: Color) {
        let mut x0 = x0 as i32;
        let mut y0 = y0 as i32;
        let x1 = x1 as i32;
        let y1 = y1 as i32;

        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        let pw = self.pixel_width() as i32;
        let ph = self.pixel_height() as i32;

        loop {
            if x0 >= 0 && x0 < pw && y0 >= 0 && y0 < ph {
                self.set_pixel(x0 as usize, y0 as usize, color);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                if x0 == x1 {
                    break;
                }
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                if y0 == y1 {
                    break;
                }
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Draw a thick line by drawing adjacent parallel pixels (for UserLink edges).
    pub fn draw_thick_line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, color: Color) {
        // Draw the main line plus one pixel offset on each side
        self.draw_line(x0, y0, x1, y1, color);

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return;
        }
        // Perpendicular offset (1 pixel)
        let nx = -dy / len;
        let ny = dx / len;
        self.draw_line(x0 + nx, y0 + ny, x1 + nx, y1 + ny, color);
        self.draw_line(x0 - nx, y0 - ny, x1 - nx, y1 - ny, color);
    }

    /// Draw a dashed line (for semantic edges).
    pub fn draw_dashed_line(
        &mut self,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        color: Color,
        dash_len: usize,
        gap_len: usize,
    ) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let total_len = (dx * dx + dy * dy).sqrt();
        if total_len < 1.0 {
            return;
        }
        let step_x = dx / total_len;
        let step_y = dy / total_len;
        let cycle = dash_len + gap_len;

        let pw = self.pixel_width() as i32;
        let ph = self.pixel_height() as i32;

        let mut i = 0usize;
        let total = total_len as usize;
        while i < total {
            let phase = i % cycle;
            if phase < dash_len {
                let px = (x0 + step_x * i as f64) as i32;
                let py = (y0 + step_y * i as f64) as i32;
                if px >= 0 && px < pw && py >= 0 && py < ph {
                    self.set_pixel(px as usize, py as usize, color);
                }
            }
            i += 1;
        }
    }

    /// Convert the buffer to a vector of Ratatui Span rows.
    /// Each row is a Vec<Span> — one Span per cell (for per-cell coloring).
    pub fn to_spans(&self) -> Vec<Vec<Span<'static>>> {
        let mut rows = Vec::with_capacity(self.height);
        for y in 0..self.height {
            let mut row = Vec::with_capacity(self.width);
            for x in 0..self.width {
                let idx = y * self.width + x;
                let bits = self.buffer[idx];
                let ch = char::from_u32((BRAILLE_BASE | bits) as u32).unwrap_or(' ');
                let color = self.colors[idx];
                if bits == 0 {
                    row.push(Span::raw(" "));
                } else {
                    row.push(Span::styled(
                        ch.to_string(),
                        Style::default().fg(color),
                    ));
                }
            }
            rows.push(row);
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_pixel() {
        let mut canvas = BrailleCanvas::new(10, 5);
        canvas.set_pixel(0, 0, Color::White);
        assert_eq!(canvas.buffer[0], 0x01);

        canvas.set_pixel(1, 0, Color::White);
        assert_eq!(canvas.buffer[0], 0x01 | 0x08);
    }

    #[test]
    fn test_pixel_resolution() {
        let canvas = BrailleCanvas::new(100, 50);
        assert_eq!(canvas.pixel_width(), 200);
        assert_eq!(canvas.pixel_height(), 200);
    }

    #[test]
    fn test_out_of_bounds() {
        let mut canvas = BrailleCanvas::new(10, 5);
        // Should not panic
        canvas.set_pixel(999, 999, Color::White);
    }
}
