//! Grid-based tile maps: visual cells, a separate collision layer, and a
//! component-string format that round-trips through scenes.

use std::collections::BTreeMap;

use crate::transform::Vec2;

/// Debug palette for tile maps **without** a tileset texture: entry `i` is the
/// color of atlas index `i + 1`. It no longer defines the atlas geometry —
/// `Tilemap::atlas_cols`/`atlas_rows` do. Kept at 6 entries because that is what
/// every legacy 6x1 tileset has.
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

/// Legacy scene encoding: one ASCII digit per cell, single-column addressing.
pub const TILES_FORMAT_LEGACY: u8 = 0;
/// Current scene encoding: comma-separated decimal atlas indices.
pub const TILES_FORMAT_ATLAS: u8 = 1;
/// Atlas columns assumed when a legacy component string omits `atlas_cols`.
pub const DEFAULT_ATLAS_COLS: usize = 6;
/// Atlas rows assumed when a legacy component string omits `atlas_rows`.
pub const DEFAULT_ATLAS_ROWS: usize = 1;

/// 4-bit autotile mask: how many atlas entries one kind reserves for neighbor
/// transitions (the four index-adjacent cells present-or-absent, all conditions
/// independent). Matches `Tilemap::autotile_slot`.
pub const AUTOTILE_SLOTS: usize = 16;

/// FNV-1a over the cell coordinates and the layer seed. Deliberately **not**
/// `DefaultHasher`: that one is seeded per-process, which would reshuffle tile
/// variants every run and redden every pixel-comparison gate.
pub const VARIANT_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a prime paired with [`VARIANT_HASH_OFFSET`].
pub const VARIANT_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Neighbors that make up the autotile mask bits (see [`Tilemap::autotile_mask`]).
///
/// Named **index-relative**, not by compass direction: tile grids store row 0 at
/// the *bottom* of the map while the tileset atlas stores row 0 at the *top*, so
/// any "north/south" wording would be ambiguous about which of the two it means.
/// The art contract lives here: bit 0 = `y - 1`, bit 1 = `x + 1`,
/// bit 2 = `y + 1`, bit 3 = `x - 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileNeighbor {
    /// Cell at `y - 1`.
    PrevRow,
    /// Cell at `x + 1`.
    NextColumn,
    /// Cell at `y + 1`.
    NextRow,
    /// Cell at `x - 1`.
    PrevColumn,
}

impl TileNeighbor {
    /// Bit position of this neighbor inside the 4-bit mask.
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::PrevRow => 0,
            Self::NextColumn => 1,
            Self::NextRow => 2,
            Self::PrevColumn => 3,
        }
    }
}

/// Salt so the x and y coordinates cannot cancel each other inside the variant
/// hash (`(x, y) = (1, 2)` must not equal `(2, 1)`).
const MIX_AXIS_X: u64 = 0x9e37_79b9_7f4a_7c15;
/// Paired salt for the y coordinate.
const MIX_AXIS_Y: u64 = 0xbf58_476d_1ce4_e5b9;

/// One FNV-1a-style mixing step.
const fn mix_u64(state: u64, value: u64) -> u64 {
    let bytes = value.to_le_bytes();
    let mut mixed = state;
    let mut index = 0;
    while index < bytes.len() {
        mixed = (mixed ^ bytes[index] as u64).wrapping_mul(VARIANT_HASH_PRIME);
        index += 1;
    }
    mixed
}

/// A rectangular grid of tiles.
///
/// `tiles` holds a row-major **tile id** per cell (`0` = empty). By default that
/// id is used directly as the atlas index (`1` = first atlas entry, row-major
/// over `atlas_cols` x `atlas_rows`), which reproduces the legacy column
/// addressing exactly. When a layer opts into autotile or variants, the id
/// becomes a **logical kind** (grass/dirt/stone/water) and
/// [`Tilemap::resolve_atlas_index`] maps it to the atlas entry to sample — so
/// `t_e_cell` never has to change meaning for data that predates M72-02.
///
/// `solid` is the separate collision layer (`true` = blocks movement); it is
/// independent of tile identity, so pathfinding ignores everything above.
///
/// Per-cell string metadata (`meta`) is runtime-only and is not serialized into
/// the scene component string. Per-cell **tint** (`tint_ids`) *is* serialized —
/// shading and moisture tint must survive a save/reload round trip.
#[derive(Clone, Debug, PartialEq)]
pub struct Tilemap {
    pub tile_size: Vec2,
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<u8>,
    pub solid: Vec<bool>,
    pub texture: String,
    /// Atlas columns in `texture`. Together with [`Self::atlas_rows`] this is
    /// the addressing geometry: entry `i` covers
    /// `col = i % cols`, `row = i / cols` of the tileset.
    pub atlas_cols: usize,
    /// Atlas rows in `texture`.
    pub atlas_rows: usize,
    /// Which [`Self::tiles`] encoding this map was saved with. Written as
    /// `tiles_format` so a reader never has to guess.
    pub tiles_format: u8,
    /// Distinct logical tile kinds, used when autotile is on: kind `k` lives at
    /// `tiles == k` and owns `k`-th blocks of [`Self::variant_count`] entries
    /// and (if [`Self::autotile`] is on) [`AUTOTILE_SLOTS`] transition entries.
    pub tile_kinds: usize,
    /// Same-kind appearance variants per kind (`1` = none). The renderer picks
    /// `hash(x, y, seed) % variant_count`, so two adjacent cells of the same
    /// kind differ without any per-cell authoring.
    pub variant_count: usize,
    /// Whether the 4-bit neighbor mask selects the atlas entry (see
    /// [`Self::resolve_atlas_index`]).
    pub autotile: bool,
    /// Seed for the variant hash. Editor and runtime must read the same value,
    /// so it is part of the component string.
    pub variant_seed: u32,
    /// Per-cell index into [`Self::tint_table`] (`0` = untinted). Sparse: most
    /// maps have zero tinted cells.
    pub tint_ids: Vec<u8>,
    /// Tint colors, one-based (`tint_ids == 1` selects `tint_table[0]`). The
    /// renderer multiplies this into the tile's vertex color.
    pub tint_table: Vec<[f32; 4]>,
    /// Per-cell string key/value bags (e.g. `tillable`, `tilled`, `crop`).
    pub meta: BTreeMap<(usize, usize), BTreeMap<String, String>>,
}

/// Errors produced while building or parsing a tile map.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TilemapError {
    #[error("invalid tile size: ({}, {})", .0.x, .0.y)]
    InvalidTileSize(Vec2),
    #[error("tile map dimensions must be non-zero")]
    EmptyDimensions,
    #[error("tile data length {actual} does not match grid {expected}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("tile cell index {0} is out of atlas range")]
    InvalidCellIndex(u8),
    #[error("tile cell ({x}, {y}) is out of bounds")]
    OutOfBounds { x: usize, y: usize },
    #[error("invalid tile map component string: {0}")]
    Parse(String),
    #[error("tile map must have a non-empty atlas grid (cols x rows)")]
    EmptyAtlas,
    #[error("tiles encoding format {0} is not supported")]
    UnsupportedTilesFormat(u8),
    #[error(
        "atlas layout needs {needed} entries but the tileset has only {capacity} ({cols}x{rows})"
    )]
    AtlasTooSmall {
        needed: usize,
        capacity: usize,
        cols: usize,
        rows: usize,
    },
    #[error("tint id {tint_id} has no entry in the tint table ({size} colors)")]
    UnknownTintId { tint_id: u8, size: usize },
}

/// Errors produced by [`Tilemap::find_path`].
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum PathError {
    #[error("path endpoint ({x}, {y}) is out of bounds")]
    OutOfBounds { x: usize, y: usize },
    #[error("path endpoint ({x}, {y}) is solid")]
    Blocked { x: usize, y: usize },
    #[error("no path from ({}, {}) to ({}, {})", .from.0, .from.1, .to.0, .to.1)]
    Unreachable {
        from: (usize, usize),
        to: (usize, usize),
    },
}

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
            atlas_cols: DEFAULT_ATLAS_COLS,
            atlas_rows: DEFAULT_ATLAS_ROWS,
            tiles_format: TILES_FORMAT_ATLAS,
            tile_kinds: DEFAULT_ATLAS_COLS,
            variant_count: 1,
            autotile: false,
            variant_seed: 0,
            tint_ids: vec![0; count],
            tint_table: Vec::new(),
            meta: BTreeMap::new(),
        })
    }

    /// Returns the normalized UV rectangle for a tileset column (`1..=6`).
    ///
    /// Legacy form of [`Tilemap::atlas_uv`] for a `cols x 1` strip: the column
    /// occupies `U = (i-1)/cols .. i/cols` and **all of V**. Retained because a
    /// handful of tools and tests reason in columns, and because it documents
    /// what the old addressing was.
    #[must_use]
    pub fn tile_uv(column: u8) -> (Vec2, Vec2) {
        let count = TILE_PALETTE.len();
        let start = (column.saturating_sub(1) as f32 / count as f32).clamp(0.0, 1.0);
        let end = (column as f32 / count as f32).clamp(0.0, 1.0);
        (Vec2::new(start, 0.0), Vec2::new(end, 1.0))
    }

    /// Number of addressable atlas entries (`atlas_cols * atlas_rows`).
    #[must_use]
    pub fn atlas_capacity(&self) -> usize {
        self.atlas_cols.saturating_mul(self.atlas_rows)
    }

    /// Normalized UV rectangle of atlas **entry** `index` (1-based, row-major
    /// over `atlas_cols` x `atlas_rows`). `index == 0` (empty cell) yields a
    /// degenerate rectangle at the origin — callers must skip empty cells
    /// before asking for UVs, and the renderer does.
    #[must_use]
    pub fn atlas_uv(&self, index: u8) -> (Vec2, Vec2) {
        let cols = self.atlas_cols.max(1) as f32;
        let rows = self.atlas_rows.max(1) as f32;
        let entry = index.saturating_sub(1) as f32;
        let col = entry % cols;
        let row = (entry / cols).floor();
        (
            Vec2::new(col / cols, row / rows),
            Vec2::new((col + 1.0) / cols, (row + 1.0) / rows),
        )
    }

    /// Deterministic variant index for a cell: FNV-1a over `(x, y, seed)`.
    ///
    /// Same inputs always return the same value, in-process and across runs —
    /// that is what makes variants safe to bake into scene data and pixel
    /// comparisons. Returns `0` when there is nothing to vary.
    #[must_use]
    pub fn variant_for(&self, x: usize, y: usize) -> usize {
        if self.variant_count < 2 {
            return 0;
        }
        let mut hash = VARIANT_HASH_OFFSET;
        for value in [
            x as u64,
            y as u64,
            u64::from(self.variant_seed),
            MIX_AXIS_X,
            MIX_AXIS_Y,
        ] {
            hash = mix_u64(hash, value);
        }
        (hash % self.variant_count as u64) as usize
    }

    /// 4-bit autotile mask of a cell: which of the four index-adjacent cells
    /// carry the **same** tile id. Out of bounds counts as absent, so a map
    /// edge reads as an exposed border rather than an interior tile.
    ///
    /// Bit layout is the art contract — the tileset generator and
    /// `tools/preview_scene.py` must agree with it, or borders land on the
    /// wrong variant. See [`TileNeighbor`].
    #[must_use]
    pub fn autotile_mask(&self, x: usize, y: usize) -> u8 {
        let same = |nx: isize, ny: isize| -> bool {
            let value = self
                .cell(nx as usize, ny as usize)
                .filter(|&neighbor| neighbor != 0);
            value == Some(self.cell(x, y).unwrap_or(0))
        };
        let prev_row = same(x as isize, y as isize - 1);
        let next_column = same(x as isize + 1, y as isize);
        let next_row = same(x as isize, y as isize + 1);
        let prev_column = same(x as isize - 1, y as isize);
        (prev_row as u8) << TileNeighbor::PrevRow.bit()
            | (next_column as u8) << TileNeighbor::NextColumn.bit()
            | (next_row as u8) << TileNeighbor::NextRow.bit()
            | (prev_column as u8) << TileNeighbor::PrevColumn.bit()
    }

    /// Which autotile block a cell lands in, given the layout contract in
    /// [`Tilemap::resolve_atlas_index`].
    #[must_use]
    pub fn autotile_slot(mask: u8) -> usize {
        (mask as usize) % AUTOTILE_SLOTS
    }

    /// Resolves a stored tile id to the atlas **entry** to sample.
    ///
    /// Layered semantics (M72-01 addressing vs M72-02 resolution stay separate
    /// concerns; this function is the only place they meet):
    ///
    /// - Default configuration (no autotile, `variant_count == 1`) is the
    ///   identity: the stored id **is** the atlas entry. Legacy maps and maps
    ///   authored without autotile therefore address exactly as before.
    /// - With `variant_count > 1`, id `k` owns a contiguous block of
    ///   `variant_count` entries starting at `1 + (k-1) * variant_count`; the
    ///   entry within the block is the deterministic variant of the cell.
    /// - With `autotile` on, each kind additionally owns [`AUTOTILE_SLOTS`]
    ///   transition entries after all variant blocks, selected by
    ///   [`Tilemap::autotile_mask`].
    ///
    /// `0` (empty) resolves to `0`. Ids beyond the layout resolve to `0` too —
    /// an unaddressable tile must not draw a neighbour's art.
    #[must_use]
    pub fn resolve_atlas_index(&self, x: usize, y: usize) -> u8 {
        let id = self.cell(x, y).unwrap_or(0);
        if id == 0 {
            return 0;
        }
        if !self.autotile && self.variant_count <= 1 {
            return id;
        }
        let kind = id as usize;
        if kind > self.tile_kinds {
            return 0;
        }
        let variant_span = self.variant_count.max(1);
        // Every kind owns `variant_span` base entries before any transition block
        // starts, so the autotile region is entered at a kind-independent offset.
        let base_span = self.tile_kinds * variant_span;
        if self.autotile {
            let block = Self::autotile_slot(self.autotile_mask(x, y));
            let index = 1 + base_span + (kind - 1) * AUTOTILE_SLOTS + block;
            if index <= self.atlas_capacity() {
                index as u8
            } else {
                0
            }
        } else {
            let index = 1 + (kind - 1) * variant_span + self.variant_for(x, y);
            if index <= self.atlas_capacity() {
                index as u8
            } else {
                0
            }
        }
    }

    /// Tint color of a cell, or white when untinted / unknown id.
    #[must_use]
    pub fn tint_at(&self, x: usize, y: usize) -> [f32; 4] {
        let id = self
            .tint_ids
            .get(y * self.width + x)
            .copied()
            .unwrap_or_default();
        if id == 0 {
            return [1.0; 4];
        }
        self.tint_table
            .get(id as usize - 1)
            .copied()
            .unwrap_or([1.0; 4])
    }

    /// Registers a tint color (idempotent: an equal color reuses the id) and
    /// returns its 1-based tint id.
    pub fn register_tint(&mut self, color: [f32; 4]) -> Result<u8, TilemapError> {
        if let Some(position) = self.tint_table.iter().position(|entry| *entry == color) {
            return self.tint_id_from_position(position);
        }
        let position = self.tint_table.len();
        if position >= u8::MAX as usize {
            return Err(TilemapError::Parse(format!(
                "tint table is full ({position} colors)"
            )));
        }
        self.tint_table.push(color);
        self.tint_id_from_position(position)
    }

    fn tint_id_from_position(&self, position: usize) -> Result<u8, TilemapError> {
        u8::try_from(position + 1)
            .map_err(|_| TilemapError::Parse("tint id out of range".to_owned()))
    }

    /// Sets a cell's tint id (`0` = untinted). Unknown non-zero ids are rejected
    /// so a scene cannot reference a tint color its table does not carry.
    pub fn set_tint(&mut self, x: usize, y: usize, tint_id: u8) -> Result<(), TilemapError> {
        let i = self.index(x, y)?;
        if tint_id > self.tint_table.len() as u8 {
            return Err(TilemapError::UnknownTintId {
                tint_id,
                size: self.tint_table.len(),
            });
        }
        self.tint_ids[i] = tint_id;
        Ok(())
    }

    /// Highest tile id this map can address given its atlas layout.
    #[must_use]
    pub fn max_tile_id(&self) -> usize {
        if self.autotile || self.variant_count > 1 {
            self.tile_kinds.max(1)
        } else {
            self.atlas_capacity()
        }
    }

    /// Layout self-consistency: autotile/variant blocks must fit the atlas.
    pub fn validate_layout(&self) -> Result<(), TilemapError> {
        if self.atlas_cols == 0 || self.atlas_rows == 0 {
            return Err(TilemapError::EmptyAtlas);
        }
        if self.variant_count == 0 {
            return Err(TilemapError::Parse(
                "variant_count must be at least 1".to_owned(),
            ));
        }
        let kinds = if self.autotile || self.variant_count > 1 {
            self.tile_kinds
        } else {
            // Identity addressing only needs the atlas to be addressable.
            0
        };
        let needed = kinds * (self.variant_count.max(1) + AUTOTILE_SLOTS * self.autotile as usize);
        if needed > self.atlas_capacity() {
            return Err(TilemapError::AtlasTooSmall {
                needed,
                capacity: self.atlas_capacity(),
                cols: self.atlas_cols,
                rows: self.atlas_rows,
            });
        }
        let max_id = self.max_tile_id();
        if let Some(position) = self.tiles.iter().position(|id| *id as usize > max_id) {
            return Err(TilemapError::InvalidCellIndex(self.tiles[position]));
        }
        Ok(())
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

    /// Sets the tile id at a cell (`0` = empty, otherwise up to
    /// [`Tilemap::max_tile_id`]).
    pub fn set_cell(&mut self, x: usize, y: usize, value: u8) -> Result<(), TilemapError> {
        if value as usize > self.max_tile_id() {
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
    /// `tile_size=<x>x<y>;width=..;height=..;texture=..;tiles_format=1;
    /// atlas_cols=..;atlas_rows=..;tiles=<comma separated ids>;solid=<0/1 string>`
    /// plus, only when in use, `tile_kinds`/`variant_count`/`autotile`/
    /// `variant_seed`/`tints`/`tints_rgb`.
    ///
    /// `tiles` switched from one ASCII digit per cell to comma-separated decimal
    /// ids (`tiles_format=1`) because a multi-row atlas cannot be addressed with
    /// single digits — the old encoding could not express index `10`.
    /// `solid` stays a `0`/`1` character run; it is one bit per cell.
    pub fn to_component_str(&self) -> String {
        let tiles = self
            .tiles
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let solid: String = self
            .solid
            .iter()
            .map(|value| if *value { '1' } else { '0' })
            .collect();
        let mut out = format!(
            "tile_size={}x{};width={};height={};texture={};tiles_format={};atlas_cols={};atlas_rows={};tiles={};solid={}",
            self.tile_size.x,
            self.tile_size.y,
            self.width,
            self.height,
            self.texture,
            TILES_FORMAT_ATLAS,
            self.atlas_cols,
            self.atlas_rows,
            tiles,
            solid
        );
        if self.autotile || self.variant_count > 1 {
            out.push_str(&format!(
                ";tile_kinds={};variant_count={}",
                self.tile_kinds, self.variant_count
            ));
        }
        if self.autotile {
            out.push_str(";autotile=1");
        }
        if self.variant_seed != 0 {
            out.push_str(&format!(";variant_seed={}", self.variant_seed));
        }
        // Shading is *sparse*: a 40x23 map with four shadowed columns would write
        // 920 digits for 16 tinted cells, and a digit run caps tint ids at 9.
        // `tints=<cellIndex>:<tintId>,...` carries only what differs from white.
        let tinted: Vec<String> = self
            .tint_ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id != 0)
            .map(|(index, id)| format!("{index}:{id}"))
            .collect();
        if !tinted.is_empty() {
            out.push_str(&format!(";tints={}", tinted.join(",")));
        }
        if !self.tint_table.is_empty() {
            let table = self
                .tint_table
                .iter()
                .map(|c| format!("{},{},{},{}", c[0], c[1], c[2], c[3]))
                .collect::<Vec<_>>()
                .join("|");
            out.push_str(&format!(";tints_rgb={table}"));
        }
        out
    }

    /// Parses a scene `Tilemap` component string back into a map. A missing
    /// `texture` field falls back to [`DEFAULT_TILESET`]; a missing
    /// `tiles_format` selects the legacy one-digit-per-cell encoding.
    pub fn from_component_str(value: &str) -> Result<Self, TilemapError> {
        let mut tile_size = None;
        let mut width = None;
        let mut height = None;
        let mut texture = None;
        let mut tiles = None;
        let mut solid = None;
        let mut atlas_cols = DEFAULT_ATLAS_COLS;
        let mut atlas_rows = DEFAULT_ATLAS_ROWS;
        let mut tiles_format = TILES_FORMAT_LEGACY;
        let mut tile_kinds = 0usize;
        let mut variant_count = 1usize;
        let mut autotile = false;
        let mut variant_seed = 0u32;
        let mut tints = None;
        let mut tint_table: Vec<[f32; 4]> = Vec::new();
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
                "tiles_format" => {
                    let format: u8 = raw.parse().map_err(|_| {
                        TilemapError::Parse("tiles_format not an integer".to_owned())
                    })?;
                    if format > TILES_FORMAT_ATLAS {
                        return Err(TilemapError::UnsupportedTilesFormat(format));
                    }
                    tiles_format = format;
                }
                "tiles" => {
                    tiles = Some(if tiles_format == TILES_FORMAT_LEGACY {
                        raw.chars()
                            .map(|c| {
                                c.to_digit(10).map(|v| v as u8).ok_or_else(|| {
                                    TilemapError::Parse("tiles contains a non-digit".to_owned())
                                })
                            })
                            .collect::<Result<Vec<u8>, _>>()?
                    } else {
                        raw.split(',')
                            .map(|part| {
                                part.trim().parse::<u8>().map_err(|_| {
                                    TilemapError::Parse(
                                        "tiles contains a non-numeric entry".to_owned(),
                                    )
                                })
                            })
                            .collect::<Result<Vec<u8>, _>>()?
                    });
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
                "atlas_cols" | "atlas_rows" => {
                    let target = if key == "atlas_cols" {
                        &mut atlas_cols
                    } else {
                        &mut atlas_rows
                    };
                    *target = raw
                        .parse()
                        .map_err(|_| TilemapError::Parse(format!("{key} not an integer")))?;
                }
                "tile_kinds" | "variant_count" => {
                    let target = if key == "tile_kinds" {
                        &mut tile_kinds
                    } else {
                        &mut variant_count
                    };
                    *target = raw
                        .parse()
                        .map_err(|_| TilemapError::Parse(format!("{key} not an integer")))?;
                }
                "autotile" => {
                    autotile = raw == "1" || raw == "true";
                }
                "variant_seed" => {
                    variant_seed = raw.parse().map_err(|_| {
                        TilemapError::Parse("variant_seed not an integer".to_owned())
                    })?;
                }
                "tints" => {
                    let mut pairs: Vec<(usize, u8)> = Vec::new();
                    for entry in raw.split(',') {
                        if entry.is_empty() {
                            continue;
                        }
                        let (index, id) = entry.split_once(':').ok_or_else(|| {
                            TilemapError::Parse(
                                "tints entries must be <index>:<tint_id>".to_owned(),
                            )
                        })?;
                        let index: usize = index.parse().map_err(|_| {
                            TilemapError::Parse("tints cell index not an integer".to_owned())
                        })?;
                        let id: u8 = id.parse().map_err(|_| {
                            TilemapError::Parse("tints id not an integer".to_owned())
                        })?;
                        pairs.push((index, id));
                    }
                    tints = Some(pairs);
                }
                "tints_rgb" => {
                    for entry in raw.split('|') {
                        let parts: Vec<f32> = entry
                            .split(',')
                            .map(str::trim)
                            .filter(|part| !part.is_empty())
                            .map(|part| {
                                part.parse::<f32>().map_err(|_| {
                                    TilemapError::Parse("tints_rgb not a number".to_owned())
                                })
                            })
                            .collect::<Result<Vec<f32>, _>>()?;
                        if parts.len() != 4 {
                            return Err(TilemapError::Parse(
                                "tints_rgb entries must be r,g,b,a".to_owned(),
                            ));
                        }
                        tint_table.push([parts[0], parts[1], parts[2], parts[3]]);
                    }
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
        if atlas_cols == 0 || atlas_rows == 0 {
            return Err(TilemapError::EmptyAtlas);
        }
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
        map.texture = texture.unwrap_or_else(|| DEFAULT_TILESET.to_owned());
        map.atlas_cols = atlas_cols;
        map.atlas_rows = atlas_rows;
        map.tiles_format = tiles_format;
        map.tile_kinds = if tile_kinds == 0 {
            // A string without `tile_kinds` predates autotile, where the stored
            // id could never exceed one legacy palette column per atlas entry.
            TILE_PALETTE.len()
        } else {
            tile_kinds
        };
        map.variant_count = variant_count;
        map.autotile = autotile;
        map.variant_seed = variant_seed;
        map.tint_table = tint_table;
        if let Some(pairs) = tints {
            for (index, id) in pairs {
                let cell = map.tint_ids.get_mut(index).ok_or_else(|| {
                    TilemapError::Parse(format!("tints cell index {index} is out of range"))
                })?;
                *cell = id;
            }
        }
        map.validate_layout()?;
        if let Some(position) = map
            .tint_ids
            .iter()
            .position(|id| *id > map.tint_table.len() as u8)
        {
            return Err(TilemapError::UnknownTintId {
                tint_id: map.tint_ids[position],
                size: map.tint_table.len(),
            });
        }
        for (i, cell) in tiles.into_iter().enumerate() {
            if cell as usize > map.max_tile_id() {
                return Err(TilemapError::InvalidCellIndex(cell));
            }
            map.tiles[i] = cell;
        }
        for (i, value) in solid.into_iter().enumerate() {
            map.solid[i] = value;
        }
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
    fn atlas_uv_addresses_rows_and_columns() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 3, 3).unwrap();
        map.atlas_cols = 12;
        map.atlas_rows = 8;
        map.tile_kinds = 12;
        // Entry 1 is the top-left cell of the atlas: column 0, row 0.
        let (min, max) = map.atlas_uv(1);
        assert_eq!((min.x, min.y), (0.0, 0.0));
        assert_eq!((max.x, max.y), (1.0 / 12.0, 1.0 / 8.0));
        // Entry 12 wraps to the last column of row 0.
        let (min, max) = map.atlas_uv(12);
        assert_eq!((min.x, min.y), (11.0 / 12.0, 0.0));
        assert_eq!((max.x, max.y), (1.0, 1.0 / 8.0));
        // Entry 13 starts row 1 — the legacy 6x1 addressing could not reach here.
        let (min, max) = map.atlas_uv(13);
        assert_eq!((min.x, min.y), (0.0, 1.0 / 8.0));
        assert_eq!((max.x, max.y), (1.0 / 12.0, 2.0 / 8.0));
        // Last entry of a 12x8 atlas covers the bottom-right sub-rectangle.
        let (min, max) = map.atlas_uv(96);
        assert_eq!((min.x, min.y), (11.0 / 12.0, 7.0 / 8.0));
        assert_eq!((max.x, max.y), (1.0, 1.0));
    }

    #[test]
    fn atlas_capacity_and_default_layout_match_legacy_columns() {
        let map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        assert_eq!((map.atlas_cols, map.atlas_rows), (6, 1));
        assert_eq!(map.atlas_capacity(), 6);
        assert_eq!(map.max_tile_id(), 6);
        // Default configuration resolves to the identity: id == atlas entry.
        let mut map = map;
        map.set_cell(0, 0, 4).unwrap();
        assert_eq!(map.resolve_atlas_index(0, 0), 4);
        assert_eq!(map.atlas_uv(4), Tilemap::tile_uv(4));
    }

    #[test]
    fn variants_are_deterministic_and_stay_in_block() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 8, 8).unwrap();
        map.atlas_cols = 8;
        map.atlas_rows = 2;
        map.tile_kinds = 4;
        map.variant_count = 4;
        map.variant_seed = 0x1234_5678;
        map.validate_layout().unwrap();
        map.set_cell(2, 3, 2).unwrap();
        let first = map.resolve_atlas_index(2, 3);
        // Kind 2 with 4 variants owns entries 5..8 (1-based block start
        // 1 + (2-1)*4), so the resolved entry must land inside that block.
        assert!(
            (5..=8).contains(&first),
            "resolved {first} outside kind 2 block"
        );
        for _ in 0..32 {
            assert_eq!(
                map.resolve_atlas_index(2, 3),
                first,
                "variant must not drift"
            );
        }
        // Neighbours of the same kind are not forced to the same variant.
        map.set_cell(3, 3, 2).unwrap();
        map.set_cell(4, 3, 2).unwrap();
        map.set_cell(5, 3, 2).unwrap();
        let neighbours = [
            map.resolve_atlas_index(2, 3),
            map.resolve_atlas_index(3, 3),
            map.resolve_atlas_index(4, 3),
            map.resolve_atlas_index(5, 3),
        ];
        assert!(
            neighbours
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                > 1,
            "adjacent same-kind cells all picked variant {neighbours:?}"
        );
    }

    #[test]
    fn autotile_mask_tracks_same_kind_neighbours() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 4).unwrap();
        map.atlas_cols = 16;
        map.atlas_rows = 4;
        map.tile_kinds = 3;
        map.autotile = true;
        map.validate_layout().unwrap();
        for y in 0..4 {
            for x in 0..4 {
                map.set_cell(x, y, 1).unwrap();
            }
        }
        // Fully surrounded interior cell: all four neighbours match.
        assert_eq!(map.autotile_mask(1, 1), 0b1111);
        // Out-of-bounds neighbours count as absent: (0,0) has no `y - 1` row and
        // no `x - 1` column.
        assert_eq!(map.autotile_mask(0, 0), 0b0110);
        // Changing one neighbour drops exactly its bit.
        map.set_cell(0, 1, 2).unwrap();
        assert_eq!(map.autotile_mask(1, 1), 0b0111);
        assert_eq!(Tilemap::autotile_slot(0b0111), 0b0111);
    }

    #[test]
    fn autotile_resolution_reaches_transition_blocks() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 4).unwrap();
        map.atlas_cols = 16;
        map.atlas_rows = 4;
        map.tile_kinds = 3;
        map.autotile = true;
        map.validate_layout().unwrap();
        for y in 0..4 {
            for x in 0..4 {
                map.set_cell(x, y, 1).unwrap();
            }
        }
        // Kind 1's transition block starts right after all kinds' base entries.
        // Interior cell (1,1) is fully surrounded: mask 1111 -> slot 15.
        assert_eq!(map.autotile_mask(1, 1), 0b1111);
        assert_eq!(map.resolve_atlas_index(1, 1), 1 + 3 + 15);
        // A neighbour of another kind is an absent neighbour: switching `x - 1`
        // and `y + 1` of (1,1) to kind 2 drops exactly their two bits.
        map.set_cell(0, 1, 2).unwrap();
        map.set_cell(1, 2, 2).unwrap();
        assert_eq!(map.autotile_mask(1, 1), 0b0011);
        assert_eq!(map.resolve_atlas_index(1, 1), 1 + 3 + 0b0011);
        // Kind 2's transition block starts after kind 1's whole 16-entry block.
        // (1,2) keeps only its `x + 1` neighbour -> mask 0b0010.
        map.set_cell(2, 2, 2).unwrap();
        assert_eq!(map.autotile_mask(1, 2), 0b0010);
        assert_eq!(map.resolve_atlas_index(1, 2), 1 + 3 + 16 + 2);
        // Same kind, different mask -> a different entry of the same block.
        assert_eq!(map.autotile_mask(2, 2), 0b1000);
        assert_eq!(map.resolve_atlas_index(2, 2), 1 + 3 + 16 + 8);
    }

    #[test]
    fn layout_validation_rejects_tiles_beyond_the_atlas() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        map.atlas_cols = 4;
        map.atlas_rows = 1;
        map.tile_kinds = 4;
        map.autotile = true;
        // 4 kinds x 16 transition entries does not fit a 4-entry atlas.
        assert!(matches!(
            map.validate_layout(),
            Err(TilemapError::AtlasTooSmall {
                needed: 68,
                capacity: 4,
                cols: 4,
                rows: 1
            })
        ));
        map.autotile = false;
        map.validate_layout().unwrap();
        map.atlas_rows = 0;
        assert!(matches!(
            map.validate_layout(),
            Err(TilemapError::EmptyAtlas)
        ));
    }

    #[test]
    fn tint_table_round_trips_and_rejects_unknown_ids() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 2, 2).unwrap();
        let dark = [0.6, 0.7, 0.6, 1.0];
        let id = map.register_tint(dark).unwrap();
        assert_eq!(id, 1);
        // Registering the same color again reuses the id (table stays size 1).
        assert_eq!(map.register_tint(dark).unwrap(), id);
        map.set_tint(1, 0, id).unwrap();
        assert_eq!(map.tint_at(1, 0), dark);
        assert_eq!(map.tint_at(0, 0), [1.0; 4], "untinted cells read white");
        assert!(matches!(
            map.set_tint(0, 0, 7),
            Err(TilemapError::UnknownTintId {
                tint_id: 7,
                size: 1
            })
        ));
        let parsed = Tilemap::from_component_str(&map.to_component_str()).unwrap();
        assert_eq!(parsed, map);
    }

    #[test]
    fn tint_serialization_is_sparse_and_not_capped_at_nine() {
        // 旧写法是一格一个 ASCII 数字：id>9 会被 `min(9)` 悄悄截掉（染色错位还不报错），
        // 而且 40x23 的图只为 4 个阴影列也要写 920 个字符。
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 20, 5).unwrap();
        let ids: Vec<u8> = (1..=12)
            .map(|i| {
                map.register_tint([1.0 - i as f32 * 0.05, 0.8, 0.6, 1.0])
                    .unwrap()
            })
            .collect();
        assert_eq!(ids, (1..=12).collect::<Vec<u8>>(), "ids must be dense");
        for (n, id) in ids.iter().enumerate() {
            map.set_tint(n % 20, n / 20, *id).unwrap();
        }
        let text = map.to_component_str();
        assert!(
            text.contains("tints=0:1,1:2,2:3"),
            "tints must list only tinted cells: {text}"
        );
        assert!(
            !text.contains("tints=0,1,2,3,4,5,6,7,8,9,0"),
            "no per-cell digit run"
        );
        let parsed = Tilemap::from_component_str(&text).unwrap();
        assert_eq!(parsed, map, "sparse tints must round-trip exactly");
        assert_eq!(parsed.tint_at(11, 0), map.tint_at(11, 0), "id 12 survives");
        // 越界格号与未知 id 都要报错，不能静默丢弃。
        assert!(
            Tilemap::from_component_str(&format!(
                "{};tints=999:1",
                text.split(";tints=").next().unwrap()
            ))
            .is_err()
        );
    }

    #[test]
    fn component_string_round_trips_atlas_fields() {
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 3, 2).unwrap();
        map.atlas_cols = 12;
        map.atlas_rows = 16;
        map.tile_kinds = 4;
        map.variant_count = 2;
        map.validate_layout().unwrap();
        map.set_cell(0, 0, 4).unwrap();
        map.set_cell(2, 1, 2).unwrap();
        let text = map.to_component_str();
        assert!(
            text.contains("tiles_format=1;atlas_cols=12;atlas_rows=16"),
            "unexpected component string: {text}"
        );
        assert!(
            text.contains(";tile_kinds=4;variant_count=2"),
            "autotile layout fields must be pinned in the string: {text}"
        );
        assert!(
            text.contains("tiles=4,0,0,0,0,2"),
            "unexpected tiles field: {text}"
        );
        let mut parsed = Tilemap::from_component_str(&text).unwrap();
        assert_eq!(parsed, map);
        assert_eq!(parsed.max_tile_id(), 4);
        assert!(parsed.set_cell(1, 1, 5).is_err(), "kind ids are bounded");
    }

    #[test]
    fn direct_atlas_indexing_reaches_entries_beyond_nine() {
        // Identity addressing (no variants, no autotile) is what M72-01 unlocks:
        // a single ASCII digit could never express index 97.
        let mut map = Tilemap::new(Vec2::new(32.0, 32.0), 4, 1).unwrap();
        map.atlas_cols = 12;
        map.atlas_rows = 9;
        map.set_cell(0, 0, 97).unwrap();
        let text = map.to_component_str();
        assert!(
            text.contains("tiles=97,0,0,0"),
            "unexpected tiles field: {text}"
        );
        let parsed = Tilemap::from_component_str(&text).unwrap();
        assert_eq!(parsed, map);
        assert_eq!(parsed.max_tile_id(), 12 * 9);
        // Entry 97 is row 8, column 0 of the 12x9 atlas.
        let (min, max) = parsed.atlas_uv(97);
        assert_eq!((min.x, min.y), (0.0, 8.0 / 9.0));
        assert_eq!((max.x, max.y), (1.0 / 12.0, 1.0));
    }

    #[test]
    fn legacy_component_string_reads_as_a_six_column_strip() {
        let map = Tilemap::from_component_str(
            "tile_size=32x32;width=3;height=2;texture=t.png;tiles=120345;solid=000000",
        )
        .unwrap();
        assert_eq!(map.tiles_format, TILES_FORMAT_LEGACY);
        assert_eq!((map.atlas_cols, map.atlas_rows), (6, 1));
        assert_eq!(map.tiles, vec![1, 2, 0, 3, 4, 5]);
        assert_eq!(map.tile_kinds, 6);
        assert_eq!(map.resolve_atlas_index(2, 0), 0, "empty cell stays empty");
        assert_eq!(map.atlas_uv(map.tiles[5]), Tilemap::tile_uv(5));
        // Re-encoding is the migration: same data, new format, UVs unchanged.
        let migrated = map.to_component_str();
        assert!(migrated.contains("tiles_format=1;atlas_cols=6;atlas_rows=1"));
        assert!(migrated.contains("tiles=1,2,0,3,4,5"));
        let reparsed = Tilemap::from_component_str(&migrated).unwrap();
        assert_eq!(reparsed.tiles_format, TILES_FORMAT_ATLAS);
        for index in 1..=6u8 {
            assert_eq!(
                reparsed.atlas_uv(index),
                Tilemap::tile_uv(index),
                "migration must not move entry {index}"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_legacy_tiles() {
        assert!(matches!(
            Tilemap::from_component_str("tile_size=32x32;width=2;height=1;tiles=17;solid=00"),
            Err(TilemapError::InvalidCellIndex(7))
        ));
    }

    #[test]
    fn rejects_unknown_tiles_format_and_bad_atlas_size() {
        assert!(matches!(
            Tilemap::from_component_str(
                "tile_size=32x32;width=1;height=1;tiles_format=7;tiles=0;solid=0"
            ),
            Err(TilemapError::UnsupportedTilesFormat(7))
        ));
        assert!(matches!(
            Tilemap::from_component_str(
                "tile_size=32x32;width=1;height=1;atlas_rows=0;tiles=0;solid=0"
            ),
            Err(TilemapError::EmptyAtlas)
        ));
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
