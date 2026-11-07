//! Built-in procedural terrain for the `__terrain__` mesh sentinel.
//!
//! The GDD needs a height surface without shipping a heightmap asset for every
//! map layer. A `MeshRenderer` mesh string of the form
//!
//! ```text
//! __terrain__?size=240&res=96&amp=2.4&seed=7
//! ```
//!
//! is parsed here, generates a CPU mesh once in the GPU mesh cache, and exposes
//! the same height field to Lua through `terrain3d.height_at(spec, x, z)`.
//! Determinism is part of the contract: the same spec always samples the same
//! height, on every platform.
//!
//! This is deliberately a *height field*, not a full terrain component: it has
//! no texture-array splatmap, collision mesh, or LOD chunks yet. Those remain
//! listed in `docs/05-engine-render-roadmap.md` as engine follow-ups. The
//! four-layer **textured** splat (vertex weights + a generated, mipmapped
//! detail atlas sampled in world space) lives in
//! [`crate::terrain_detail`] and is opt-in via `detail=<metres-per-tile>`.

use crate::mesh3d::{
    MaterialAsset, MaterialTexture, MeshAsset, MeshPrimitive, MeshVertex, TangentBasis,
};
use rustlua_assets::gltf_asset::GltfTextureImage;
use std::fmt;

/// Prefix used by `MeshRenderer.mesh` to request the built-in terrain.
pub const TERRAIN_PREFIX: &str = "__terrain__";

/// Limits keep one bad scene string from allocating a huge mesh.
pub const TERRAIN_MIN_SIZE: f32 = 8.0;
pub const TERRAIN_MAX_SIZE: f32 = 4000.0;
pub const TERRAIN_MIN_RES: u32 = 4;
pub const TERRAIN_MAX_RES: u32 = 192;
/// P3-R48：逐像素四通道 splat 权重图分辨率。
pub const SPLAT_MAP_RES: u32 = 512;
pub const TERRAIN_MAX_AMPLITUDE: f32 = 200.0;
/// Maximum local edits carried by one terrain spec string.
pub const TERRAIN_MAX_EDITS: usize = 32;
/// Smallest world-space size of one detail-atlas tile, in world units.
pub const TERRAIN_MIN_DETAIL: f32 = 0.25;
/// Largest world-space size of one detail-atlas tile, in world units.
pub const TERRAIN_MAX_DETAIL: f32 = 1024.0;

/// One local height edit baked into a terrain spec.
///
/// Edits are **immutable spec data** (encoded in the mesh path), not a mutable
/// terrain grid: raising a bump and showing it means building a new
/// `__terrain__?...&edit=x:z:radius:delta` string and assigning it to the
/// entity's `MeshRenderer`. That keeps the height query, mesh and renderer cache
/// key on one value and avoids a second hidden height source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainEdit {
    pub x: f32,
    pub z: f32,
    /// Influence radius in world units; `<= 0` is invalid.
    pub radius: f32,
    /// Height delta at the center; negative digs a pit.
    pub delta: f32,
}

impl TerrainEdit {
    const ZERO: Self = Self {
        x: 0.0,
        z: 0.0,
        radius: 1.0,
        delta: 0.0,
    };
}

/// A ray hit against the procedural height field.
///
/// `distance` is measured from the ray origin along the **normalized** direction,
/// so a caller can compare it with `max_distance` and place props exactly on the
/// visible surface. `normal` is the same central-difference normal the mesh uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainHit {
    /// World-space hit point on the height field.
    pub position: [f32; 3],
    /// Unit surface normal at the hit point.
    pub normal: [f32; 3],
    /// Distance from the ray origin.
    pub distance: f32,
}

/// Parsed terrain parameters. All fields are plain numbers so the mesh path is
/// stable in scene text and can be round-tripped without an asset file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainSpec {
    /// Full edge length of the square terrain in world units.
    pub size: f32,
    /// Quads per edge; vertices are `(res + 1)^2`.
    pub resolution: u32,
    /// Peak height in world units.
    pub amplitude: f32,
    /// Deterministic seed; identical seed = identical field.
    pub seed: u32,
    /// Extra depth carved along the built-in river curve (0 = no channel).
    pub river_depth: f32,
    /// Half-width of the carved channel in world units.
    pub river_width: f32,
    /// Extra depth carved along the north tributary curve (0 = no channel).
    pub tributary_depth: f32,
    /// Half-width of the north tributary channel.
    pub tributary_width: f32,
    /// Water surface height used for the wet-sand band, in raw terrain units.
    pub shore_level: f32,
    /// World-space size covered by one repetition of the ground detail atlas;
    /// `0` keeps the original flat procedural colours.
    ///
    /// The unit is whatever the project's world unit is (Myrmex's macro
    /// meadow uses sub-metre units, so its tiles are tens of units wide).
    /// The atlas is generated from the seed and embedded into the mesh
    /// material, so the mesh path stays a pure numeric spec.
    pub detail: f32,
    /// Local height edits, `edit_count` entries are active.
    pub edits: [TerrainEdit; TERRAIN_MAX_EDITS],
    /// Number of active entries in [`Self::edits`].
    pub edit_count: u8,
    /// P3-R145：可选分块中心（世界坐标）；`chunk_size == 0` 表示整张地形。
    pub chunk_origin: [f32; 2],
    /// 分块边长；0 = 整张 `size`。
    pub chunk_size: f32,
    /// P3-R147：分块裙边向下延伸的深度；0 = 无裙边。
    pub chunk_skirt: f32,
}

impl Default for TerrainSpec {
    fn default() -> Self {
        Self {
            size: 240.0,
            resolution: 96,
            amplitude: 2.4,
            seed: 7,
            river_depth: 0.0,
            river_width: 7.0,
            tributary_depth: 0.0,
            tributary_width: 2.5,
            shore_level: 0.0,
            detail: 0.0,
            edits: [TerrainEdit::ZERO; TERRAIN_MAX_EDITS],
            edit_count: 0,
            chunk_origin: [0.0, 0.0],
            chunk_size: 0.0,
            chunk_skirt: 0.0,
        }
    }
}

impl TerrainSpec {
    /// Parses the full mesh path (`__terrain__?size=..&...`) or the query part.
    pub fn parse(path: &str) -> Result<Self, TerrainSpecError> {
        let query = path
            .strip_prefix(TERRAIN_PREFIX)
            .ok_or(TerrainSpecError::MissingPrefix)?;
        let query = query
            .strip_prefix('?')
            .ok_or_else(|| TerrainSpecError::MalformedField(path.to_owned()))?;
        let mut spec = Self::default();
        for field in query.split('&').filter(|field| !field.is_empty()) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| TerrainSpecError::MalformedField(field.to_owned()))?;
            match key {
                "size" => spec.size = parse_number(raw, key)?,
                "res" => {
                    let value: f32 = parse_number(raw, key)?;
                    if !value.is_finite() || value.fract() != 0.0 {
                        return Err(TerrainSpecError::MalformedField(field.to_owned()));
                    }
                    spec.resolution = value as u32;
                }
                "amp" => spec.amplitude = parse_number(raw, key)?,
                "river" => spec.river_depth = parse_number(raw, key)?,
                "riverw" => spec.river_width = parse_number(raw, key)?,
                "trib" => spec.tributary_depth = parse_number(raw, key)?,
                "tribw" => spec.tributary_width = parse_number(raw, key)?,
                "shore" => spec.shore_level = parse_number(raw, key)?,
                "detail" => spec.detail = parse_number(raw, key)?,
                "cx" => spec.chunk_origin[0] = parse_number(raw, key)?,
                "cz" => spec.chunk_origin[1] = parse_number(raw, key)?,
                "cs" => spec.chunk_size = parse_number(raw, key)?,
                "sk" => spec.chunk_skirt = parse_number(raw, key)?,
                "seed" => {
                    let value: f32 = parse_number(raw, key)?;
                    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
                        return Err(TerrainSpecError::MalformedField(field.to_owned()));
                    }
                    spec.seed = value as u32;
                }
                "edit" => {
                    let parts: Vec<&str> = raw.split(':').collect();
                    if parts.len() != 4 {
                        return Err(TerrainSpecError::MalformedField(field.to_owned()));
                    }
                    spec.push_edit(TerrainEdit {
                        x: parse_number(parts[0], key)?,
                        z: parse_number(parts[1], key)?,
                        radius: parse_number(parts[2], key)?,
                        delta: parse_number(parts[3], key)?,
                    })?;
                }
                _ => return Err(TerrainSpecError::UnknownField(key.to_owned())),
            }
        }
        spec.validate()?;
        Ok(spec)
    }

    /// Appends a local edit; exposed for the Lua `terrain3d.raise` helper and
    /// for tests that build an edited spec without parsing a path.
    pub fn push_edit(&mut self, edit: TerrainEdit) -> Result<(), TerrainSpecError> {
        if edit.x.is_finite()
            && edit.z.is_finite()
            && edit.radius.is_finite()
            && edit.delta.is_finite()
            && edit.radius > 0.0
            && edit.radius <= TERRAIN_MAX_SIZE
            && edit.delta.abs() <= TERRAIN_MAX_AMPLITUDE
        {
            if usize::from(self.edit_count) >= TERRAIN_MAX_EDITS {
                return Err(TerrainSpecError::TooManyEdits(TERRAIN_MAX_EDITS));
            }
            self.edits[usize::from(self.edit_count)] = edit;
            self.edit_count += 1;
            Ok(())
        } else {
            Err(TerrainSpecError::EditOutOfRange)
        }
    }

    /// True when a `MeshRenderer.mesh` string asks for the built-in terrain.
    #[must_use]
    pub fn is_terrain_path(path: &str) -> bool {
        path.starts_with(TERRAIN_PREFIX)
    }

    /// Canonical mesh path used by scene text and the renderer cache.
    #[must_use]
    pub fn mesh_path(&self) -> String {
        let mut path = format!(
            "{TERRAIN_PREFIX}?size={}&res={}&amp={}&seed={}",
            self.size, self.resolution, self.amplitude, self.seed
        );
        if self.river_depth > 0.0 {
            path.push_str(&format!(
                "&river={}&riverw={}",
                self.river_depth, self.river_width
            ));
        }
        if self.tributary_depth > 0.0 {
            path.push_str(&format!(
                "&trib={}&tribw={}",
                self.tributary_depth, self.tributary_width
            ));
        }
        if self.shore_level != 0.0 {
            path.push_str(&format!("&shore={}", self.shore_level));
        }
        if self.detail > 0.0 {
            path.push_str(&format!("&detail={}", self.detail));
        }
        if self.chunk_size > 0.0 {
            path.push_str(&format!(
                "&cx={}&cz={}&cs={}",
                self.chunk_origin[0], self.chunk_origin[1], self.chunk_size
            ));
        }
        if self.chunk_skirt > 0.0 {
            path.push_str(&format!("&sk={}", self.chunk_skirt));
        }
        for edit in self.edits.iter().take(usize::from(self.edit_count)) {
            path.push_str(&format!(
                "&edit={}:{}:{}:{}",
                edit.x, edit.z, edit.radius, edit.delta
            ));
        }
        path
    }

    /// Rejects values that would make a degenerate or huge mesh.
    pub fn validate(&self) -> Result<(), TerrainSpecError> {
        if !self.size.is_finite() || !(TERRAIN_MIN_SIZE..=TERRAIN_MAX_SIZE).contains(&self.size) {
            return Err(TerrainSpecError::SizeOutOfRange(self.size));
        }
        if !(TERRAIN_MIN_RES..=TERRAIN_MAX_RES).contains(&self.resolution) {
            return Err(TerrainSpecError::ResolutionOutOfRange(self.resolution));
        }
        if !self.chunk_size.is_finite()
            || self.chunk_size < 0.0
            || self.chunk_size > self.size
            || !self.chunk_origin[0].is_finite()
            || !self.chunk_origin[1].is_finite()
            || !self.chunk_skirt.is_finite()
            || self.chunk_skirt < 0.0
            || self.chunk_skirt > 50.0
        {
            return Err(TerrainSpecError::MalformedField(format!(
                "chunk cx={} cz={} cs={} sk={}",
                self.chunk_origin[0], self.chunk_origin[1], self.chunk_size, self.chunk_skirt
            )));
        }
        if !self.amplitude.is_finite()
            || self.amplitude < 0.0
            || self.amplitude > TERRAIN_MAX_AMPLITUDE
        {
            return Err(TerrainSpecError::AmplitudeOutOfRange(self.amplitude));
        }
        if !self.river_depth.is_finite()
            || self.river_depth < 0.0
            || self.river_depth > TERRAIN_MAX_AMPLITUDE
        {
            return Err(TerrainSpecError::RiverDepthOutOfRange(self.river_depth));
        }
        if !self.river_width.is_finite() || !(1.0..=200.0).contains(&self.river_width) {
            return Err(TerrainSpecError::RiverWidthOutOfRange(self.river_width));
        }
        if !self.tributary_depth.is_finite()
            || self.tributary_depth < 0.0
            || self.tributary_depth > TERRAIN_MAX_AMPLITUDE
        {
            return Err(TerrainSpecError::RiverDepthOutOfRange(self.tributary_depth));
        }
        if !self.tributary_width.is_finite() || !(1.0..=200.0).contains(&self.tributary_width) {
            return Err(TerrainSpecError::RiverWidthOutOfRange(self.tributary_width));
        }
        if !self.shore_level.is_finite() {
            return Err(TerrainSpecError::MalformedField(
                "shore must be finite".to_owned(),
            ));
        }
        if !self.detail.is_finite()
            || (self.detail != 0.0
                && !(TERRAIN_MIN_DETAIL..=TERRAIN_MAX_DETAIL).contains(&self.detail))
        {
            return Err(TerrainSpecError::DetailOutOfRange(self.detail));
        }
        if usize::from(self.edit_count) > TERRAIN_MAX_EDITS {
            return Err(TerrainSpecError::TooManyEdits(TERRAIN_MAX_EDITS));
        }
        for edit in self.edits.iter().take(usize::from(self.edit_count)) {
            if !edit.x.is_finite()
                || !edit.z.is_finite()
                || !edit.radius.is_finite()
                || edit.radius <= 0.0
                || edit.radius > TERRAIN_MAX_SIZE
                || !edit.delta.is_finite()
                || edit.delta.abs() > TERRAIN_MAX_AMPLITUDE
            {
                return Err(TerrainSpecError::EditOutOfRange);
            }
        }
        Ok(())
    }

    /// Deterministic height at world `(x, z)`, in world units around y = 0.
    ///
    /// Three sines at different scales give broad hills plus a smaller ridge.
    /// The formula is the single source of truth for both the rendered mesh and
    /// `terrain3d.height_at`, so gameplay placement matches the visible ground.
    #[must_use]
    pub fn height(&self, x: f32, z: f32) -> f32 {
        if !x.is_finite() || !z.is_finite() {
            return 0.0;
        }
        let seed = self.seed as f32;
        let broad = (x * 0.045 + seed * 0.37).sin() * (z * 0.037 - seed * 0.21).cos();
        let ridge = ((x + z) * 0.083 + seed * 0.11).sin();
        let detail = (x * 0.19 + seed).sin() * (z * 0.17 - seed * 0.5).cos();
        let base = self.amplitude * (0.55 * broad + 0.28 * ridge + 0.12 * detail);
        let mut height = base;
        if self.river_depth > 0.0 {
            // Built-in river curve: the same two sines the game uses for `river_z`.
            // A Gaussian channel keeps the banks untouched and only lowers the bed.
            let center = (x * 0.035).sin() * 8.0 + (x * 0.011 + 1.2).sin() * 4.0;
            height -= carve(z - center, self.river_width, self.river_depth);
        }
        if self.tributary_depth > 0.0
            && let Some(center) = tributary_center(x)
        {
            height -= carve(z - center, self.tributary_width, self.tributary_depth);
        }
        for edit in self.edits.iter().take(usize::from(self.edit_count)) {
            let dx = x - edit.x;
            let dz = z - edit.z;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance < edit.radius {
                let t = 1.0 - distance / edit.radius;
                // Smoothstep falloff: exact delta at the center, C1 at the rim.
                height += edit.delta * t * t * (3.0 - 2.0 * t);
            }
        }
        height
    }

    /// Triangle-list mesh for the renderer cache. Positions are centered on the
    /// entity origin; normals come from central differences of [`Self::height`].
    /// Deterministic world-space normal at `(x, z)`.
    ///
    /// Uses central differences over one grid step so Lua-side prop tilt and the
    /// generated mesh normals agree closely. Flat/NaN inputs return up.
    #[must_use]
    pub fn normal(&self, x: f32, z: f32) -> [f32; 3] {
        if !x.is_finite() || !z.is_finite() {
            return [0.0, 1.0, 0.0];
        }
        let step = (self.size / self.resolution.max(TERRAIN_MIN_RES) as f32).max(0.25);
        let dx = (self.height(x + step, z) - self.height(x - step, z)) / (2.0 * step);
        let dz = (self.height(x, z + step) - self.height(x, z - step)) / (2.0 * step);
        let length = (dx * dx + 1.0 + dz * dz).sqrt();
        if !length.is_finite() || length <= 1e-6 {
            return [0.0, 1.0, 0.0];
        }
        [-dx / length, 1.0 / length, -dz / length]
    }

    /// Slope angle in degrees (0 = flat) at `(x, z)`.
    #[must_use]
    pub fn slope_degrees(&self, x: f32, z: f32) -> f32 {
        self.normal(x, z)[1].clamp(-1.0, 1.0).acos().to_degrees()
    }

    /// Deterministic four-channel surface mix at `(x, z)`:
    /// `[grass, moss, leaf_litter, bare_soil]`, normalized to sum 1.
    ///
    /// This is a **procedural splat weight**, not a texture splatmap: the terrain
    /// has no texture array, so the weights ride in the otherwise-unused
    /// `MeshVertex::weights` lane of this non-skinned mesh and the PBR terrain
    /// branch mixes four base colors before applying the height/slope shading.
    /// Three decorrelated low-frequency fields keep the bands broad enough to
    /// read at strategy zoom; they are deterministic for a given `seed`.
    #[must_use]
    pub fn splat_weights(&self, x: f32, z: f32) -> [f32; 4] {
        if !x.is_finite() || !z.is_finite() {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let seed = self.seed as f32;
        let grass = 0.5 + 0.5 * (x * 0.022 + seed * 0.31).sin() * (z * 0.019 - seed * 0.17).cos();
        let moss = 0.5 + 0.5 * (x * 0.047 - seed * 0.23).cos() * (z * 0.041 + seed * 0.57).sin();
        let litter = 0.5 + 0.5 * (x * 0.089 + seed * 0.73).sin() * (z * 0.071 - seed * 0.39).cos();
        let bare = 0.5 + 0.5 * (x * 0.013 - seed * 0.61).sin() * (z * 0.017 + seed * 0.11).cos();
        let raw = [grass * 0.55, moss * 0.35, litter * 0.25, bare * 0.15];
        let sum: f32 = raw.iter().sum();
        if !sum.is_finite() || sum <= 1e-6 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        [raw[0] / sum, raw[1] / sum, raw[2] / sum, raw[3] / sum]
    }

    /// Casts a ray against the height field and returns the nearest hit.
    ///
    /// The ray is clipped against [`Self::bounds`] first, then marched in
    /// half-cell steps with a bisection refinement at the first sign change.
    /// This is deliberately a **query**, not a physics collider: terrain has no
    /// rigid body, so `physics3d.raycast` cannot see it. Placement tools use
    /// this to put props on the visible ground without a separate height probe.
    ///
    /// Returns `None` for a zero/non-finite direction, a non-positive or
    /// non-finite `max_distance`, a miss, or a ray that starts already below the
    /// surface but travels away (the hit at t=0 is only reported when the origin
    /// itself is inside the terrain bounds).
    #[must_use]
    pub fn raycast(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
    ) -> Option<TerrainHit> {
        if origin.iter().any(|value| !value.is_finite())
            || direction.iter().any(|value| !value.is_finite())
            || !max_distance.is_finite()
            || max_distance <= 0.0
        {
            return None;
        }
        let length = (direction[0] * direction[0]
            + direction[1] * direction[1]
            + direction[2] * direction[2])
            .sqrt();
        if !length.is_finite() || length <= 1e-6 {
            return None;
        }
        let dir = [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ];
        let (min, max) = self.bounds();
        let mut enter = 0.0f32;
        let mut exit = max_distance;
        for axis in 0..3 {
            if dir[axis].abs() <= 1e-8 {
                if origin[axis] < min[axis] || origin[axis] > max[axis] {
                    return None;
                }
                continue;
            }
            let t1 = (min[axis] - origin[axis]) / dir[axis];
            let t2 = (max[axis] - origin[axis]) / dir[axis];
            let (near, far) = if t1 <= t2 { (t1, t2) } else { (t2, t1) };
            enter = enter.max(near);
            exit = exit.min(far);
            if enter > exit {
                return None;
            }
        }
        if exit < 0.0 {
            return None;
        }
        let start = enter.max(0.0).min(exit);
        let sample = |t: f32| {
            origin[1] + dir[1] * t - self.height(origin[0] + dir[0] * t, origin[2] + dir[2] * t)
        };
        let start_delta = sample(start);
        if start_delta <= 0.0 {
            return Some(self.hit_at(origin, dir, start));
        }
        let cell = (self.size / self.resolution.max(TERRAIN_MIN_RES) as f32).max(0.25);
        let span = exit - start;
        let steps = (span / (cell * 0.5).max(0.25)).ceil().clamp(1.0, 1024.0) as u32;
        let mut previous_t = start;
        let mut previous_delta = start_delta;
        for index in 1..=steps {
            let t = start + span * index as f32 / steps as f32;
            let delta = sample(t);
            if delta <= 0.0 {
                let hit_t = if previous_delta.abs() <= 1e-6 {
                    previous_t
                } else {
                    let mut low = previous_t;
                    let mut high = t;
                    for _ in 0..24 {
                        let middle = (low + high) * 0.5;
                        if sample(middle) <= 0.0 {
                            high = middle;
                        } else {
                            low = middle;
                        }
                    }
                    high
                };
                return Some(self.hit_at(origin, dir, hit_t));
            }
            previous_t = t;
            previous_delta = delta;
        }
        None
    }

    fn hit_at(&self, origin: [f32; 3], direction: [f32; 3], distance: f32) -> TerrainHit {
        let x = origin[0] + direction[0] * distance;
        let z = origin[2] + direction[2] * distance;
        TerrainHit {
            position: [x, self.height(x, z), z],
            normal: self.normal(x, z),
            distance,
        }
    }

    /// 逐像素 splat 权重图（512² RGBA8，R/G/B/A = 草/苔/落叶/裸土）。
    ///
    /// 与顶点权重同一个 [`Self::splat_weights`] 采样函数，世界坐标映射到
    /// `(u−0.5)·size`；着色器用 `world.xz / size + 0.5` 直接采样，因此 biome 边界
    /// 不再受 192×192 顶点插值限制。
    #[must_use]
    pub fn build_splat_map(&self) -> GltfTextureImage {
        let res = SPLAT_MAP_RES;
        let mut rgba = vec![0u8; (res * res * 4) as usize];
        for row in 0..res {
            for col in 0..res {
                let u = (col as f32 + 0.5) / res as f32;
                let v = (row as f32 + 0.5) / res as f32;
                let x = (u - 0.5) * self.size;
                let z = (v - 0.5) * self.size;
                let weights = self.splat_weights(x, z);
                let index = ((row * res + col) * 4) as usize;
                for channel in 0..4 {
                    rgba[index + channel] =
                        (weights[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        GltfTextureImage {
            width: res,
            height: res,
            rgba,
        }
    }

    #[must_use]
    pub fn build_mesh(&self) -> MeshAsset {
        let res = self.resolution.max(TERRAIN_MIN_RES);
        let span_size = if self.chunk_size > 0.0 { self.chunk_size } else { self.size };
        let origin = if self.chunk_size > 0.0 { self.chunk_origin } else { [0.0, 0.0] };
        let step = span_size / res as f32;
        let half = span_size * 0.5;
        let stride = res as usize + 1;
        let mut vertices = Vec::with_capacity(stride * stride);
        let mut min_y = -(self.amplitude + self.river_depth + self.tributary_depth);
        let mut max_y = self.amplitude;
        for edit in self.edits.iter().take(usize::from(self.edit_count)) {
            if edit.delta < 0.0 {
                min_y += edit.delta;
            } else {
                max_y += edit.delta;
            }
        }
        let span = (max_y - min_y).max(1e-4);
        for row in 0..=res {
            let z = -half + row as f32 * step;
            for col in 0..=res {
                let x = -half + col as f32 * step;
                let wx = origin[0] + x;
                let wz = origin[1] + z;
                let height = self.height(wx, wz);
                let dx = (self.height(wx + step, wz) - self.height(wx - step, wz)) / (2.0 * step);
                let dz = (self.height(wx, wz + step) - self.height(wx, wz - step)) / (2.0 * step);
                let normal = normalize([-dx, 1.0, -dz]);
                // UV is not a texture coordinate for terrain: x = normalized
                // height, y = slope. The PBR shader blends low river sand,
                // normal soil and steep rock from these two channels.
                let height_t = ((height - min_y) / span).clamp(0.0, 1.0);
                let slope_t = (1.0 - normal[1]).clamp(0.0, 1.0);
                // Terrain is never skinned, so the skin-weight lane is free to
                // carry the four procedural splat weights to the fragment shader.
                let splat = self.splat_weights(wx, wz);
                vertices.push(MeshVertex {
                    position: [x, height, z],
                    normal,
                    texcoord: [height_t, slope_t],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    joints: [0; 4],
                    weights: splat,
                });
            }
        }
        let mut indices = Vec::with_capacity(res as usize * res as usize * 6);
        for row in 0..res {
            for col in 0..res {
                let a = row * stride as u32 + col;
                let b = a + 1;
                let c = a + stride as u32;
                let d = c + 1;
                // Winding chosen so the geometric normal points +Y.
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        // P3-R147：分块裙边，向下延伸一圈，遮住不同 LOD 相邻块之间的细缝。
        if self.chunk_size > 0.0 && self.chunk_skirt > 0.0 {
            let skirt = self.chunk_skirt;
            let mut edge_starts = Vec::new();
            for row in [0usize, res as usize] {
                edge_starts.push(vertices.len() as u32);
                for col in 0..=res as usize {
                    let base = vertices[row * stride as usize + col];
                    vertices.push(MeshVertex {
                        position: [base.position[0], base.position[1] - skirt, base.position[2]],
                        normal: base.normal,
                        texcoord: base.texcoord,
                        tangent: base.tangent,
                        joints: base.joints,
                        weights: base.weights,
                    });
                }
            }
            for col in [0usize, res as usize] {
                edge_starts.push(vertices.len() as u32);
                for row in 0..=res as usize {
                    let base = vertices[row * stride as usize + col];
                    vertices.push(MeshVertex {
                        position: [base.position[0], base.position[1] - skirt, base.position[2]],
                        normal: base.normal,
                        texcoord: base.texcoord,
                        tangent: base.tangent,
                        joints: base.joints,
                        weights: base.weights,
                    });
                }
            }
            for (edge, start) in edge_starts.into_iter().enumerate() {
                let horizontal = edge < 2;
                for i in 0..res {
                    let (a, b) = if horizontal {
                        let row = if edge == 0 { 0 } else { res as usize };
                        let base = (row * stride + i as usize) as u32;
                        (base, base + 1)
                    } else {
                        let col = if edge == 2 { 0 } else { res as usize };
                        let base = (i as usize * stride + col) as u32;
                        (base, base + stride as u32)
                    };
                    let sa = start + i as u32;
                    let sb = sa + 1;
                    indices.extend_from_slice(&[a, sa, b, b, sa, sb]);
                }
            }
        }
        let detail_atlas =
            (self.detail > 0.0).then(|| crate::terrain_detail::ground_atlas(self.seed));
        let detail_normal = detail_atlas
            .as_ref()
            .map(|color| crate::terrain_detail::normal_atlas(color, 6.0));
        let splat_map = (self.detail > 0.0).then(|| self.build_splat_map());
        let material = MaterialAsset {
            base_color: [0.30, 0.27, 0.19, 1.0],
            metallic: 0.0,
            roughness: 0.95,
            // Negative normal_scale is the built-in terrain marker; the normal
            // atlas is sampled in world space by the terrain branch itself, not
            // through the generic tangent-space slot. -2 keeps the height/slope
            // colour-blend marker intact.
            normal_scale: -2.0,
            double_sided: self.chunk_skirt > 0.0,
            // Terrain is opaque, so alpha_cutoff is free to carry the normalized
            // shore level to the fragment shader (terrain wet-sand band). 1.0
            // means "no shore band" for a generic terrain without a water body.
            alpha_cutoff: if self.shore_level == 0.0 {
                1.0
            } else {
                ((self.shore_level - min_y) / span).clamp(0.0, 0.999)
            },
            // Opt-in textured splat: a generated 2x2 ground atlas embedded in
            // the material (no project asset path), sampled in world space by
            // the PBR terrain branch. `detail_scale` is world units per tile
            // and doubles as the shader-side "texture present" flag. The normal
            // atlas is derived from the colour atlas' luminance so blades and
            // leaf edges get matching relief.
            base_color_texture: detail_atlas.map(|image| MaterialTexture {
                path: None,
                image: Some(image),
            }),
            normal_texture: detail_normal.map(|image| MaterialTexture {
                path: None,
                image: Some(image),
            }),
            // P3-R48：逐像素四通道 splat 权重图，走 occlusion 槽位（线性、无 mip）。
            // 地形分支用它替换逐顶点插值；`occlusion_strength` 这个槽位对地形携带
            // 世界尺寸（着色器用 animation.y 读），所以地形不能再走通用 AO。
            occlusion_texture: splat_map.map(|image| MaterialTexture {
                path: None,
                image: Some(image),
            }),
            occlusion_strength: if self.detail > 0.0 { self.size } else { 0.0 },
            detail_scale: self.detail,
            ..MaterialAsset::default()
        };
        MeshAsset {
            name: Some("__terrain__".to_owned()),
            primitives: vec![MeshPrimitive {
                vertices,
                indices,
                material: Some(0),
                skinned: false,
                tangent_basis: TangentBasis::Generated,
            }],
            materials: vec![material],
        }
    }

    /// World-space AABB for culling and picking.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let half = self.size * 0.5;
        let mut min_y = -(self.amplitude + self.river_depth + self.tributary_depth);
        let mut max_y = self.amplitude;
        for edit in self.edits.iter().take(usize::from(self.edit_count)) {
            if edit.delta < 0.0 {
                min_y += edit.delta;
            } else {
                max_y += edit.delta;
            }
        }
        ([-half, min_y, -half], [half, max_y, half])
    }
}

fn parse_number(raw: &str, key: &str) -> Result<f32, TerrainSpecError> {
    raw.parse::<f32>()
        .map_err(|_| TerrainSpecError::MalformedField(format!("{key}={raw}")))
}

fn carve(distance: f32, width: f32, depth: f32) -> f32 {
    depth * (-(distance * distance) / (2.0 * width * width)).exp()
}

/// North tributary curve shared with the game's `terrain.tributary_z`.
fn tributary_center(x: f32) -> Option<f32> {
    if !(-96.0..=4.0).contains(&x) {
        return None;
    }
    let t = ((-x) / 96.0).clamp(0.0, 1.0);
    let join = (0.0f32 * 0.035).sin() * 8.0 + (0.0f32 * 0.011 + 1.2).sin() * 4.0;
    Some(join + (58.0 - join) * t.powf(1.15) + (x * 0.09).sin() * 2.0 * (1.0 - t))
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if !length.is_finite() || length <= 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}

/// Errors from parsing or validating a terrain mesh path.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TerrainSpecError {
    #[error("terrain mesh path must start with {TERRAIN_PREFIX}")]
    MissingPrefix,
    #[error("malformed terrain field: {0}")]
    MalformedField(String),
    #[error("unknown terrain field: {0}")]
    UnknownField(String),
    #[error("terrain size must be {TERRAIN_MIN_SIZE}..={TERRAIN_MAX_SIZE}: {0}")]
    SizeOutOfRange(f32),
    #[error("terrain resolution must be {TERRAIN_MIN_RES}..={TERRAIN_MAX_RES}: {0}")]
    ResolutionOutOfRange(u32),
    #[error("terrain amplitude must be 0..={TERRAIN_MAX_AMPLITUDE}: {0}")]
    AmplitudeOutOfRange(f32),
    #[error("terrain river depth must be 0..={TERRAIN_MAX_AMPLITUDE}: {0}")]
    RiverDepthOutOfRange(f32),
    #[error("terrain river width must be 1..=200: {0}")]
    RiverWidthOutOfRange(f32),
    #[error("terrain detail must be 0 or {TERRAIN_MIN_DETAIL}..={TERRAIN_MAX_DETAIL}: {0}")]
    DetailOutOfRange(f32),
    #[error("terrain supports at most {0} local edits")]
    TooManyEdits(usize),
    #[error("terrain edit values are out of range")]
    EditOutOfRange,
}

impl fmt::Display for TerrainSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.mesh_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let spec = TerrainSpec::parse("__terrain__?size=240&res=96&amp=2.4&seed=7").unwrap();
        assert_eq!(spec, TerrainSpec::default());
        assert_eq!(
            TerrainSpec::parse(&spec.mesh_path()).unwrap(),
            TerrainSpec::default()
        );
        assert!(TerrainSpec::is_terrain_path(&spec.mesh_path()));
    }

    #[test]
    fn rejects_bad_fields_and_ranges() {
        for bad in [
            "__terrain__?size=0",
            "__terrain__?res=1",
            "__terrain__?amp=-1",
            "__terrain__?nope=1",
            "__terrain__?size=abc",
            "__terrain__size=240",
        ] {
            assert!(TerrainSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn splat_map_is_per_pixel_and_normalized() {
        let spec = TerrainSpec {
            size: 480.0,
            resolution: 64,
            detail: 80.0,
            ..TerrainSpec::default()
        };
        let map = spec.build_splat_map();
        assert_eq!(map.width, SPLAT_MAP_RES);
        assert_eq!(map.height, SPLAT_MAP_RES);
        for (col, row) in [(0, 0), (SPLAT_MAP_RES / 2, SPLAT_MAP_RES / 2), (511, 511)] {
            let index = ((row * SPLAT_MAP_RES + col) * 4) as usize;
            let sum: u32 = map.rgba[index..index + 4]
                .iter()
                .map(|value| u32::from(*value))
                .sum();
            assert!((sum as i32 - 255).abs() <= 3, "splat sum {sum} at {col},{row}");
            // Decoded texel must match the same CPU splat function used by vertices.
            let u = (col as f32 + 0.5) / SPLAT_MAP_RES as f32;
            let v = (row as f32 + 0.5) / SPLAT_MAP_RES as f32;
            let expected = spec.splat_weights((u - 0.5) * spec.size, (v - 0.5) * spec.size);
            for channel in 0..4 {
                let actual = f32::from(map.rgba[index + channel]) / 255.0;
                assert!(
                    (actual - expected[channel]).abs() < 0.02,
                    "channel {channel} at {col},{row}: {actual} vs {}",
                    expected[channel]
                );
            }
        }
        assert_eq!(
            spec.build_splat_map(),
            spec.build_splat_map(),
            "splat map must be deterministic"
        );
    }

    #[test]
    fn detail_atlas_is_opt_in_embedded_and_round_trips() {
        let spec =
            TerrainSpec::parse("__terrain__?size=240&res=96&amp=2.4&seed=7&detail=2.4").unwrap();
        assert_eq!(spec.detail, 2.4);
        assert!(spec.mesh_path().contains("&detail=2.4"));
        assert_eq!(TerrainSpec::parse(&spec.mesh_path()).unwrap(), spec);

        for bad in [
            "__terrain__?detail=0.1",
            "__terrain__?detail=2048",
            "__terrain__?detail=-1",
            "__terrain__?detail=NaN",
            "__terrain__?detail=inf",
        ] {
            assert!(TerrainSpec::parse(bad).is_err(), "{bad} must be rejected");
        }

        // Opt-in: the default spec keeps the old flat-colour path byte for byte.
        let plain = TerrainSpec::default().build_mesh();
        assert!(plain.materials[0].base_color_texture.is_none());
        assert_eq!(plain.materials[0].detail_scale, 0.0);

        // Enabled: an embedded (path-less) atlas with the seed's pixels.
        let mesh = spec.build_mesh();
        let material = &mesh.materials[0];
        assert_eq!(material.detail_scale, 2.4);
        let texture = material
            .base_color_texture
            .as_ref()
            .expect("detail terrain must embed a ground atlas");
        assert!(
            texture.path.is_none(),
            "terrain detail must not need an asset file"
        );
        let image = texture.image.as_ref().expect("embedded RGBA8 pixels");
        assert_eq!(image.width, crate::terrain_detail::DETAIL_ATLAS_SIZE);
        assert_eq!(image.height, crate::terrain_detail::DETAIL_ATLAS_SIZE);
        // The normal atlas is embedded alongside the colour atlas and stays a
        // data texture (linear upload, path-less).
        let normal = material
            .normal_texture
            .as_ref()
            .expect("detail terrain must embed a normal atlas");
        assert!(normal.path.is_none());
        let normal_image = normal.image.as_ref().expect("normal RGBA8 pixels");
        assert_eq!(normal_image.width, crate::terrain_detail::DETAIL_ATLAS_SIZE);
        assert_eq!(
            normal_image.height,
            crate::terrain_detail::DETAIL_ATLAS_SIZE
        );
        assert_ne!(normal_image.rgba, image.rgba);
        assert_eq!(material.normal_scale, -2.0, "terrain marker must stay");
        assert!(plain.materials[0].normal_texture.is_none());
        // P3-R48: per-pixel splat weights ride the occlusion slot; terrain no
        // longer uses that slot for AO, so it carries the world size instead.
        let splat = material
            .occlusion_texture
            .as_ref()
            .expect("detail terrain must embed a splat map");
        assert!(splat.path.is_none());
        let splat_image = splat.image.as_ref().expect("splat RGBA8 pixels");
        assert_eq!(splat_image.width, SPLAT_MAP_RES);
        assert_eq!(splat_image.height, SPLAT_MAP_RES);
        assert_eq!(material.occlusion_strength, spec.size);
        assert!(plain.materials[0].occlusion_texture.is_none());
        assert_eq!(plain.materials[0].occlusion_strength, 0.0);

        // Rebuilding the same spec is deterministic (mesh cache key contract).
        let rebuilt = spec.build_mesh();
        assert_eq!(
            image,
            rebuilt.materials[0]
                .base_color_texture
                .as_ref()
                .unwrap()
                .image
                .as_ref()
                .unwrap()
        );
        // A different seed produces different ground detail.
        let other = TerrainSpec { seed: 8, ..spec }.build_mesh();
        assert_ne!(
            image,
            other.materials[0]
                .base_color_texture
                .as_ref()
                .unwrap()
                .image
                .as_ref()
                .unwrap()
        );
    }

    #[test]
    fn height_is_deterministic_and_bounded() {
        let spec = TerrainSpec::default();
        let a = spec.height(12.5, -8.0);
        let b = TerrainSpec::default().height(12.5, -8.0);
        assert_eq!(a, b);
        assert!(a.abs() <= spec.amplitude + 1e-5);
        assert_eq!(spec.height(f32::NAN, 0.0), 0.0);
    }

    #[test]
    fn river_carves_a_channel_and_round_trips() {
        let spec = TerrainSpec {
            river_depth: 1.2,
            river_width: 7.0,
            ..TerrainSpec::default()
        };
        let center = (0.0f32 * 0.035).sin() * 8.0 + (0.0f32 * 0.011 + 1.2).sin() * 4.0;
        let bed = spec.height(0.0, center);
        let bank = spec.height(0.0, center + 30.0);
        assert!(bed < bank - 0.8, "river bed {bed} vs bank {bank}");
        assert!(spec.bounds().0[1] <= -1.2);
        let parsed = TerrainSpec::parse(&spec.mesh_path()).unwrap();
        assert_eq!(parsed, spec);
        assert!(TerrainSpec::parse("__terrain__?river=-1").is_err());
        assert!(TerrainSpec::parse("__terrain__?riverw=0").is_err());
        let shore = TerrainSpec {
            shore_level: -0.15,
            ..spec
        };
        assert_eq!(TerrainSpec::parse(&shore.mesh_path()).unwrap(), shore);
        let mesh = shore.build_mesh();
        assert!(mesh.materials[0].alpha_cutoff > 0.0);
        assert!(mesh.materials[0].alpha_cutoff < 1.0);
    }

    #[test]
    fn normal_is_unit_length_and_points_up() {
        let spec = TerrainSpec::default();
        for (x, z) in [(0.0, 0.0), (12.0, -7.0), (-80.0, 45.0)] {
            let n = spec.normal(x, z);
            let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((length - 1.0).abs() < 1e-4, "normal length {length}");
            assert!(n[1] > 0.8, "normal y {} too small", n[1]);
        }
        assert_eq!(spec.normal(f32::NAN, 0.0), [0.0, 1.0, 0.0]);
        let slope = spec.slope_degrees(12.0, -7.0);
        assert!(
            slope.is_finite() && (0.0..90.0).contains(&slope),
            "slope {slope}"
        );
        assert!(
            spec.slope_degrees(0.0, 0.0) < 15.0,
            "default field should be gentle"
        );
    }

    #[test]
    fn raycast_hits_the_same_surface_the_mesh_draws() {
        let spec = TerrainSpec::default();
        let (x, z) = (12.0, -7.0);
        let expected = spec.height(x, z);
        let hit = spec
            .raycast([x, 50.0, z], [0.0, -1.0, 0.0], 200.0)
            .expect("downward ray must hit");
        assert!((hit.position[0] - x).abs() < 1e-3);
        assert!((hit.position[2] - z).abs() < 1e-3);
        assert!((hit.position[1] - expected).abs() < 1e-3);
        assert!((hit.distance - (50.0 - expected)).abs() < 1e-3);
        let n = hit.normal;
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((length - 1.0).abs() < 1e-4);

        // Direction is normalized before the distance is reported.
        let slanted = spec
            .raycast([x, 50.0, z], [0.0, -2.0, 0.0], 200.0)
            .expect("slanted ray");
        assert!((slanted.distance - hit.distance).abs() < 1e-3);
    }

    #[test]
    fn raycast_rejects_degenerate_and_missing_rays() {
        let spec = TerrainSpec::default();
        assert!(
            spec.raycast([0.0, 50.0, 0.0], [0.0, 1.0, 0.0], 100.0)
                .is_none()
        );
        assert!(
            spec.raycast([0.0, 50.0, 0.0], [0.0, 0.0, 0.0], 100.0)
                .is_none()
        );
        assert!(
            spec.raycast([0.0, 50.0, 0.0], [0.0, -1.0, 0.0], 0.0)
                .is_none()
        );
        assert!(
            spec.raycast([f32::NAN, 0.0, 0.0], [0.0, -1.0, 0.0], 100.0)
                .is_none()
        );
        // A flat zero-amplitude field is still a valid plane.
        let flat = TerrainSpec {
            amplitude: 0.0,
            ..TerrainSpec::default()
        };
        let hit = flat
            .raycast([0.0, 10.0, 0.0], [0.0, -1.0, 0.0], 20.0)
            .expect("flat plane");
        assert!(hit.position[1].abs() < 1e-5);
        assert!((hit.distance - 10.0).abs() < 1e-4);
    }

    #[test]
    fn local_edit_raises_center_and_round_trips_through_the_mesh_path() {
        let mut spec = TerrainSpec::default();
        let base = spec.height(0.0, 0.0);
        spec.push_edit(TerrainEdit {
            x: 0.0,
            z: 0.0,
            radius: 6.0,
            delta: 1.5,
        })
        .unwrap();
        assert!((spec.height(0.0, 0.0) - (base + 1.5)).abs() < 1e-5);
        let rim = TerrainSpec::default().height(6.0, 0.0);
        assert!((spec.height(6.0, 0.0) - rim).abs() < 1e-5);
        let path = spec.mesh_path();
        assert!(path.contains("&edit=0:0:6:1.5"));
        let parsed = TerrainSpec::parse(&path).unwrap();
        assert_eq!(parsed, spec);
        assert!((parsed.height(0.0, 0.0) - (base + 1.5)).abs() < 1e-5);
        assert!(parsed.bounds().1[1] > TerrainSpec::default().bounds().1[1]);
        let hit = parsed
            .raycast([0.0, 50.0, 0.0], [0.0, -1.0, 0.0], 200.0)
            .expect("raised surface is still hit");
        assert!((hit.position[1] - (base + 1.5)).abs() < 1e-3);
    }

    #[test]
    fn local_edits_are_bounded_and_deterministic() {
        let mut spec = TerrainSpec::default();
        for index in 0..TERRAIN_MAX_EDITS {
            spec.push_edit(TerrainEdit {
                x: index as f32 * 4.0,
                z: 0.0,
                radius: 3.0,
                delta: 0.2,
            })
            .unwrap();
        }
        assert_eq!(usize::from(spec.edit_count), TERRAIN_MAX_EDITS);
        assert!(matches!(
            spec.push_edit(TerrainEdit {
                x: 0.0,
                z: 0.0,
                radius: 3.0,
                delta: 0.2,
            }),
            Err(TerrainSpecError::TooManyEdits(_))
        ));
        let bad = [
            TerrainEdit {
                x: 0.0,
                z: 0.0,
                radius: 0.0,
                delta: 1.0,
            },
            TerrainEdit {
                x: 0.0,
                z: 0.0,
                radius: 3.0,
                delta: f32::NAN,
            },
        ];
        for edit in bad {
            let mut fresh = TerrainSpec::default();
            assert!(matches!(
                fresh.push_edit(edit),
                Err(TerrainSpecError::EditOutOfRange)
            ));
        }
        let pit = TerrainSpec {
            edits: {
                let mut edits = [TerrainEdit::ZERO; TERRAIN_MAX_EDITS];
                edits[0] = TerrainEdit {
                    x: 0.0,
                    z: 0.0,
                    radius: 4.0,
                    delta: -2.0,
                };
                edits
            },
            edit_count: 1,
            ..TerrainSpec::default()
        };
        assert!(pit.bounds().0[1] < TerrainSpec::default().bounds().0[1]);
        assert!(pit.height(0.0, 0.0) < TerrainSpec::default().height(0.0, 0.0));
    }

    #[test]
    fn splat_weights_are_normalized_and_vary_over_the_map() {
        let spec = TerrainSpec::default();
        let a = spec.splat_weights(0.0, 0.0);
        let b = spec.splat_weights(37.0, -19.0);
        assert!((a.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((b.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(a.iter().all(|weight| *weight >= 0.0));
        assert!(
            a.iter()
                .zip(b.iter())
                .any(|(left, right)| (left - right).abs() > 1e-3),
            "splat field must vary spatially: {a:?} vs {b:?}"
        );
        assert_eq!(spec.splat_weights(f32::NAN, 0.0), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            spec.splat_weights(12.5, -8.0),
            TerrainSpec::default().splat_weights(12.5, -8.0)
        );
    }

    #[test]
    fn terrain_vertices_carry_splat_weights_not_skin_weights() {
        let spec = TerrainSpec {
            resolution: 4,
            ..TerrainSpec::default()
        };
        let mesh = spec.build_mesh();
        let primitive = &mesh.primitives[0];
        for (index, vertex) in primitive.vertices.iter().enumerate() {
            let col = index % 5;
            let row = index / 5;
            let x = -spec.size * 0.5 + col as f32 * spec.size / 4.0;
            let z = -spec.size * 0.5 + row as f32 * spec.size / 4.0;
            assert_eq!(vertex.weights, spec.splat_weights(x, z));
            assert!((vertex.weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
        assert!(!primitive.skinned, "terrain must stay non-skinned");
    }

    #[test]
    fn mesh_has_expected_shape_and_upward_normals() {
        let spec = TerrainSpec {
            resolution: 4,
            ..TerrainSpec::default()
        };
        let mesh = spec.build_mesh();
        let primitive = &mesh.primitives[0];
        assert_eq!(primitive.vertices.len(), 25);
        assert_eq!(primitive.indices.len(), 4 * 4 * 6);
        assert!(
            primitive
                .vertices
                .iter()
                .all(|vertex| vertex.normal[1] > 0.0)
        );
        assert!(
            primitive
                .vertices
                .iter()
                .all(|vertex| (0.0..=1.0).contains(&vertex.texcoord[0])
                    && (0.0..=1.0).contains(&vertex.texcoord[1]))
        );
        assert_eq!(mesh.materials.len(), 1);
        assert_eq!(mesh.materials[0].normal_scale, -2.0);
    }

    #[test]
    fn chunk_builds_local_bounds_and_samples_global_height() {
        let base = TerrainSpec::default();
        let chunk = TerrainSpec {
            chunk_origin: [60.0, -20.0],
            chunk_size: 60.0,
            resolution: 4,
            ..TerrainSpec::default()
        };
        let mesh = chunk.build_mesh();
        let primitive = &mesh.primitives[0];
        assert!(primitive.vertices.iter().all(|v| v.position[0].abs() <= 30.0 + 1e-4));
        assert!(primitive.vertices.iter().all(|v| v.position[2].abs() <= 30.0 + 1e-4));
        let center = &primitive.vertices[2 * 5 + 2];
        assert!((center.position[1] - base.height(60.0, -20.0)).abs() < 1e-4);
        let path = chunk.mesh_path();
        assert!(path.contains("&cx=60&cz=-20&cs=60"), "{path}");
        let parsed = TerrainSpec::parse(&path).expect("chunk path parses");
        assert_eq!(parsed.chunk_origin, [60.0, -20.0]);
        assert_eq!(parsed.chunk_size, 60.0);
    }

    #[test]
    fn chunk_skirt_adds_downward_ring_and_double_sided_material() {
        let spec = TerrainSpec {
            chunk_origin: [0.0, 0.0],
            chunk_size: 60.0,
            chunk_skirt: 2.0,
            resolution: 4,
            ..TerrainSpec::default()
        };
        let mesh = spec.build_mesh();
        let primitive = &mesh.primitives[0];
        assert_eq!(primitive.vertices.len(), 25 + 4 * 5);
        assert_eq!(primitive.indices.len(), 4 * 4 * 6 + 4 * 4 * 6);
        assert!(mesh.materials[0].double_sided);
        let skirt = &primitive.vertices[25];
        assert!((skirt.position[1] + 2.0 - primitive.vertices[0].position[1]).abs() < 1e-4);
    }
}
