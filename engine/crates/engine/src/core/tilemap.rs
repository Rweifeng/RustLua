//! Grid-based tile maps: visual cells, a separate collision layer, and a
//! component-string format that round-trips through scenes.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::core::transform::Vec2;

/// Maximum distinct tile appearances (1..=6 map to a tileset column; 0 = empty).
pub const TILE_PALETTE: [[f32; 4]; 6] = [
    [0.18, 0.72, 1.0, 1.0],
    [0.95, 0.65, 0.15, 1.0],
    [0.35, 0.85, 0.40, 1.0],
    [0.90, 0.35, 0.45, 1.0],
    [0.70, 0.45, 0.95, 1.0],
    [0.90, 0.85, 0.35, 1.0],
];

/// Default tileset texture a tile map samples when its component string omits
/// a `texture` field. Columns `1..=6` hold the six distinct tile appearances.
pub const DEFAULT_TILESET: &str = "assets/tileset.png";

/// A rectangular grid of tiles. `tiles` holds a row-major tileset column per
/// cell (`0` = empty, `1..=6` = tileset column), and `solid` holds the
/// separate collision layer (`true` = blocks movement). The whole map samples
/// `texture`; each tile's UV selects its tileset column.
///
/// Per-cell string metadata (`meta`) is runtime-only in Phase A and is not
/// serialized into the scene component string.
#[derive(Clone, Debug, PartialEq)]
pub struct Tilemap {
    pub tile_size: Vec2,
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<u8>,
    pub solid: Vec<bool>,
    pub texture: String,
    /// Per-cell string key/value bags (e.g. `tillable`, `tilled`, `crop`).
    pub meta: BTreeMap<(usize, usize), BTreeMap<String, String>>,
}

/// Errors produced while building or parsing a tile map.
#[derive(Clone, Debug, PartialEq)]
pub enum TilemapError {
    InvalidTileSize(Vec2),
    EmptyDimensions,
    LengthMismatch { expected: usize, actual: usize },
    InvalidCellIndex(u8),
    OutOfBounds { x: usize, y: usize },
    Parse(String),
}

impl fmt::Display for TilemapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTileSize(v) => write!(f, "invalid tile size: ({}, {})", v.x, v.y),
            Self::EmptyDimensions => write!(f, "tile map dimensions must be non-zero"),
            Self::LengthMismatch { expected, actual } => {
                write!(
                    f,
                    "tile data length {actual} does not match grid {expected}"
                )
            }
            Self::InvalidCellIndex(v) => write!(f, "tile cell index {v} is out of palette range"),
            Self::OutOfBounds { x, y } => write!(f, "tile cell ({x}, {y}) is out of bounds"),
            Self::Parse(reason) => write!(f, "invalid tile map component string: {reason}"),
        }
    }
}

impl Error for TilemapError {}

/// Errors produced by [`Tilemap::find_path`].
#[derive(Clone, Debug, PartialEq)]
pub enum PathError {
    OutOfBounds {
        x: usize,
        y: usize,
    },
    Blocked {
        x: usize,
        y: usize,
    },
    Unreachable {
        from: (usize, usize),
        to: (usize, usize),
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { x, y } => write!(f, "path endpoint ({x}, {y}) is out of bounds"),
            Self::Blocked { x, y } => write!(f, "path endpoint ({x}, {y}) is solid"),
            Self::Unreachable { from, to } => {
                write!(
                    f,
                    "no path from ({}, {}) to ({}, {})",
                    from.0, from.1, to.0, to.1
                )
            }
        }
    }
}

impl Error for PathError {}

impl Tilemap {
    /// Creates a grid with the given size and tile size, all cells empty and
    /// sampling the default tileset.
    pub fn new(tile_size: Vec2, width: usize, height: usize) -> Result<Self, TilemapError> {
        validate_dimensions(tile_size, width, height)?;
        let count = width * height;
        Ok(Self {
            tile_size,
            width,
            height,
            tiles: vec![0; count],
            solid: vec![false; count],
            texture: DEFAULT_TILESET.to_owned(),
            meta: BTreeMap::new(),
        })
    }

    /// Returns the normalized UV rectangle for a tileset column (`1..=6`).
    #[must_use]
    pub fn tile_uv(column: u8) -> (Vec2, Vec2) {
        let count = TILE_PALETTE.len();
        let start = (column.saturating_sub(1) as f32 / count as f32).clamp(0.0, 1.0);
        let end = (column as f32 / count as f32).clamp(0.0, 1.0);
        (Vec2::new(start, 0.0), Vec2::new(end, 1.0))
    }

    fn index(&self, x: usize, y: usize) -> Result<usize, TilemapError> {
        if x >= self.width || y >= self.height {
            return Err(TilemapError::OutOfBounds { x, y });
        }
        Ok(y * self.width + x)
    }

    /// Returns the palette index at a cell, or `None` when out of bounds.
    pub fn cell(&self, x: usize, y: usize) -> Option<u8> {
        self.index(x, y).ok().map(|i| self.tiles[i])
    }

    /// Sets the palette index at a cell (`0` = empty, `1..=6` = palette).
    pub fn set_cell(&mut self, x: usize, y: usize, value: u8) -> Result<(), TilemapError> {
        if value > TILE_PALETTE.len() as u8 {
            return Err(TilemapError::InvalidCellIndex(value));
        }
        let i = self.index(x, y)?;
        self.tiles[i] = value;
        Ok(())
    }

    /// Returns whether a cell blocks movement, or `None` when out of bounds.
    pub fn is_solid(&self, x: usize, y: usize) -> Option<bool> {
        self.index(x, y).ok().map(|i| self.solid[i])
    }

    /// Marks a cell as solid (blocking) or not.
    pub fn set_solid(&mut self, x: usize, y: usize, solid: bool) -> Result<(), TilemapError> {
        let i = self.index(x, y)?;
        self.solid[i] = solid;
        Ok(())
    }

    /// Returns a metadata value for a cell, or `None` when missing / OOB.
    pub fn get_meta(&self, x: usize, y: usize, key: &str) -> Option<&str> {
        self.index(x, y).ok()?;
        self.meta
            .get(&(x, y))
            .and_then(|bag| bag.get(key))
            .map(String::as_str)
    }

    /// Sets a metadata value for a cell. Empty `value` removes the key.
    pub fn set_meta(
        &mut self,
        x: usize,
        y: usize,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), TilemapError> {
        let _ = self.index(x, y)?;
        let key = key.into();
        let value = value.into();
        if key.is_empty() {
            return Err(TilemapError::Parse("meta key must not be empty".to_owned()));
        }
        if value.is_empty() {
            if let Some(bag) = self.meta.get_mut(&(x, y)) {
                bag.remove(&key);
                if bag.is_empty() {
                    self.meta.remove(&(x, y));
                }
            }
            return Ok(());
        }
        self.meta.entry((x, y)).or_default().insert(key, value);
        Ok(())
    }

    /// Finds a shortest path from `start` to `goal` through non-solid cells.
    ///
    /// Returns the path including both endpoints (`[start, ..., goal]`). With
    /// `allow_diagonal`, diagonal moves are allowed but never cut a corner
    /// through a solid cell. `start` and `goal` must be in-bounds and
    /// non-solid; when no route exists, returns [`PathError::Unreachable`].
    pub fn find_path(
        &self,
        start: (usize, usize),
        goal: (usize, usize),
        allow_diagonal: bool,
    ) -> Result<Vec<(usize, usize)>, PathError> {
        use std::{cmp::Ordering, collections::BinaryHeap};

        let (sx, sy) = start;
        let (gx, gy) = goal;
        self.index(sx, sy)
            .map_err(|_| PathError::OutOfBounds { x: sx, y: sy })?;
        self.index(gx, gy)
            .map_err(|_| PathError::OutOfBounds { x: gx, y: gy })?;
        if self.solid[sy * self.width + sx] {
            return Err(PathError::Blocked { x: sx, y: sy });
        }
        if self.solid[gy * self.width + gx] {
            return Err(PathError::Blocked { x: gx, y: gy });
        }
        if start == goal {
            return Ok(vec![start]);
        }

        // Manhattan (4-directional) or Chebyshev (8-directional) distance; both
        // are admissible under a uniform per-step cost of 1.
        let heuristic = |x: usize, y: usize| -> i32 {
            let dx = (x as i32 - gx as i32).abs();
            let dy = (y as i32 - gy as i32).abs();
            if allow_diagonal { dx.max(dy) } else { dx + dy }
        };

        #[derive(Clone, Copy, Eq, PartialEq)]
        struct Node {
            f: i32,
            g: i32,
            x: usize,
            y: usize,
        }
        impl Ord for Node {
            fn cmp(&self, other: &Self) -> Ordering {
                other.f.cmp(&self.f).then_with(|| other.g.cmp(&self.g))
            }
        }
        impl PartialOrd for Node {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let start_index = sy * self.width + sx;
        let goal_index = gy * self.width + gx;
        let mut open = BinaryHeap::new();
        let mut g_score = vec![i32::MAX; self.width * self.height];
        let mut came_from: Vec<Option<usize>> = vec![None; self.width * self.height];
        g_score[start_index] = 0;
        open.push(Node {
            f: heuristic(sx, sy),
            g: 0,
            x: sx,
            y: sy,
        });

        while let Some(current) = open.pop() {
            if (current.x, current.y) == goal {
                let mut path = vec![goal];
                let mut cursor = goal_index;
                while cursor != start_index {
                    let Some(previous) = came_from[cursor] else {
                        return Err(PathError::Unreachable {
                            from: start,
                            to: goal,
                        });
                    };
                    cursor = previous;
                    path.push((cursor % self.width, cursor / self.width));
                }
                path.reverse();
                return Ok(path);
            }
            let current_index = current.y * self.width + current.x;
            if current.g > g_score[current_index] {
                continue;
            }
            for (nx, ny) in self.walkable_neighbors(current.x, current.y, allow_diagonal) {
                let neighbor_index = ny * self.width + nx;
                let tentative = current.g + 1;
                if tentative < g_score[neighbor_index] {
                    g_score[neighbor_index] = tentative;
                    came_from[neighbor_index] = Some(current_index);
                    open.push(Node {
                        f: tentative + heuristic(nx, ny),
                        g: tentative,
                        x: nx,
                        y: ny,
                    });
                }
            }
        }

        Err(PathError::Unreachable {
            from: start,
            to: goal,
        })
    }

    /// Returns the in-bounds, non-solid neighbors of a cell for pathfinding.
    /// Diagonal moves require both adjacent orthogonal cells to be non-solid so
    /// a path never cuts a corner.
    fn walkable_neighbors(&self, x: usize, y: usize, allow_diagonal: bool) -> Vec<(usize, usize)> {
        let west = x
            .checked_sub(1)
            .and_then(|nx| self.is_walkable(nx, y).then_some(nx));
        let east = self.is_walkable(x + 1, y).then_some(x + 1);
        let north = y
            .checked_sub(1)
            .and_then(|ny| self.is_walkable(x, ny).then_some(ny));
        let south = self.is_walkable(x, y + 1).then_some(y + 1);

        let mut result = Vec::with_capacity(8);
        if let Some(nx) = west {
            result.push((nx, y));
        }
        if let Some(nx) = east {
            result.push((nx, y));
        }
        if let Some(ny) = north {
            result.push((x, ny));
        }
        if let Some(ny) = south {
            result.push((x, ny));
        }
        if allow_diagonal {
            if let (Some(nx), Some(ny)) = (west, north)
                && self.is_walkable(nx, ny)
            {
                result.push((nx, ny));
            }
            if let (Some(nx), Some(ny)) = (east, north)
                && self.is_walkable(nx, ny)
            {
                result.push((nx, ny));
            }
            if let (Some(nx), Some(ny)) = (west, south)
                && self.is_walkable(nx, ny)
            {
                result.push((nx, ny));
            }
            if let (Some(nx), Some(ny)) = (east, south)
                && self.is_walkable(nx, ny)
            {
                result.push((nx, ny));
            }
        }
        result
    }

    /// A cell is walkable when it is in bounds and non-solid.
    fn is_walkable(&self, x: usize, y: usize) -> bool {
        self.index(x, y).is_ok() && !self.solid[y * self.width + x]
    }

    /// Serializes the map into a scene `Tilemap` component string:
    /// `tile_size=<x>x<y>;width=..;height=..;texture=<path>;tiles=..;solid=..`
    /// (`tiles` uses `0`-`6`, `solid` uses `0`/`1`, row-major).
    pub fn to_component_str(&self) -> String {
        let tiles: String = self
            .tiles
            .iter()
            .map(|cell| char::from(b'0' + cell))
            .collect();
        let solid: String = self
            .solid
            .iter()
            .map(|value| if *value { '1' } else { '0' })
            .collect();
        format!(
            "tile_size={}x{};width={};height={};texture={};tiles={};solid={}",
            self.tile_size.x, self.tile_size.y, self.width, self.height, self.texture, tiles, solid
        )
    }

    /// Parses a scene `Tilemap` component string back into a map. A missing
    /// `texture` field falls back to [`DEFAULT_TILESET`].
    pub fn from_component_str(value: &str) -> Result<Self, TilemapError> {
        let mut tile_size = None;
        let mut width = None;
        let mut height = None;
        let mut texture = None;
        let mut tiles = None;
        let mut solid = None;
        for field in value.split(';') {
            let Some((key, raw)) = field.split_once('=') else {
                return Err(TilemapError::Parse(format!("malformed field: {field}")));
            };
            match key {
                "tile_size" => {
                    let (x, y) = raw.split_once('x').ok_or_else(|| {
                        TilemapError::Parse("tile_size must be <x>x<y>".to_owned())
                    })?;
                    let x: f32 = x
                        .parse()
                        .map_err(|_| TilemapError::Parse("tile_size x not a number".to_owned()))?;
                    let y: f32 = y
                        .parse()
                        .map_err(|_| TilemapError::Parse("tile_size y not a number".to_owned()))?;
                    tile_size = Some(Vec2::new(x, y));
                }
                "width" => {
                    width = Some(
                        raw.parse()
                            .map_err(|_| TilemapError::Parse("width not an integer".to_owned()))?,
                    );
                }
                "height" => {
                    height =
                        Some(raw.parse().map_err(|_| {
                            TilemapError::Parse("height not an integer".to_owned())
                        })?);
                }
                "texture" => {
                    if raw.is_empty() {
                        return Err(TilemapError::Parse("texture must not be empty".to_owned()));
                    }
                    texture = Some(raw.to_owned());
                }
                "tiles" => {
                    tiles = Some(
                        raw.chars()
                            .map(|c| {
                                c.to_digit(10).map(|v| v as u8).ok_or_else(|| {
                                    TilemapError::Parse("tiles contains a non-digit".to_owned())
                                })
                            })
                            .collect::<Result<Vec<u8>, _>>()?,
                    );
                }
                "solid" => {
                    solid = Some(
                        raw.chars()
                            .map(|c| match c {
                                '0' => Ok(false),
                                '1' => Ok(true),
                                _ => Err(TilemapError::Parse(
                                    "solid contains a non-0/1 character".to_owned(),
                                )),
                            })
                            .collect::<Result<Vec<bool>, _>>()?,
                    );
                }
                _ => return Err(TilemapError::Parse(format!("unknown field: {key}"))),
            }
        }
        let tile_size =
            tile_size.ok_or_else(|| TilemapError::Parse("missing tile_size".to_owned()))?;
        let width = width.ok_or_else(|| TilemapError::Parse("missing width".to_owned()))?;
        let height = height.ok_or_else(|| TilemapError::Parse("missing height".to_owned()))?;
        let tiles = tiles.ok_or_else(|| TilemapError::Parse("missing tiles".to_owned()))?;
        let solid = solid.ok_or_else(|| TilemapError::Parse("missing solid".to_owned()))?;
        let mut map = Tilemap::new(tile_size, width, height)?;
        if tiles.len() != map.tiles.len() {
            return Err(TilemapError::LengthMismatch {
                expected: map.tiles.len(),
                actual: tiles.len(),
            });
        }
        if solid.len() != map.solid.len() {
            return Err(TilemapError::LengthMismatch {
                expected: map.solid.len(),
                actual: solid.len(),
            });
        }
        for (i, cell) in tiles.into_iter().enumerate() {
            if cell > TILE_PALETTE.len() as u8 {
                return Err(TilemapError::InvalidCellIndex(cell));
            }
            map.tiles[i] = cell;
        }
        for (i, value) in solid.into_iter().enumerate() {
            map.solid[i] = value;
        }
        map.texture = texture.unwrap_or_else(|| DEFAULT_TILESET.to_owned());
        Ok(map)
    }
}

fn validate_dimensions(tile_size: Vec2, width: usize, height: usize) -> Result<(), TilemapError> {
    if !tile_size.x.is_finite()
        || !tile_size.y.is_finite()
        || tile_size.x <= 0.0
        || tile_size.y <= 0.0
    {
        return Err(TilemapError::InvalidTileSize(tile_size));
    }
    if width == 0 || height == 0 {
        return Err(TilemapError::EmptyDimensions);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tilemap {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 3).unwrap();
        map.set_cell(1, 1, 2).unwrap();
        map.set_solid(1, 1, true).unwrap();
        map.set_cell(3, 2, 1).unwrap();
        map
    }

    #[test]
    fn round_trips_through_component_string() {
        let mut map = sample();
        map.texture = "assets/custom-tileset.png".to_owned();
        let text = map.to_component_str();
        assert!(text.contains("texture=assets/custom-tileset.png"));
        assert_eq!(Tilemap::from_component_str(&text).unwrap(), map);
    }

    #[test]
    fn missing_texture_falls_back_to_default_tileset() {
        let map =
            Tilemap::from_component_str("tile_size=32x32;width=2;height=2;tiles=0000;solid=0000")
                .unwrap();
        assert_eq!(map.texture, DEFAULT_TILESET);
    }

    #[test]
    fn tile_uv_selects_tileset_columns() {
        let (min, max) = Tilemap::tile_uv(1);
        assert_eq!(min.x, 0.0);
        assert_eq!(max.x, 1.0 / 6.0);
        let (min, max) = Tilemap::tile_uv(6);
        assert_eq!(min.x, 5.0 / 6.0);
        assert_eq!(max.x, 1.0);
        let (min, max) = Tilemap::tile_uv(0);
        assert_eq!(min.x, 0.0);
        assert_eq!(max.x, 0.0);
    }

    #[test]
    fn accessors_report_bounds() {
        let map = sample();
        assert_eq!(map.cell(1, 1), Some(2));
        assert_eq!(map.is_solid(1, 1), Some(true));
        assert_eq!(map.cell(0, 0), Some(0));
        assert_eq!(map.is_solid(0, 0), Some(false));
        assert_eq!(map.cell(4, 0), None);
        assert_eq!(map.is_solid(0, 3), None);
    }

    #[test]
    fn rejects_invalid_cell_index_and_dimensions() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        assert!(matches!(
            map.set_cell(0, 0, 9),
            Err(TilemapError::InvalidCellIndex(9))
        ));
        assert!(Tilemap::new(Vec2::new(0.0, 1.0), 2, 2).is_err());
        assert!(Tilemap::new(Vec2::new(32.0, 32.0), 0, 2).is_err());
        assert!(map.set_cell(2, 0, 1).is_err());
    }

    #[test]
    fn rejects_malformed_component_strings() {
        assert!(Tilemap::from_component_str("width=2;height=2").is_err());
        assert!(
            Tilemap::from_component_str("tile_size=32x32;width=2;height=2;tiles=0000;solid=0000x")
                .is_err()
        );
        assert!(
            Tilemap::from_component_str("tile_size=32x32;width=2;height=2;tiles=0;solid=0000")
                .is_err()
        );
    }

    #[test]
    fn meta_round_trips_per_cell_and_clears_empty() {
        let mut map = sample();
        map.set_meta(1, 1, "tillable", "true").unwrap();
        map.set_meta(1, 1, "crop", "tomato").unwrap();
        assert_eq!(map.get_meta(1, 1, "tillable"), Some("true"));
        assert_eq!(map.get_meta(1, 1, "crop"), Some("tomato"));
        assert_eq!(map.get_meta(0, 0, "tillable"), None);
        map.set_meta(1, 1, "crop", "").unwrap();
        assert_eq!(map.get_meta(1, 1, "crop"), None);
        assert_eq!(map.get_meta(1, 1, "tillable"), Some("true"));
        map.set_meta(1, 1, "tillable", "").unwrap();
        assert!(map.meta.is_empty());
        assert!(map.set_meta(9, 9, "x", "y").is_err());
    }

    #[test]
    fn find_path_returns_shortest_orthogonal_and_diagonal() {
        let map = Tilemap::new(Vec2::new(32.0, 32.0), 3, 3).unwrap();
        // Orthogonal: 3 right/down moves -> 5 cells.
        assert_eq!(map.find_path((0, 0), (2, 2), false).unwrap().len(), 5);
        // Diagonal: straight line -> 3 cells.
        assert_eq!(map.find_path((0, 0), (2, 2), true).unwrap().len(), 3);
        // Start == goal -> single cell.
        assert_eq!(map.find_path((1, 1), (1, 1), false).unwrap(), vec![(1, 1)]);
    }

    #[test]
    fn find_path_navigates_around_solid_cells() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 3).unwrap();
        map.set_solid(1, 0, true).unwrap();
        map.set_solid(2, 0, true).unwrap();
        let path = map.find_path((0, 0), (3, 0), false).unwrap();
        assert_eq!(path.first(), Some(&(0, 0)));
        assert_eq!(path.last(), Some(&(3, 0)));
        // Detour via y=1 is the shortest route (6 cells).
        assert_eq!(path.len(), 6);
        for &(x, y) in &path {
            assert!(!map.is_solid(x, y).unwrap());
        }
        assert!(!path.contains(&(1, 0)));
        assert!(!path.contains(&(2, 0)));
    }

    #[test]
    fn find_path_blocks_diagonal_corner_cutting() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        map.set_solid(1, 0, true).unwrap();
        map.set_solid(0, 1, true).unwrap();
        assert_eq!(
            map.find_path((0, 0), (1, 1), true),
            Err(PathError::Unreachable {
                from: (0, 0),
                to: (1, 1)
            })
        );
    }

    #[test]
    fn find_path_rejects_solid_endpoints_and_out_of_bounds() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        map.set_solid(0, 0, true).unwrap();
        assert_eq!(
            map.find_path((0, 0), (1, 1), false),
            Err(PathError::Blocked { x: 0, y: 0 })
        );
        assert_eq!(
            map.find_path((0, 0), (9, 9), false),
            Err(PathError::OutOfBounds { x: 9, y: 9 })
        );
    }
}
