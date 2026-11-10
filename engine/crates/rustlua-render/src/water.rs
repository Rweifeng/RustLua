//! Built-in procedural water surface for the `__water__` mesh sentinel.
//!
//! `__water__?size=..&res=..&y=..&amp=..&seed=..` generates a transparent,
//! gently rippled plane. It is still a **surface**, not a water simulation:
//! there is no depth volume, flow field, reflection/refraction, or shoreline
//! wetness. The UV edge band carries `foam=` and `flow=` only advects the
//! procedural ripple/foam phase; neither moves the mesh or the height. Those
//! stay on the engine roadmap; this sentinel gives the game a real 3D water
//! mesh with a stable height query so props and units can sit on the water line.
//!
//! The material uses alpha blending and a low-roughness blue-green base color.
//! The renderer already routes `MaterialAlphaMode::Blend` draws after opaque
//! geometry, so overlapping water segments sort correctly.

use crate::mesh3d::{
    MaterialAlphaMode, MaterialAsset, MaterialTexture, MeshAsset, MeshPrimitive, MeshVertex,
    TangentBasis,
};
use crate::terrain::{TERRAIN_PREFIX, TerrainSpec};
use rustlua_assets::gltf_asset::GltfTextureImage;

/// Prefix used by `MeshRenderer.mesh` to request the built-in water surface.
pub const WATER_PREFIX: &str = "__water__";

pub const WATER_MIN_SIZE: f32 = 1.0;
pub const WATER_MAX_SIZE: f32 = 4000.0;
pub const WATER_MIN_RES: u32 = 2;
pub const WATER_MAX_RES: u32 = 128;
pub const WATER_MAX_AMPLITUDE: f32 = 5.0;
/// Scalar flow speed encoded along the surface's local +x axis.
pub const WATER_MAX_FLOW: f32 = 4.0;
/// Resolution of the baked per-pixel bed-depth texture.
pub const WATER_DEPTH_RES: u32 = 256;
/// Signed depth range encoded into the depth texture (decoded as `r*9-1`).
pub const WATER_DEPTH_MIN: f32 = -1.0;
pub const WATER_DEPTH_MAX: f32 = 8.0;

/// Parsed water parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSpec {
    /// Full edge length of the square surface.
    pub size: f32,
    /// Quads per edge.
    pub resolution: u32,
    /// Rest height of the surface in world units.
    pub level: f32,
    /// Ripple amplitude around `level`.
    pub amplitude: f32,
    /// Deterministic phase seed.
    pub seed: u32,
    /// Shore-foam strength near the surface edge, `0..=1`.
    ///
    /// The water has no depth texture, so this is an **edge band**, not a
    /// terrain-depth shoreline: `0` disables it, `1` is the strongest. It rides
    /// in the otherwise-unused `MaterialAsset::alpha_cutoff` slot (water is
    /// alpha-blended, so the cutoff never discards).
    pub foam: f32,
    /// P3-R125：结冰标记；真时材质把 `alpha_cutoff` 写成 2.0 作为冰面标记。
    pub ice: bool,
    /// P3-R197：水面廉价光照档。`true` 时 PBR shader 对这块水跳过 SH9 漫反射、
    /// 点光循环与四级联阴影采样（保留直射阳光、镜面 IBL 与天空反射）。默认 `false`，
    /// 缺省路径逐字节不变；由 `cheap=1` 打开，或宿主用 `--water-cheap` 全局临时开启。
    pub cheap_lighting: bool,
    /// Flow speed along the surface's local +x axis, `-4..=4`; `0` = still.
    ///
    /// This is a **visual advection** of the procedural ripple/foam phase, not a
    /// flow simulation: it does not move the mesh, change the water height, or
    /// push props. Water tiles orient their local +x along the river with
    /// `view.pose_prop(..., yaw, ...)`, so the game can set one scalar per tile.
    pub flow: f32,
    /// Optional terrain height field sampled per vertex to bake water depth.
    ///
    /// Once present, the water mesh carries `depth` in the vertex splat channel
    /// and the PBR shader fades/tints the surface by depth instead of relying
    /// only on the UV edge band. This is a **per-vertex depth bake**, not a
    /// depth texture or a water volume.
    pub bed: Option<TerrainSpec>,
    /// World-space origin of the local water mesh, used only when sampling
    /// `bed`; the generated vertices stay local to the mesh.
    pub bed_origin: [f32; 2],
    /// Yaw (radians) applied to the local mesh before sampling `bed`.
    pub bed_yaw: f32,
    /// Local `(x, z)` scale applied before sampling `bed`; matches the
    /// `Transform3D` scale of the water tile.
    pub bed_scale: [f32; 2],
}

impl Default for WaterSpec {
    fn default() -> Self {
        Self {
            size: 10.0,
            resolution: 8,
            level: 0.06,
            amplitude: 0.02,
            seed: 3,
            foam: 0.6,
            ice: false,
            cheap_lighting: false,
            flow: 0.0,
            bed: None,
            bed_origin: [0.0, 0.0],
            bed_yaw: 0.0,
            bed_scale: [1.0, 1.0],
        }
    }
}

impl WaterSpec {
    pub fn parse(path: &str) -> Result<Self, WaterSpecError> {
        let query = path
            .strip_prefix(WATER_PREFIX)
            .ok_or(WaterSpecError::MissingPrefix)?;
        let query = query
            .strip_prefix('?')
            .ok_or_else(|| WaterSpecError::MalformedField(path.to_owned()))?;
        let mut spec = Self::default();
        for field in query.split('&').filter(|field| !field.is_empty()) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| WaterSpecError::MalformedField(field.to_owned()))?;
            match key {
                "size" => spec.size = number(raw, key)?,
                "res" => spec.resolution = integer(raw, key)?,
                "y" => spec.level = number(raw, key)?,
                "amp" => spec.amplitude = number(raw, key)?,
                "seed" => spec.seed = integer(raw, key)?,
                "foam" => spec.foam = number(raw, key)?,
                "cheap" => {
                    spec.cheap_lighting = match integer(raw, key)? {
                        0 => false,
                        1 => true,
                        _ => {
                            return Err(WaterSpecError::MalformedField(format!("cheap={raw}")));
                        }
                    };
                }
                "ice" => {
                    spec.ice = match integer(raw, key)? {
                        0 => false,
                        1 => true,
                        _ => {
                            return Err(WaterSpecError::MalformedField(format!("ice={raw}")));
                        }
                    };
                }
                "flow" => spec.flow = number(raw, key)?,
                "bed" => {
                    // 嵌套地形 spec 用 `~` 代替 `&`，避免和本层的字段分隔符冲突。
                    let terrain =
                        TerrainSpec::parse(&format!("{TERRAIN_PREFIX}?{}", raw.replace('~', "&")))
                            .map_err(|error| {
                                WaterSpecError::MalformedField(format!("bed={raw}: {error}"))
                            })?;
                    spec.bed = Some(terrain);
                }
                "bed_x" => spec.bed_origin[0] = number(raw, key)?,
                "bed_z" => spec.bed_origin[1] = number(raw, key)?,
                "bed_yaw" => spec.bed_yaw = number(raw, key)?,
                "bed_sx" => spec.bed_scale[0] = number(raw, key)?,
                "bed_sz" => spec.bed_scale[1] = number(raw, key)?,
                _ => return Err(WaterSpecError::UnknownField(key.to_owned())),
            }
        }
        spec.validate()?;
        Ok(spec)
    }

    #[must_use]
    pub fn is_water_path(path: &str) -> bool {
        path.starts_with(WATER_PREFIX)
    }

    #[must_use]
    pub fn mesh_path(&self) -> String {
        let mut path = format!(
            "{WATER_PREFIX}?size={}&res={}&y={}&amp={}&seed={}",
            self.size, self.resolution, self.level, self.amplitude, self.seed
        );
        if self.foam != 0.6 {
            path.push_str(&format!("&foam={}", self.foam));
        }
        if self.cheap_lighting {
            path.push_str("&cheap=1");
        }
        if self.ice {
            path.push_str("&ice=1");
        }
        if self.flow != 0.0 {
            path.push_str(&format!("&flow={}", self.flow));
        }
        if let Some(bed) = self.bed {
            let nested = bed
                .mesh_path()
                .strip_prefix(TERRAIN_PREFIX)
                .and_then(|path| path.strip_prefix('?'))
                .unwrap_or_default()
                .replace('&', "~");
            path.push_str(&format!("&bed={nested}"));
        }
        if self.bed_origin[0] != 0.0 {
            path.push_str(&format!("&bed_x={}", self.bed_origin[0]));
        }
        if self.bed_origin[1] != 0.0 {
            path.push_str(&format!("&bed_z={}", self.bed_origin[1]));
        }
        if self.bed_yaw != 0.0 {
            path.push_str(&format!("&bed_yaw={}", self.bed_yaw));
        }
        if self.bed_scale[0] != 1.0 {
            path.push_str(&format!("&bed_sx={}", self.bed_scale[0]));
        }
        if self.bed_scale[1] != 1.0 {
            path.push_str(&format!("&bed_sz={}", self.bed_scale[1]));
        }
        path
    }

    pub fn validate(&self) -> Result<(), WaterSpecError> {
        if !self.size.is_finite() || !(WATER_MIN_SIZE..=WATER_MAX_SIZE).contains(&self.size) {
            return Err(WaterSpecError::SizeOutOfRange(self.size));
        }
        if !(WATER_MIN_RES..=WATER_MAX_RES).contains(&self.resolution) {
            return Err(WaterSpecError::ResolutionOutOfRange(self.resolution));
        }
        if !self.level.is_finite() {
            return Err(WaterSpecError::LevelNotFinite(self.level));
        }
        if !self.amplitude.is_finite()
            || self.amplitude < 0.0
            || self.amplitude > WATER_MAX_AMPLITUDE
        {
            return Err(WaterSpecError::AmplitudeOutOfRange(self.amplitude));
        }
        if !self.foam.is_finite() || !(0.0..=1.0).contains(&self.foam) {
            return Err(WaterSpecError::FoamOutOfRange(self.foam));
        }
        if !self.flow.is_finite() || self.flow.abs() > WATER_MAX_FLOW {
            return Err(WaterSpecError::FlowOutOfRange(self.flow));
        }
        if !self.bed_origin[0].is_finite() || !self.bed_origin[1].is_finite() {
            return Err(WaterSpecError::BedOriginNotFinite(self.bed_origin));
        }
        if !self.bed_yaw.is_finite()
            || !self.bed_scale[0].is_finite()
            || !self.bed_scale[1].is_finite()
            || self.bed_scale[0] <= 0.0
            || self.bed_scale[1] <= 0.0
        {
            return Err(WaterSpecError::BedTransformInvalid {
                yaw: self.bed_yaw,
                scale: self.bed_scale,
            });
        }
        Ok(())
    }

    /// Deterministic water height at world `(x, z)`.
    #[must_use]
    pub fn height(&self, x: f32, z: f32) -> f32 {
        if !x.is_finite() || !z.is_finite() {
            return self.level;
        }
        let seed = self.seed as f32;
        self.level
            + self.amplitude
                * ((x * 0.55 + seed).sin() * (z * 0.41 - seed * 0.7).cos()
                    + 0.45 * ((x - z) * 0.23 + seed * 1.3).sin())
    }

    /// Water depth over a terrain height field at `(x, z)`.
    ///
    /// Positive = water above the bed; negative = the point is dry land above the
    /// surface. No scene offsets are applied; callers own their world offsets.
    #[must_use]
    pub fn depth(&self, terrain: &crate::terrain::TerrainSpec, x: f32, z: f32) -> f32 {
        self.height(x, z) - terrain.height(x, z)
    }

    /// Bakes the bed depth into a per-pixel RGBA8 texture (R channel).
    ///
    /// The terrain is sampled on the **same local grid / bed transform** as the
    /// water vertices, so the fragment shader can read `uv` directly. Encoding
    /// is linear over `[WATER_DEPTH_MIN, WATER_DEPTH_MAX]` (`r*9-1`); negative
    /// values mean "terrain above the water surface" and drive the shoreline
    /// fade. The texture is linear (bound through the normal-map slot) so the
    /// renderer never applies an sRGB curve to a data texture.
    #[must_use]
    pub fn build_depth_texture(&self) -> Option<GltfTextureImage> {
        let bed = self.bed?;
        let res = WATER_DEPTH_RES;
        let half = self.size * 0.5;
        let range = WATER_DEPTH_MAX - WATER_DEPTH_MIN;
        let (sin_yaw, cos_yaw) = self.bed_yaw.sin_cos();
        let mut rgba = vec![0u8; (res * res * 4) as usize];
        for row in 0..res {
            for col in 0..res {
                let u = (col as f32 + 0.5) / res as f32;
                let v = (row as f32 + 0.5) / res as f32;
                let x = -half + u * self.size;
                let z = -half + v * self.size;
                let local_x = x * self.bed_scale[0];
                let local_z = z * self.bed_scale[1];
                let world_x = self.bed_origin[0] + local_x * cos_yaw - local_z * sin_yaw;
                let world_z = self.bed_origin[1] + local_x * sin_yaw + local_z * cos_yaw;
                let depth = (self.level - bed.height(world_x, world_z))
                    .clamp(WATER_DEPTH_MIN, WATER_DEPTH_MAX);
                let encoded = ((depth - WATER_DEPTH_MIN) / range).clamp(0.0, 1.0);
                let index = ((row * res + col) * 4) as usize;
                rgba[index] = (encoded * 255.0).round().clamp(0.0, 255.0) as u8;
                rgba[index + 3] = 255;
            }
        }
        Some(GltfTextureImage {
            width: res,
            height: res,
            rgba,
        })
    }

    #[must_use]
    pub fn build_mesh(&self) -> MeshAsset {
        let res = self.resolution.max(WATER_MIN_RES);
        let step = self.size / res as f32;
        let half = self.size * 0.5;
        let stride = res as usize + 1;
        let mut vertices = Vec::with_capacity(stride * stride);
        for row in 0..=res {
            let z = -half + row as f32 * step;
            for col in 0..=res {
                let x = -half + col as f32 * step;
                let y = self.height(x, z);
                // Finite-difference normal; ripples are small enough that the
                // plane stays stable under the existing PBR lighting.
                let dx = (self.height(x + step, z) - self.height(x - step, z)) / (2.0 * step);
                let dz = (self.height(x, z + step) - self.height(x, z - step)) / (2.0 * step);
                let normal = normalize([-dx, 1.0, -dz]);
                // 有 bed 时把水面减地形的深度烘进 splat.x，splat.y=1 作为
                // “这个顶点带深度”的标记；没有 bed 的老 spec 仍保持 [1,0,0,0]。
                let weights = if let Some(bed) = self.bed {
                    let (sin_yaw, cos_yaw) = self.bed_yaw.sin_cos();
                    let local_x = x * self.bed_scale[0];
                    let local_z = z * self.bed_scale[1];
                    let world_x = self.bed_origin[0] + local_x * cos_yaw - local_z * sin_yaw;
                    let world_z = self.bed_origin[1] + local_x * sin_yaw + local_z * cos_yaw;
                    [
                        (y - bed.height(world_x, world_z)).clamp(-1.0, 8.0),
                        1.0,
                        0.0,
                        0.0,
                    ]
                } else {
                    [1.0, 0.0, 0.0, 0.0]
                };
                vertices.push(MeshVertex {
                    position: [x, y, z],
                    normal,
                    texcoord: [col as f32 / res as f32, row as f32 / res as f32],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    joints: [0; 4],
                    weights,
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
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        let material = MaterialAsset {
            base_color: [0.10, 0.30, 0.36, 0.74],
            metallic: 0.0,
            roughness: 0.16,
            alpha_mode: MaterialAlphaMode::Blend,
            double_sided: true,
            // 负 normal_scale 是「顶点动画水面」的引擎内标记：水面没有法线贴图，
            // 这个值不会被采样；GPU 侧据此读取渲染时钟做涟漪。
            normal_scale: -1.0,
            // 水面是 blend，alpha 阈值不会丢弃任何像素；这个槽位因此携带
            // 泡沫强度（0..1）给片元着色器，默认 0.6。
            alpha_cutoff: if self.ice { 2.0 } else { self.foam },
            // 水面没有遮蔽贴图；flags.y（occlusion_strength）这个槽位因此携带
            // 沿局部 +x 的流速，片元着色器只在水面分支读它。
            occlusion_strength: self.flow,
            // 有 bed 时把河床深度烘成一张**逐像素**深度图，走 normal 槽位
            // （线性上传，不会被 sRGB 解码扭曲线性深度）；`detail_scale=-1`
            // 是水面“有深度图”的片元标记。顶点量 splat.x 仍保留作回退。
            normal_texture: self.build_depth_texture().map(|image| MaterialTexture {
                path: None,
                image: Some(image),
            }),
            detail_scale: if self.bed.is_some() { -1.0 } else { 0.0 },
            ..MaterialAsset::default()
        };
        MeshAsset {
            name: Some("__water__".to_owned()),
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

    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let half = self.size * 0.5;
        (
            [-half, self.level - self.amplitude, -half],
            [half, self.level + self.amplitude, half],
        )
    }
}

fn number(raw: &str, key: &str) -> Result<f32, WaterSpecError> {
    raw.parse::<f32>()
        .map_err(|_| WaterSpecError::MalformedField(format!("{key}={raw}")))
}

fn integer(raw: &str, key: &str) -> Result<u32, WaterSpecError> {
    let value: f32 = number(raw, key)?;
    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
        return Err(WaterSpecError::MalformedField(format!("{key}={raw}")));
    }
    Ok(value as u32)
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    if !length.is_finite() || length <= 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        [value[0] / length, value[1] / length, value[2] / length]
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum WaterSpecError {
    #[error("water mesh path must start with {WATER_PREFIX}")]
    MissingPrefix,
    #[error("malformed water field: {0}")]
    MalformedField(String),
    #[error("unknown water field: {0}")]
    UnknownField(String),
    #[error("water size must be {WATER_MIN_SIZE}..={WATER_MAX_SIZE}: {0}")]
    SizeOutOfRange(f32),
    #[error("water resolution must be {WATER_MIN_RES}..={WATER_MAX_RES}: {0}")]
    ResolutionOutOfRange(u32),
    #[error("water level must be finite: {0}")]
    LevelNotFinite(f32),
    #[error("water amplitude must be 0..={WATER_MAX_AMPLITUDE}: {0}")]
    AmplitudeOutOfRange(f32),
    #[error("water foam must be 0..=1: {0}")]
    FoamOutOfRange(f32),
    #[error("water flow must be -{WATER_MAX_FLOW}..={WATER_MAX_FLOW}: {0}")]
    FlowOutOfRange(f32),
    #[error("water bed origin must be finite: {0:?}")]
    BedOriginNotFinite([f32; 2]),
    #[error("water bed transform must be finite with positive scale: yaw={yaw}, scale={scale:?}")]
    BedTransformInvalid { yaw: f32, scale: [f32; 2] },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let spec = WaterSpec::parse("__water__?size=10&res=8&y=0.06&amp=0.02&seed=3").unwrap();
        assert_eq!(spec, WaterSpec::default());
        assert_eq!(WaterSpec::parse(&spec.mesh_path()).unwrap(), spec);
    }

    #[test]
    fn rejects_bad_fields() {
        for bad in [
            "__water__?size=0",
            "__water__?res=1",
            "__water__?amp=-1",
            "__water__?y=nan",
            "__water__?foam=-0.1",
            "__water__?foam=1.1",
            "__water__?flow=4.1",
            "__water__?flow=-4.1",
            "__water__?flow=nan",
            "__water__?bed_x=nan",
            "__water__?bed_z=inf",
            "__water__?bed_yaw=nan",
            "__water__?bed_sx=0",
            "__water__?nope=1",
            "__water__size=10",
        ] {
            assert!(WaterSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn foam_strength_round_trips_and_reaches_the_material() {
        let spec = WaterSpec {
            foam: 0.25,
            ..WaterSpec::default()
        };
        assert!(spec.mesh_path().contains("foam=0.25"));
        assert_eq!(WaterSpec::parse(&spec.mesh_path()).unwrap(), spec);
        let mesh = spec.build_mesh();
        assert!((mesh.materials[0].alpha_cutoff - 0.25).abs() < 1e-6);
        let off = WaterSpec {
            foam: 0.0,
            ..WaterSpec::default()
        };
        assert!(off.mesh_path().contains("foam=0"));
        assert_eq!(off.build_mesh().materials[0].alpha_cutoff, 0.0);
    }

    #[test]
    fn flow_round_trips_and_reaches_the_material() {
        let spec = WaterSpec {
            flow: 1.25,
            ..WaterSpec::default()
        };
        assert!(spec.mesh_path().contains("flow=1.25"));
        assert_eq!(WaterSpec::parse(&spec.mesh_path()).unwrap(), spec);
        assert_eq!(spec.build_mesh().materials[0].occlusion_strength, 1.25);
        let still = WaterSpec::default();
        assert!(!still.mesh_path().contains("flow="));
        assert_eq!(still.build_mesh().materials[0].occlusion_strength, 0.0);
    }

    #[test]
    fn bed_bakes_depth_and_round_trips() {
        let bed = crate::terrain::TerrainSpec {
            river_depth: 1.6,
            ..crate::terrain::TerrainSpec::default()
        };
        let spec = WaterSpec {
            bed: Some(bed),
            bed_origin: [10.0, -20.0],
            bed_yaw: 0.7,
            bed_scale: [1.12, 0.8],
            ..WaterSpec::default()
        };
        let path = spec.mesh_path();
        assert!(path.contains("&bed="), "{path}");
        let bed_value = path
            .split("&bed=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap();
        assert!(!bed_value.contains('&'), "嵌套字段必须用 `~` 分隔：{path}");
        assert!(bed_value.contains('~'), "{path}");
        assert_eq!(WaterSpec::parse(&path).unwrap(), spec);
        let mesh = spec.build_mesh();
        assert!(
            mesh.primitives[0]
                .vertices
                .iter()
                .all(|vertex| { vertex.weights[1] == 1.0 && vertex.weights[0].is_finite() })
        );
        let plain = WaterSpec::default().build_mesh();
        assert_eq!(
            plain.primitives[0].vertices[0].weights,
            [1.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn bed_bakes_a_per_pixel_depth_texture() {
        let river_center_z = (0.0f32 * 0.035).sin() * 8.0 + (0.0f32 * 0.011 + 1.2).sin() * 4.0;
        let terrain = crate::terrain::TerrainSpec {
            size: 480.0,
            resolution: 64,
            amplitude: 0.0,
            river_depth: 1.6,
            river_width: 5.0,
            ..crate::terrain::TerrainSpec::default()
        };
        let spec = WaterSpec {
            size: 480.0,
            resolution: 8,
            level: -0.5,
            amplitude: 0.0,
            foam: 0.0,
            bed: Some(terrain),
            ..WaterSpec::default()
        };
        let mesh = spec.build_mesh();
        let material = &mesh.materials[0];
        assert_eq!(material.detail_scale, -1.0, "water depth marker");
        let image = material
            .normal_texture
            .as_ref()
            .and_then(|texture| texture.image.as_ref())
            .expect("bed water must bake a depth texture");
        assert_eq!(image.width, WATER_DEPTH_RES);
        let sample = |x: f32, z: f32| -> f32 {
            let u = (x + 240.0) / 480.0;
            let v = (z + 240.0) / 480.0;
            let col = ((u * WATER_DEPTH_RES as f32) as u32).min(WATER_DEPTH_RES - 1);
            let row = ((v * WATER_DEPTH_RES as f32) as u32).min(WATER_DEPTH_RES - 1);
            let index = ((row * WATER_DEPTH_RES + col) * 4) as usize;
            f32::from(image.rgba[index]) / 255.0 * 9.0 - 1.0
        };
        let center = sample(0.0, river_center_z);
        let bank = sample(0.0, river_center_z + 20.0);
        assert!(
            center > 0.3,
            "river centre depth {center} should be positive"
        );
        assert!(bank < 0.0, "bank depth {bank} should be above water");

        // No bed: no texture and no marker; the shader falls back to vertex data.
        let open = WaterSpec::default().build_mesh();
        assert!(open.materials[0].normal_texture.is_none());
        assert_eq!(open.materials[0].detail_scale, 0.0);
    }

    #[test]
    fn height_is_deterministic_and_bounded() {
        let spec = WaterSpec::default();
        assert_eq!(
            spec.height(2.0, -3.0),
            WaterSpec::default().height(2.0, -3.0)
        );
        assert!(spec.height(2.0, -3.0).abs() <= spec.level + spec.amplitude + 1e-6);
        assert_eq!(spec.height(f32::INFINITY, 0.0), spec.level);
    }

    #[test]
    fn depth_is_water_minus_bed() {
        let water = WaterSpec::default();
        let terrain = crate::terrain::TerrainSpec::default();
        let depth = water.depth(&terrain, 0.0, 0.0);
        assert!((depth - (water.height(0.0, 0.0) - terrain.height(0.0, 0.0))).abs() < 1e-6);
        let high = crate::terrain::TerrainSpec {
            amplitude: 200.0,
            ..crate::terrain::TerrainSpec::default()
        };
        assert!(water.depth(&high, 0.0, 0.0) < 0.0);
    }

    #[test]
    fn ice_flag_round_trips_and_marks_the_material() {
        let spec = WaterSpec {
            ice: true,
            resolution: 4,
            ..WaterSpec::default()
        };
        let path = spec.mesh_path();
        assert!(path.contains("&ice=1"), "{path}");
        let parsed = WaterSpec::parse(&path).expect("ice path parses");
        assert!(parsed.ice);
        let mesh = spec.build_mesh();
        let primitive = &mesh.primitives[0];
        let material = &mesh.materials[primitive.material.expect("water has a material")];
        assert_eq!(material.alpha_cutoff, 2.0);
        assert!(WaterSpec::parse("__water__?ice=2").is_err());
    }

    #[test]
    fn mesh_is_transparent_and_upward_facing() {
        let spec = WaterSpec {
            resolution: 4,
            ..WaterSpec::default()
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
        assert_eq!(mesh.materials[0].alpha_mode, MaterialAlphaMode::Blend);
        // 负 normal_scale 是顶点动画开关；水面没有法线贴图，不会采样它。
        assert_eq!(mesh.materials[0].normal_scale, -1.0);
    }
}

#[cfg(test)]
mod cheap_lighting_tests {
    use super::WaterSpec;

    /// P3-R197：`cheap=1` 解析进 spec、并随 `mesh_path()` 往返；非法值报错。
    #[test]
    fn cheap_lighting_flag_parses_and_round_trips() {
        let spec = WaterSpec::parse("__water__?size=10&res=8&y=0.06&amp=0.02&seed=3&cheap=1")
            .expect("cheap=1 parses");
        assert!(spec.cheap_lighting);
        let round_trip = WaterSpec::parse(&spec.mesh_path()).expect("round trip");
        assert!(round_trip.cheap_lighting, "mesh_path 必须回写 cheap=1");
        assert!(
            WaterSpec::parse("__water__?cheap=2").is_err(),
            "cheap=2 必须报错"
        );
        assert!(!WaterSpec::default().cheap_lighting, "缺省必须关闭");
    }
}
