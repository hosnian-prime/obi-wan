use std::collections::HashMap;

use obi_core::node::NodeId;

use super::math::{ease_out, lerp_f64, Rect2D, Vec2};

/// Camera viewport for the graph panel.
///
/// Manages the mapping between world coordinates (where nodes live)
/// and screen coordinates (terminal cells / Braille pixels).
pub struct Viewport {
    /// Camera center in world coordinates.
    pub center: Vec2,
    /// Zoom level: 0.1 (far) to 5.0 (close).
    pub zoom: f64,
    /// Terminal cell size of the graph panel (width, height).
    pub size: (u16, u16),
    /// Active animated transition, if any.
    transition: Option<ViewTransition>,
}

struct ViewTransition {
    from_center: Vec2,
    from_zoom: f64,
    to_center: Vec2,
    to_zoom: f64,
    progress: f64,
    duration_frames: u32,
    elapsed_frames: u32,
}

impl Viewport {
    pub fn new() -> Self {
        Self {
            center: Vec2::ZERO,
            zoom: 1.0,
            size: (80, 40),
            transition: None,
        }
    }

    /// Convert world coordinates to Braille pixel coordinates.
    pub fn world_to_pixel(&self, world_pos: Vec2) -> (f64, f64) {
        let pixel_w = self.size.0 as f64 * 2.0; // Braille: 2 dots per cell horizontally
        let pixel_h = self.size.1 as f64 * 4.0; // Braille: 4 dots per cell vertically

        let dx = (world_pos.x - self.center.x) * self.zoom;
        let dy = (world_pos.y - self.center.y) * self.zoom;

        let px = pixel_w / 2.0 + dx;
        let py = pixel_h / 2.0 + dy;
        (px, py)
    }

    /// Convert Braille pixel coordinates to world coordinates.
    pub fn pixel_to_world(&self, px: f64, py: f64) -> Vec2 {
        let pixel_w = self.size.0 as f64 * 2.0;
        let pixel_h = self.size.1 as f64 * 4.0;

        let dx = (px - pixel_w / 2.0) / self.zoom;
        let dy = (py - pixel_h / 2.0) / self.zoom;

        Vec2::new(self.center.x + dx, self.center.y + dy)
    }

    /// Convert world coordinates to terminal cell coordinates (for text overlay).
    pub fn world_to_cell(&self, world_pos: Vec2) -> (i32, i32) {
        let (px, py) = self.world_to_pixel(world_pos);
        ((px / 2.0) as i32, (py / 4.0) as i32)
    }

    /// Get the visible world-space rectangle (for frustum culling).
    pub fn visible_rect(&self) -> Rect2D {
        let half_w = (self.size.0 as f64) / self.zoom;
        let half_h = (self.size.1 as f64 * 2.0) / self.zoom; // account for aspect ratio
        Rect2D::from_center_size(self.center, half_w, half_h)
    }

    /// Auto-fit all nodes in the viewport.
    pub fn fit_all(&mut self, positions: &HashMap<NodeId, Vec2>, animate: bool) {
        if positions.is_empty() {
            return;
        }

        let mut bounds = Rect2D::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for pos in positions.values() {
            bounds.include(*pos);
        }

        // Add padding (10%)
        let pad_x = bounds.width() * 0.1 + 5.0;
        let pad_y = bounds.height() * 0.1 + 5.0;
        bounds.min.x -= pad_x;
        bounds.min.y -= pad_y;
        bounds.max.x += pad_x;
        bounds.max.y += pad_y;

        let target_center = bounds.center();

        // Compute zoom to fit bounds in viewport
        let zoom_x = if bounds.width() > 0.0 {
            self.size.0 as f64 / bounds.width()
        } else {
            1.0
        };
        let zoom_y = if bounds.height() > 0.0 {
            (self.size.1 as f64 * 2.0) / bounds.height()
        } else {
            1.0
        };
        let target_zoom = zoom_x.min(zoom_y).clamp(0.1, 5.0);

        if animate {
            self.animate_to(target_center, target_zoom);
        } else {
            self.center = target_center;
            self.zoom = target_zoom;
        }
    }

    /// Smoothly pan + zoom to center on a specific node.
    pub fn focus_node(&mut self, pos: Vec2) {
        self.animate_to(pos, 1.5_f64.min(self.zoom.max(1.0)));
    }

    /// Pan the viewport by delta (in world units).
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.transition = None; // cancel any animation
        self.center.x += dx / self.zoom;
        self.center.y += dy / self.zoom;
    }

    /// Zoom in/out by a factor, clamped to [0.1, 5.0].
    pub fn zoom_by(&mut self, factor: f64) {
        self.transition = None;
        self.zoom = (self.zoom * factor).clamp(0.1, 5.0);
    }

    /// Zoom towards a specific world position (keeps target point visually stable).
    pub fn zoom_towards(&mut self, target: Vec2, factor: f64) {
        self.transition = None;
        let old_zoom = self.zoom;
        let new_zoom = (old_zoom * factor).clamp(0.1, 5.0);
        // Shift center so that the target point stays at the same screen position
        self.center.x += (target.x - self.center.x) * (1.0 - old_zoom / new_zoom);
        self.center.y += (target.y - self.center.y) * (1.0 - old_zoom / new_zoom);
        self.zoom = new_zoom;
    }

    /// Update size (called on terminal resize or panel resize).
    pub fn set_size(&mut self, width: u16, height: u16) {
        self.size = (width, height);
    }

    /// Start an animated transition.
    fn animate_to(&mut self, target_center: Vec2, target_zoom: f64) {
        self.transition = Some(ViewTransition {
            from_center: self.center,
            from_zoom: self.zoom,
            to_center: target_center,
            to_zoom: target_zoom,
            progress: 0.0,
            duration_frames: 12,
            elapsed_frames: 0,
        });
    }

    /// Advance animation by one frame. Returns true if animation is active.
    pub fn tick_animation(&mut self) -> bool {
        let transition = match self.transition.as_mut() {
            Some(t) => t,
            None => return false,
        };

        transition.elapsed_frames += 1;
        transition.progress =
            (transition.elapsed_frames as f64 / transition.duration_frames as f64).min(1.0);

        let t = ease_out(transition.progress);
        self.center = transition.from_center.lerp(transition.to_center, t);
        self.zoom = lerp_f64(transition.from_zoom, transition.to_zoom, t);

        if transition.progress >= 1.0 {
            self.transition = None;
            return false;
        }
        true
    }

    /// Current zoom level category for LOD rendering.
    pub fn zoom_level(&self) -> ZoomLevel {
        if self.zoom < 0.3 {
            ZoomLevel::Far
        } else if self.zoom < 1.0 {
            ZoomLevel::Medium
        } else {
            ZoomLevel::Close
        }
    }
}

/// Level-of-detail zoom categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    /// 0.1–0.3: colored dot only.
    Far,
    /// 0.3–1.0: dot + truncated 8-char name.
    Medium,
    /// >1.0: full name + type icon + connection count.
    Close,
}
