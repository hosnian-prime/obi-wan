use super::math::{Rect2D, Vec2};

/// Barnes-Hut quadtree for O(n log n) force approximation.
///
/// Spatial partition of 2D space. Each cell stores the center of mass
/// and total mass of all nodes within it. Distant cells are treated
/// as single point masses for force calculation.
/// Max recursion depth to prevent stack overflow with many overlapping nodes.
const MAX_DEPTH: usize = 40;

pub struct QuadTree {
    bounds: Rect2D,
    center_of_mass: Vec2,
    total_mass: f64,
    body: Option<usize>, // index into positions array (leaf)
    children: Option<Box<[QuadTree; 4]>>, // NW, NE, SW, SE
    depth: usize,
}

impl QuadTree {
    /// Build a quadtree from a set of node positions.
    pub fn build(positions: &[Vec2]) -> Self {
        if positions.is_empty() {
            return Self::empty(Rect2D::new(0.0, 0.0, 1.0, 1.0));
        }

        // Compute bounding box
        let mut bounds = Rect2D::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in positions {
            bounds.include(*p);
        }
        // Add small padding to avoid zero-size bounds
        let pad = bounds.width().max(bounds.height()).max(1.0) * 0.01;
        bounds.min.x -= pad;
        bounds.min.y -= pad;
        bounds.max.x += pad;
        bounds.max.y += pad;

        let mut tree = Self::empty(bounds);
        for (i, _pos) in positions.iter().enumerate() {
            tree.insert(i, positions);
        }
        tree
    }

    fn empty(bounds: Rect2D) -> Self {
        Self::with_depth(bounds, 0)
    }

    fn with_depth(bounds: Rect2D, depth: usize) -> Self {
        Self {
            bounds,
            center_of_mass: Vec2::ZERO,
            total_mass: 0.0,
            body: None,
            children: None,
            depth,
        }
    }

    fn insert(&mut self, idx: usize, positions: &[Vec2]) {
        let pos = positions[idx];

        if self.total_mass == 0.0 && self.body.is_none() {
            // Empty leaf — place body here
            self.body = Some(idx);
            self.center_of_mass = pos;
            self.total_mass = 1.0;
            return;
        }

        // At max depth, just accumulate mass (don't subdivide further)
        if self.depth >= MAX_DEPTH {
            let new_mass = self.total_mass + 1.0;
            self.center_of_mass = Vec2::new(
                (self.center_of_mass.x * self.total_mass + pos.x) / new_mass,
                (self.center_of_mass.y * self.total_mass + pos.y) / new_mass,
            );
            self.total_mass = new_mass;
            return;
        }

        // If this is a leaf with a body, subdivide
        if let Some(existing_idx) = self.body.take() {
            self.subdivide();
            self.insert_into_child(existing_idx, positions);
        }

        // Insert new body into appropriate child
        if self.children.is_some() {
            self.insert_into_child(idx, positions);
        }

        // Update center of mass
        let new_mass = self.total_mass + 1.0;
        self.center_of_mass = Vec2::new(
            (self.center_of_mass.x * self.total_mass + pos.x) / new_mass,
            (self.center_of_mass.y * self.total_mass + pos.y) / new_mass,
        );
        self.total_mass = new_mass;
    }

    fn subdivide(&mut self) {
        let b = self.bounds;
        let d = self.depth + 1;
        self.children = Some(Box::new([
            Self::with_depth(b.quadrant(0), d),
            Self::with_depth(b.quadrant(1), d),
            Self::with_depth(b.quadrant(2), d),
            Self::with_depth(b.quadrant(3), d),
        ]));
    }

    fn insert_into_child(&mut self, idx: usize, positions: &[Vec2]) {
        let pos = positions[idx];
        let children = self.children.as_mut().unwrap();
        let cx = (self.bounds.min.x + self.bounds.max.x) / 2.0;
        let cy = (self.bounds.min.y + self.bounds.max.y) / 2.0;
        let qi = if pos.x < cx {
            if pos.y < cy { 0 } else { 2 }
        } else if pos.y < cy {
            1
        } else {
            3
        };
        children[qi].insert(idx, positions);
    }

    /// Compute the repulsive force on a node at `pos` from this quadtree cell.
    ///
    /// `theta` controls accuracy: higher = faster but less accurate (0.8 typical).
    /// `repulsion_k` is the repulsion constant.
    ///
    /// Returns the force vector to apply to the node.
    pub fn compute_force(&self, pos: Vec2, theta: f64, repulsion_k: f64) -> Vec2 {
        if self.total_mass == 0.0 {
            return Vec2::ZERO;
        }

        let diff = pos - self.center_of_mass;
        let dist_sq = diff.length_sq().max(0.01); // avoid division by zero
        let dist = dist_sq.sqrt();

        let cell_size = self.bounds.width().max(self.bounds.height());

        // Barnes-Hut criterion: if cell is far enough, treat as point mass
        if self.children.is_none() || cell_size / dist < theta {
            // Coulomb-like repulsion: F = k * m / d²
            let force_mag = repulsion_k * self.total_mass / dist_sq;
            return diff.normalized() * force_mag;
        }

        // Cell too close — recurse into children
        let mut force = Vec2::ZERO;
        if let Some(ref children) = self.children {
            for child in children.iter() {
                force += child.compute_force(pos, theta, repulsion_k);
            }
        }
        force
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_empty() {
        let tree = QuadTree::build(&[]);
        assert_eq!(tree.total_mass, 0.0);
    }

    #[test]
    fn test_build_single() {
        let positions = vec![Vec2::new(5.0, 5.0)];
        let tree = QuadTree::build(&positions);
        assert_eq!(tree.total_mass, 1.0);
    }

    #[test]
    fn test_force_repulsion() {
        let positions = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)];
        let tree = QuadTree::build(&positions);

        let force = tree.compute_force(Vec2::new(0.0, 0.0), 0.8, 100.0);
        // Force should push node at origin away from node at (10,0)
        // But also has self-repulsion via center of mass
        assert!(force.length() > 0.0);
    }

    #[test]
    fn test_many_nodes() {
        let positions: Vec<Vec2> = (0..100)
            .map(|i| Vec2::new((i % 10) as f64 * 10.0, (i / 10) as f64 * 10.0))
            .collect();
        let tree = QuadTree::build(&positions);
        assert_eq!(tree.total_mass, 100.0);

        // Should not panic
        let force = tree.compute_force(Vec2::new(50.0, 50.0), 0.8, 100.0);
        assert!(force.length().is_finite());
    }
}
