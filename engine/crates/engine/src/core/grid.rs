//! Grid helpers for 2D tile-based gameplay: world/cell conversion and facing.

use crate::core::transform::Vec2;

/// Converts a world-space position to a cell coordinate using floor division.
///
/// Cells are indexed from `(0, 0)` at the world origin. Negative world positions
/// map to negative cell indices (caller decides whether that is in-bounds).
#[must_use]
pub fn world_to_cell(x: f32, y: f32, tile_size: f32) -> (i32, i32) {
    debug_assert!(tile_size.is_finite() && tile_size > 0.0);
    let size = tile_size.max(f32::EPSILON);
    ((x / size).floor() as i32, (y / size).floor() as i32)
}

/// Returns the world-space center of a cell.
#[must_use]
pub fn cell_to_world(cx: i32, cy: i32, tile_size: f32) -> Vec2 {
    debug_assert!(tile_size.is_finite() && tile_size > 0.0);
    let size = tile_size.max(f32::EPSILON);
    Vec2::new((cx as f32 + 0.5) * size, (cy as f32 + 0.5) * size)
}

/// Cardinal facing used by [`facing_cell`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Facing {
    Up,
    Down,
    Left,
    Right,
}

impl Facing {
    /// Parses a facing from a short string (`up`/`down`/`left`/`right` or
    /// `n`/`s`/`w`/`e`). Unknown values default to [`Facing::Down`].
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "up" | "n" | "north" => Self::Up,
            "left" | "w" | "west" => Self::Left,
            "right" | "e" | "east" => Self::Right,
            _ => Self::Down,
        }
    }

    /// Unit step on the cell grid for this facing.
    #[must_use]
    pub fn step(self) -> (i32, i32) {
        match self {
            Self::Up => (0, 1),
            Self::Down => (0, -1),
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
        }
    }
}

/// Returns the cell one step in `facing` from the cell that contains `(x, y)`.
#[must_use]
pub fn facing_cell(x: f32, y: f32, facing: Facing, tile_size: f32) -> (i32, i32) {
    let (cx, cy) = world_to_cell(x, y, tile_size);
    let (dx, dy) = facing.step();
    (cx + dx, cy + dy)
}

/// Returns whether `(cx, cy)` lies inside a `[0, width) × [0, height)` map.
#[must_use]
pub fn in_bounds(cx: i32, cy: i32, width: usize, height: usize) -> bool {
    cx >= 0 && cy >= 0 && (cx as usize) < width && (cy as usize) < height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_and_cell_round_trip_centers() {
        let (cx, cy) = world_to_cell(48.0, 16.0, 32.0);
        assert_eq!((cx, cy), (1, 0));
        let center = cell_to_world(cx, cy, 32.0);
        assert_eq!(center, Vec2::new(48.0, 16.0));
    }

    #[test]
    fn world_to_cell_floors_negatives() {
        assert_eq!(world_to_cell(-0.1, -32.0, 32.0), (-1, -1));
    }

    #[test]
    fn facing_cell_steps_cardinally() {
        assert_eq!(facing_cell(16.0, 16.0, Facing::Right, 32.0), (1, 0));
        assert_eq!(facing_cell(16.0, 16.0, Facing::Up, 32.0), (0, 1));
        assert_eq!(Facing::parse("west"), Facing::Left);
        assert_eq!(Facing::parse("???"), Facing::Down);
    }

    #[test]
    fn in_bounds_checks_dimensions() {
        assert!(in_bounds(0, 0, 4, 3));
        assert!(in_bounds(3, 2, 4, 3));
        assert!(!in_bounds(4, 0, 4, 3));
        assert!(!in_bounds(-1, 0, 4, 3));
    }
}
