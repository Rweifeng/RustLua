//! Grid navigation for RustLua: A* over walkable cells, line-of-sight path
//! smoothing, and deterministic path-following agents.
//!
//! The crate is deliberately data-oriented and platform-free: it depends only on
//! `rustlua-math` and `thiserror`, so it can be used from the runtime, from the
//! editor, from headless acceptance drivers and from the wasm host alike.
//!
//! Coordinates follow the engine's 2D convention (`docs/06`): `+x` right, `+y`
//! down in cell space, world position of a cell is its center:
//! `origin + ((x + 0.5) * cell_size, (y + 0.5) * cell_size)`.
//!
//! Determinism is part of the contract: neighbor iteration order, heap
//! tie-breaking and agent integration use a fixed order and no hashing. Two
//! runs with identical inputs produce bit-identical positions, which is what
//! lockstep networking relies on.

use rustlua_math::math2d::Vec2;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Errors produced by [`NavGrid`] and [`NavAgent`].
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum NavError {
    #[error("navigation grid must be at least 1x1 (got {width}x{height})")]
    ZeroSize { width: usize, height: usize },
    #[error("cell size must be finite and positive (got {0})")]
    InvalidCellSize(f32),
    #[error("navigation grid origin must be finite (got {0:?})")]
    InvalidOrigin(Vec2),
    #[error("blocked-mask length {len} does not match {width}x{height}")]
    MaskLength {
        len: usize,
        width: usize,
        height: usize,
    },
    #[error("cell ({x}, {y}) is out of bounds")]
    OutOfBounds { x: i32, y: i32 },
    #[error("cell ({x}, {y}) is blocked")]
    Blocked { x: usize, y: usize },
    #[error("no route from ({from_x}, {from_y}) to ({to_x}, {to_y})")]
    Unreachable {
        from_x: usize,
        from_y: usize,
        to_x: usize,
        to_y: usize,
    },
    #[error("world point must be finite (got {0:?})")]
    NonFinitePoint(Vec2),
    #[error("agent speed and radius must be finite and non-negative")]
    InvalidAgent,
}

/// A rectangular walkable/blocked grid.
#[derive(Clone, Debug, PartialEq)]
pub struct NavGrid {
    width: usize,
    height: usize,
    cell_size: f32,
    origin: Vec2,
    blocked: Vec<bool>,
}

impl NavGrid {
    /// Creates a fully walkable grid.
    pub fn new(width: usize, height: usize, cell_size: f32) -> Result<Self, NavError> {
        Self::with_origin(width, height, cell_size, Vec2::zero())
    }

    /// Creates a fully walkable grid whose cell centers start at `origin`.
    pub fn with_origin(
        width: usize,
        height: usize,
        cell_size: f32,
        origin: Vec2,
    ) -> Result<Self, NavError> {
        if width == 0 || height == 0 {
            return Err(NavError::ZeroSize { width, height });
        }
        if !cell_size.is_finite() || cell_size <= 0.0 {
            return Err(NavError::InvalidCellSize(cell_size));
        }
        if !origin.is_finite() {
            return Err(NavError::InvalidOrigin(origin));
        }
        Ok(Self {
            width,
            height,
            cell_size,
            origin,
            blocked: vec![false; width * height],
        })
    }

    /// Creates a grid from a row-major blocked mask (`true` = wall).
    pub fn from_blocked(
        width: usize,
        height: usize,
        cell_size: f32,
        origin: Vec2,
        blocked: Vec<bool>,
    ) -> Result<Self, NavError> {
        let mut grid = Self::with_origin(width, height, cell_size, origin)?;
        if blocked.len() != grid.blocked.len() {
            return Err(NavError::MaskLength {
                len: blocked.len(),
                width,
                height,
            });
        }
        grid.blocked = blocked;
        Ok(grid)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    pub fn origin(&self) -> Vec2 {
        self.origin
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then_some(y * self.width + x)
    }

    /// Returns whether a cell is walkable; out-of-bounds cells count as walls.
    pub fn is_walkable(&self, x: usize, y: usize) -> bool {
        self.index(x, y).is_some_and(|index| !self.blocked[index])
    }

    /// Marks a cell as wall (`blocked = true`) or walkable. Endpoints of an
    /// in-flight path are not re-validated; callers re-path after changing the
    /// mask (same contract as `tilemap.set_solid`).
    pub fn set_blocked(&mut self, x: usize, y: usize, blocked: bool) -> Result<(), NavError> {
        let index = self.index(x, y).ok_or(NavError::OutOfBounds {
            x: x as i32,
            y: y as i32,
        })?;
        self.blocked[index] = blocked;
        Ok(())
    }

    /// Converts a world point to the cell containing it.
    pub fn world_to_cell(&self, point: Vec2) -> Result<(usize, usize), NavError> {
        if !point.is_finite() {
            return Err(NavError::NonFinitePoint(point));
        }
        let local_x = (point.x - self.origin.x) / self.cell_size;
        let local_y = (point.y - self.origin.y) / self.cell_size;
        if local_x < 0.0 || local_y < 0.0 {
            return Err(NavError::OutOfBounds {
                x: local_x.floor() as i32,
                y: local_y.floor() as i32,
            });
        }
        let x = local_x.floor() as usize;
        let y = local_y.floor() as usize;
        if x >= self.width || y >= self.height {
            return Err(NavError::OutOfBounds {
                x: x as i32,
                y: y as i32,
            });
        }
        Ok((x, y))
    }

    /// World-space center of a cell.
    pub fn cell_center(&self, (x, y): (usize, usize)) -> Vec2 {
        Vec2::new(
            self.origin.x + (x as f32 + 0.5) * self.cell_size,
            self.origin.y + (y as f32 + 0.5) * self.cell_size,
        )
    }

    /// A* over walkable cells. Returns the cell path including both endpoints.
    ///
    /// With `allow_diagonal`, diagonal steps cost the same as orthogonal ones
    /// (matching `Tilemap::find_path`) but may never cut a blocked corner.
    /// Neighbor order and heap tie-breaking are fixed, so the returned path is
    /// deterministic for a given grid and endpoints.
    pub fn find_path(
        &self,
        start: (usize, usize),
        goal: (usize, usize),
        allow_diagonal: bool,
    ) -> Result<Vec<(usize, usize)>, NavError> {
        let start_index = self.index(start.0, start.1).ok_or(NavError::OutOfBounds {
            x: start.0 as i32,
            y: start.1 as i32,
        })?;
        let goal_index = self.index(goal.0, goal.1).ok_or(NavError::OutOfBounds {
            x: goal.0 as i32,
            y: goal.1 as i32,
        })?;
        if self.blocked[start_index] {
            return Err(NavError::Blocked {
                x: start.0,
                y: start.1,
            });
        }
        if self.blocked[goal_index] {
            return Err(NavError::Blocked {
                x: goal.0,
                y: goal.1,
            });
        }
        if start == goal {
            return Ok(vec![start]);
        }

        let (gx, gy) = (goal.0 as i32, goal.1 as i32);
        let heuristic = |x: usize, y: usize| -> i32 {
            let dx = (x as i32 - gx).abs();
            let dy = (y as i32 - gy).abs();
            if allow_diagonal { dx.max(dy) } else { dx + dy }
        };

        #[derive(Clone, Copy, Eq, PartialEq)]
        struct Node {
            f: i32,
            g: i32,
            index: usize,
        }
        impl Ord for Node {
            fn cmp(&self, other: &Self) -> Ordering {
                // Min-heap on f, then g, then a stable cell index so equal-cost
                // frontiers always pop in the same order.
                other
                    .f
                    .cmp(&self.f)
                    .then_with(|| other.g.cmp(&self.g))
                    .then_with(|| other.index.cmp(&self.index))
            }
        }
        impl PartialOrd for Node {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut open = BinaryHeap::new();
        let mut g_score = vec![i32::MAX; self.width * self.height];
        let mut came_from: Vec<Option<usize>> = vec![None; self.width * self.height];
        g_score[start_index] = 0;
        open.push(Node {
            f: heuristic(start.0, start.1),
            g: 0,
            index: start_index,
        });

        while let Some(current) = open.pop() {
            if current.g > g_score[current.index] {
                continue;
            }
            if current.index == goal_index {
                let mut path = Vec::new();
                let mut cursor = goal_index;
                loop {
                    path.push((cursor % self.width, cursor / self.width));
                    if cursor == start_index {
                        break;
                    }
                    let Some(previous) = came_from[cursor] else {
                        return Err(NavError::Unreachable {
                            from_x: start.0,
                            from_y: start.1,
                            to_x: goal.0,
                            to_y: goal.1,
                        });
                    };
                    cursor = previous;
                }
                path.reverse();
                return Ok(path);
            }
            let x = current.index % self.width;
            let y = current.index / self.width;
            for (nx, ny) in self.neighbors(x, y, allow_diagonal) {
                let neighbor = ny * self.width + nx;
                let tentative = current.g + 1;
                if tentative < g_score[neighbor] {
                    g_score[neighbor] = tentative;
                    came_from[neighbor] = Some(current.index);
                    open.push(Node {
                        f: tentative + heuristic(nx, ny),
                        g: tentative,
                        index: neighbor,
                    });
                }
            }
        }

        Err(NavError::Unreachable {
            from_x: start.0,
            from_y: start.1,
            to_x: goal.0,
            to_y: goal.1,
        })
    }

    fn neighbors(&self, x: usize, y: usize, allow_diagonal: bool) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(8);
        // Fixed order: E, W, S, N, then SE, SW, NE, NW.
        for (dx, dy) in [(1_i32, 0_i32), (-1, 0), (0, 1), (0, -1)] {
            if let Some(cell) = self.step(x, y, dx, dy) {
                out.push(cell);
            }
        }
        if allow_diagonal {
            for (dx, dy) in [(1_i32, 1_i32), (-1, 1), (1, -1), (-1, -1)] {
                // Diagonal moves require both orthogonal cells to be walkable;
                // this is what prevents corner cutting through a wall.
                let Some(side_a) = self.step(x, y, dx, 0) else {
                    continue;
                };
                let Some(side_b) = self.step(x, y, 0, dy) else {
                    continue;
                };
                let Some(cell) = self.step(x, y, dx, dy) else {
                    continue;
                };
                if self.is_walkable(side_a.0, side_a.1) && self.is_walkable(side_b.0, side_b.1) {
                    out.push(cell);
                }
            }
        }
        out
    }

    fn step(&self, x: usize, y: usize, dx: i32, dy: i32) -> Option<(usize, usize)> {
        let nx = x as i32 + dx;
        let ny = y as i32 + dy;
        if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
            return None;
        }
        let cell = (nx as usize, ny as usize);
        self.is_walkable(cell.0, cell.1).then_some(cell)
    }

    /// Whether a straight line between two cell centers stays on walkable cells.
    ///
    /// Uses Bresenham traversal; diagonal steps additionally require both
    /// orthogonal cells to be walkable, mirroring `find_path`'s corner rule.
    /// Endpoints must be walkable.
    pub fn line_of_sight(&self, from: (usize, usize), to: (usize, usize)) -> bool {
        if !self.is_walkable(from.0, from.1) || !self.is_walkable(to.0, to.1) {
            return false;
        }
        let (mut x, mut y) = (from.0 as i32, from.1 as i32);
        let (tx, ty) = (to.0 as i32, to.1 as i32);
        let dx = (tx - x).abs();
        let dy = (ty - y).abs();
        let sx = if tx > x { 1 } else { -1 };
        let sy = if ty > y { 1 } else { -1 };
        let mut err = dx - dy;
        loop {
            if !self.is_walkable(x as usize, y as usize) {
                return false;
            }
            if x == tx && y == ty {
                return true;
            }
            let e2 = 2 * err;
            if e2 > -dy && e2 < dx {
                if !self.is_walkable((x + sx) as usize, y as usize)
                    || !self.is_walkable(x as usize, (y + sy) as usize)
                {
                    return false;
                }
                err -= dy;
                err += dx;
                x += sx;
                y += sy;
            } else if e2 > -dy {
                err -= dy;
                x += sx;
            } else {
                err += dx;
                y += sy;
            }
        }
    }

    /// String-pulls a cell path into a shorter, geometrically straight path.
    ///
    /// The first and last cells are preserved; every returned segment is
    /// validated with [`Self::line_of_sight`], so smoothing never creates a
    /// shortcut through a wall.
    pub fn smooth_path(&self, cells: &[(usize, usize)]) -> Vec<(usize, usize)> {
        if cells.len() <= 2 {
            return cells.to_vec();
        }
        let mut out = Vec::with_capacity(cells.len());
        out.push(cells[0]);
        let mut anchor = 0;
        while anchor + 1 < cells.len() {
            let mut furthest = anchor + 1;
            for candidate in (anchor + 2)..cells.len() {
                if self.line_of_sight(cells[anchor], cells[candidate]) {
                    furthest = candidate;
                } else {
                    break;
                }
            }
            out.push(cells[furthest]);
            anchor = furthest;
        }
        out
    }

    /// Convenience: A* plus smoothing, returning world-space waypoints.
    pub fn find_world_path(
        &self,
        start: Vec2,
        goal: Vec2,
        allow_diagonal: bool,
    ) -> Result<NavPath, NavError> {
        let start_cell = self.world_to_cell(start)?;
        let goal_cell = self.world_to_cell(goal)?;
        let cells = self.find_path(start_cell, goal_cell, allow_diagonal)?;
        let smoothed = self.smooth_path(&cells);
        let points = smoothed
            .iter()
            .map(|cell| self.cell_center(*cell))
            .collect();
        Ok(NavPath {
            cells: smoothed,
            points,
        })
    }
}

/// A path of cell centers produced by [`NavGrid::find_world_path`].
#[derive(Clone, Debug, PartialEq)]
pub struct NavPath {
    cells: Vec<(usize, usize)>,
    points: Vec<Vec2>,
}

impl NavPath {
    pub fn cells(&self) -> &[(usize, usize)] {
        &self.cells
    }

    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn goal(&self) -> Option<Vec2> {
        self.points.last().copied()
    }
}

/// A circular obstacle used for local avoidance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub position: Vec2,
    pub radius: f32,
}

/// Result of one agent step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgentStep {
    pub position: Vec2,
    pub arrived: bool,
    pub waypoint: usize,
}

/// Follows a world-space path at a fixed speed with separation steering.
#[derive(Clone, Debug, PartialEq)]
pub struct NavAgent {
    position: Vec2,
    speed: f32,
    radius: f32,
    waypoint_tolerance: f32,
    path: Vec<Vec2>,
    cursor: usize,
}

impl NavAgent {
    pub fn new(position: Vec2, speed: f32, radius: f32) -> Result<Self, NavError> {
        if !position.is_finite()
            || !speed.is_finite()
            || !radius.is_finite()
            || speed < 0.0
            || radius < 0.0
        {
            return Err(NavError::InvalidAgent);
        }
        Ok(Self {
            position,
            speed,
            radius,
            waypoint_tolerance: radius.max(0.05),
            path: Vec::new(),
            cursor: 0,
        })
    }

    pub fn position(&self) -> Vec2 {
        self.position
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    pub fn path(&self) -> &[Vec2] {
        &self.path
    }

    pub fn waypoint(&self) -> Option<Vec2> {
        self.path.get(self.cursor).copied()
    }

    pub fn arrived(&self) -> bool {
        self.cursor >= self.path.len()
    }

    /// Replaces the current path. The first point is treated as the next
    /// waypoint, so callers that already stand on it simply advance.
    pub fn set_path(&mut self, points: Vec<Vec2>) {
        self.path = points;
        self.cursor = 0;
        self.skip_reached_waypoints();
    }

    pub fn clear_path(&mut self) {
        self.path.clear();
        self.cursor = 0;
    }

    fn skip_reached_waypoints(&mut self) {
        while let Some(target) = self.path.get(self.cursor).copied() {
            if distance(self.position, target) <= self.waypoint_tolerance {
                self.cursor += 1;
            } else {
                break;
            }
        }
    }

    /// Advances the agent by `dt` seconds, combining path seeking with
    /// separation from `obstacles` (other agents / static circles).
    ///
    /// The combination is normalized back to `speed`, so avoidance can change
    /// direction but never makes an agent faster than its own speed.
    pub fn update(&mut self, dt: f32, obstacles: &[Obstacle]) -> AgentStep {
        if !dt.is_finite() || dt <= 0.0 {
            self.skip_reached_waypoints();
            return AgentStep {
                position: self.position,
                arrived: self.arrived(),
                waypoint: self.cursor,
            };
        }

        let was_arrived = self.arrived();
        let mut desired = if was_arrived {
            Vec2::zero()
        } else {
            let target = self.path[self.cursor];
            normalize(sub(target, self.position)).unwrap_or(Vec2::zero())
        };
        let separation = self.separation(obstacles);
        if separation.length() > 0.0 {
            // Weight avoidance below pure pursuit so a crowd still makes
            // progress toward the objective.
            desired = normalize(add(desired, mul(separation, 1.5))).unwrap_or(desired);
        }

        if desired.length() > 0.0 {
            let step = mul(desired, self.speed * dt);
            self.position = add(self.position, step);
            self.skip_reached_waypoints();
            // Snap to the final waypoint so a completed path lands on the exact
            // goal instead of stopping one tolerance short. Only do this on the
            // step that actually completes the path; already-arrived agents
            // stay free to be pushed apart by separation.
            if !was_arrived
                && self.arrived()
                && let Some(last) = self.path.last().copied()
            {
                self.position = last;
            }
        }

        AgentStep {
            position: self.position,
            arrived: self.arrived(),
            waypoint: self.cursor,
        }
    }

    fn separation(&self, obstacles: &[Obstacle]) -> Vec2 {
        let mut push = Vec2::zero();
        for obstacle in obstacles {
            let delta = sub(self.position, obstacle.position);
            let distance = delta.length();
            let combined = self.radius + obstacle.radius;
            if distance >= combined || combined <= 0.0 {
                continue;
            }
            let direction = if distance > 1e-4 {
                mul(delta, 1.0 / distance)
            } else {
                // Deterministic tie-break for perfectly overlapping agents.
                Vec2::new(1.0, 0.0)
            };
            let overlap = (combined - distance) / combined;
            push = add(push, mul(direction, overlap));
        }
        push
    }
}

fn add(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x + b.x, a.y + b.y)
}

fn sub(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(a.x - b.x, a.y - b.y)
}

fn mul(v: Vec2, scale: f32) -> Vec2 {
    Vec2::new(v.x * scale, v.y * scale)
}

fn normalize(v: Vec2) -> Option<Vec2> {
    let length = v.length();
    (length > 1e-6).then(|| mul(v, 1.0 / length))
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    sub(a, b).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_5x5() -> NavGrid {
        NavGrid::new(5, 5, 1.0).unwrap()
    }

    #[test]
    fn construction_rejects_degenerate_inputs() {
        assert!(matches!(
            NavGrid::new(0, 5, 1.0),
            Err(NavError::ZeroSize { .. })
        ));
        assert!(matches!(
            NavGrid::new(5, 5, 0.0),
            Err(NavError::InvalidCellSize(_))
        ));
        assert!(matches!(
            NavGrid::new(5, 5, f32::NAN),
            Err(NavError::InvalidCellSize(_))
        ));
        assert!(matches!(
            NavGrid::from_blocked(2, 2, 1.0, Vec2::zero(), vec![false; 3]),
            Err(NavError::MaskLength { .. })
        ));
    }

    #[test]
    fn world_and_cell_coordinates_round_trip() {
        let grid = NavGrid::with_origin(4, 3, 2.0, Vec2::new(-4.0, 10.0)).unwrap();
        for y in 0..3 {
            for x in 0..4 {
                let center = grid.cell_center((x, y));
                assert_eq!(grid.world_to_cell(center), Ok((x, y)));
            }
        }
        assert_eq!(grid.cell_center((0, 0)), Vec2::new(-3.0, 11.0));
        assert!(matches!(
            grid.world_to_cell(Vec2::new(100.0, 0.0)),
            Err(NavError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn finds_shortest_orthogonal_and_diagonal_paths() {
        let grid = grid_5x5();
        assert_eq!(grid.find_path((0, 0), (3, 0), false).unwrap().len(), 4);
        assert_eq!(grid.find_path((0, 0), (3, 3), true).unwrap().len(), 4);
        assert_eq!(grid.find_path((2, 2), (2, 2), false).unwrap(), vec![(2, 2)]);
    }

    #[test]
    fn routes_around_walls_and_reports_unreachable() {
        let mut blocked = vec![false; 25];
        for y in 0..4 {
            blocked[y * 5 + 2] = true;
        }
        let grid = NavGrid::from_blocked(5, 5, 1.0, Vec2::zero(), blocked).unwrap();
        let path = grid.find_path((0, 0), (4, 0), false).unwrap();
        assert!(path.contains(&(2, 4)));
        assert_eq!(*path.first().unwrap(), (0, 0));
        assert_eq!(*path.last().unwrap(), (4, 0));

        let mut sealed = vec![false; 25];
        for y in 0..5 {
            sealed[y * 5 + 2] = true;
        }
        let sealed = NavGrid::from_blocked(5, 5, 1.0, Vec2::zero(), sealed).unwrap();
        assert!(matches!(
            sealed.find_path((0, 0), (4, 0), true),
            Err(NavError::Unreachable { .. })
        ));
    }

    #[test]
    fn diagonal_paths_never_cut_blocked_corners() {
        let mut blocked = vec![false; 25];
        blocked[5] = true; // (0,1) blocks the corner at (1,1)
        let grid = NavGrid::from_blocked(5, 5, 1.0, Vec2::zero(), blocked).unwrap();
        let path = grid.find_path((0, 0), (1, 1), true).unwrap();
        for pair in path.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a.0 != b.0 && a.1 != b.1 {
                assert!(grid.is_walkable(a.0, b.1));
                assert!(grid.is_walkable(b.0, a.1));
            }
        }
        assert!(!grid.line_of_sight((0, 0), (1, 1)));
    }

    #[test]
    fn blocks_paths_that_start_or_end_in_walls() {
        let mut blocked = vec![false; 25];
        blocked[0] = true;
        blocked[24] = true;
        let mut grid = NavGrid::from_blocked(5, 5, 1.0, Vec2::zero(), blocked).unwrap();
        assert_eq!(
            grid.find_path((0, 0), (4, 4), false),
            Err(NavError::Blocked { x: 0, y: 0 })
        );
        assert_eq!(
            grid.find_path((0, 1), (4, 4), false),
            Err(NavError::Blocked { x: 4, y: 4 })
        );
        assert_eq!(
            grid.set_blocked(9, 9, true),
            Err(NavError::OutOfBounds { x: 9, y: 9 })
        );
    }

    #[test]
    fn smoothing_removes_collinear_detour_but_keeps_wall_clearance() {
        let mut blocked = vec![false; 49];
        for y in 0..5 {
            blocked[y * 7 + 3] = true;
        }
        let grid = NavGrid::from_blocked(7, 7, 1.0, Vec2::zero(), blocked).unwrap();
        let raw = grid.find_path((0, 0), (6, 0), true).unwrap();
        let smooth = grid.smooth_path(&raw);
        assert!(smooth.len() < raw.len());
        assert_eq!(smooth.first(), Some(&(0, 0)));
        assert_eq!(smooth.last(), Some(&(6, 0)));
        for segment in smooth.windows(2) {
            assert!(grid.line_of_sight(segment[0], segment[1]));
        }
        assert!(!grid.line_of_sight((0, 0), (6, 0)));
    }

    #[test]
    fn world_path_returns_smoothed_world_waypoints() {
        let grid = NavGrid::with_origin(5, 5, 1.0, Vec2::new(0.0, 0.0)).unwrap();
        let path = grid
            .find_world_path(Vec2::new(0.5, 0.5), Vec2::new(4.5, 4.5), true)
            .unwrap();
        assert_eq!(path.points().first(), Some(&Vec2::new(0.5, 0.5)));
        assert_eq!(path.goal(), Some(Vec2::new(4.5, 4.5)));
        assert_eq!(path.cells().len(), path.points().len());
        assert!(path.len() <= 5);
    }

    #[test]
    fn agent_follows_a_path_and_arrives() {
        let mut agent = NavAgent::new(Vec2::new(0.0, 0.0), 2.0, 0.2).unwrap();
        agent.set_path(vec![
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(3.0, 0.0),
        ]);
        let mut steps = 0;
        while !agent.arrived() && steps < 100 {
            agent.update(0.1, &[]);
            steps += 1;
        }
        assert!(agent.arrived(), "agent did not arrive in {steps} steps");
        assert_eq!(agent.position(), Vec2::new(3.0, 0.0));
    }

    #[test]
    fn separation_pushes_overlapping_agents_apart() {
        let mut agent = NavAgent::new(Vec2::new(0.0, 0.0), 1.0, 0.5).unwrap();
        let other = Obstacle {
            position: Vec2::new(0.1, 0.0),
            radius: 0.5,
        };
        let before = agent.position();
        agent.update(0.1, &[other]);
        assert!(
            agent.position().x < before.x,
            "overlap should push to the left"
        );
    }

    #[test]
    fn zero_or_negative_dt_does_not_move_the_agent() {
        let mut agent = NavAgent::new(Vec2::zero(), 3.0, 0.2).unwrap();
        agent.set_path(vec![Vec2::new(5.0, 0.0)]);
        agent.update(0.0, &[]);
        agent.update(-0.5, &[]);
        assert_eq!(agent.position(), Vec2::zero());
    }

    #[test]
    fn navigation_is_bit_identical_across_runs() {
        fn run() -> Vec<f32> {
            let mut agent = NavAgent::new(Vec2::new(0.0, 0.0), 1.7, 0.3).unwrap();
            agent.set_path(vec![
                Vec2::new(3.0, 1.0),
                Vec2::new(5.0, 4.0),
                Vec2::new(9.0, 2.0),
            ]);
            let obstacles = [
                Obstacle {
                    position: Vec2::new(1.0, 0.4),
                    radius: 0.4,
                },
                Obstacle {
                    position: Vec2::new(3.2, 1.2),
                    radius: 0.5,
                },
            ];
            let mut out = Vec::new();
            for _ in 0..120 {
                let step = agent.update(1.0 / 60.0, &obstacles);
                out.push(step.position.x);
                out.push(step.position.y);
            }
            out
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn agent_rejects_non_finite_configuration() {
        assert!(matches!(
            NavAgent::new(Vec2::zero(), f32::INFINITY, 0.2),
            Err(NavError::InvalidAgent)
        ));
        assert!(matches!(
            NavAgent::new(Vec2::zero(), 1.0, -0.1),
            Err(NavError::InvalidAgent)
        ));
    }
}
