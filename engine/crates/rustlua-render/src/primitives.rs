//! M82-A：`__primitive__` 哨兵——参数化基本几何（box/sphere/cylinder/cone/torus/plane）。
//!
//! 与 `__terrain__` / `__water__` / `__cells__` 同一条路线：**规格串就是全部输入**，引擎按
//! 确定性参数合成几何，不读任何资源文件，跨平台/打包后逐字节一致。区别在于这里出的是通用
//! 实体而非地形/巢体，于是「零外部素材」的玩法/AI 闭环可以直接拿基本体搭场景，再经
//! `mesh.set_batch` 免费得到实例化。
//!
//! 规格编进网格路径：
//!
//! ```text
//! __primitive__?kind=box&w=1&h=1&d=1
//! __primitive__?kind=sphere&r=0.5&seg=16
//! __primitive__?kind=cylinder&r=0.5&h=1&seg=16
//! __primitive__?kind=cone&r=0.5&h=1&seg=16
//! __primitive__?kind=torus&R=0.5&r=0.2&seg=24&ring_seg=12
//! __primitive__?kind=plane&w=1&h=1
//! ```
//!
//! 所有几何以原点为中心（plane 落在 XZ 面、法线 +Y，与 `__ground_plane__` 同朝向）。
//! 逐字段校验、未知字段拒绝、非法 kind/非有限参数报错——口径照 [`crate::cellmesh`]。
//! 本模块**不新增 Lua 函数**：哨兵是 `mesh.set` / `mesh.set_batch` 的路径值，`mesh` 表在
//! M81 属 3D 包，故 primitive 天然 3D-only，2D 构建里 `mesh.set` 不存在即不可达。

use crate::mesh3d::{MaterialAsset, MeshAsset, MeshPrimitive, MeshVertex, TangentBasis};
use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::fmt;

/// `MeshRenderer.mesh` 里请求内建参数化基本体的前缀。
pub const PRIMITIVE_PREFIX: &str = "__primitive__";
/// 线性尺寸（w/h/d/r/R）下限：防一条坏规格串出退化几何。
pub const PRIMITIVE_MIN_SIZE: f32 = 0.05;
/// 线性尺寸上限（与 [`crate::cellmesh::CELLS_MAX_SIZE`] 同数）。
pub const PRIMITIVE_MAX_SIZE: f32 = 64.0;
/// 环向分段下限：少于 3 段围不成闭合环。
pub const PRIMITIVE_MIN_SEG: u32 = 3;
/// 环向分段上限：128² 已远超一个基本体所需，主要是防坏规格吃光内存。
pub const PRIMITIVE_MAX_SEG: u32 = 128;

/// 六种基本体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveKind {
    Box,
    Sphere,
    Cylinder,
    Cone,
    Torus,
    Plane,
}

impl PrimitiveKind {
    fn from_token(raw: &str) -> Result<Self, PrimitiveError> {
        match raw {
            "box" => Ok(Self::Box),
            "sphere" => Ok(Self::Sphere),
            "cylinder" => Ok(Self::Cylinder),
            "cone" => Ok(Self::Cone),
            "torus" => Ok(Self::Torus),
            "plane" => Ok(Self::Plane),
            _ => Err(PrimitiveError::InvalidKind(raw.to_owned())),
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Sphere => "sphere",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Torus => "torus",
            Self::Plane => "plane",
        }
    }
}

/// 一条 `__primitive__` 规格。每个变体只携带自己用到的参数，于是「未知字段」能按 kind 拒绝。
#[derive(Clone, Debug, PartialEq)]
pub enum PrimitiveSpec {
    /// 长方体，三轴边长 `w/h/d`，中心在原点。
    Box { w: f32, h: f32, d: f32 },
    /// UV 球，半径 `r`、经向分段 `seg`（纬向环数取 `max(2, seg/2)`）。
    Sphere { r: f32, seg: u32 },
    /// 圆柱（带上下盖），半径 `r`、高 `h`、环向分段 `seg`。
    Cylinder { r: f32, h: f32, seg: u32 },
    /// 圆锥（带底盖），底半径 `r`、高 `h`、环向分段 `seg`，顶点在 +Y。
    Cone { r: f32, h: f32, seg: u32 },
    /// 圆环，主半径 `R`、管半径 `r`、主环分段 `seg`、管环分段 `ring_seg`。
    Torus {
        big_r: f32,
        r: f32,
        seg: u32,
        ring_seg: u32,
    },
    /// XZ 平面，边长 `w/h`，法线 +Y，单面。
    Plane { w: f32, h: f32 },
}

impl PrimitiveSpec {
    /// True when a `MeshRenderer.mesh` string asks for a built-in primitive.
    #[must_use]
    pub fn is_primitive_path(path: &str) -> bool {
        path.starts_with(PRIMITIVE_PREFIX)
    }

    /// 解析 `__primitive__?...`：逐字段校验，未知/重复字段拒绝而非静默忽略。
    pub fn parse(path: &str) -> Result<Self, PrimitiveError> {
        let query = path
            .strip_prefix(PRIMITIVE_PREFIX)
            .ok_or(PrimitiveError::MissingPrefix)?;
        let query = query
            .strip_prefix('?')
            .ok_or_else(|| PrimitiveError::MalformedField(path.to_owned()))?;
        let mut fields: BTreeMap<&str, &str> = BTreeMap::new();
        for field in query.split('&').filter(|field| !field.is_empty()) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| PrimitiveError::MalformedField(field.to_owned()))?;
            if fields.insert(key, raw).is_some() {
                return Err(PrimitiveError::DuplicateField(key.to_owned()));
            }
        }
        let kind = PrimitiveKind::from_token(
            fields
                .remove("kind")
                .ok_or(PrimitiveError::MissingField("kind"))?,
        )?;
        let spec = match kind {
            PrimitiveKind::Box => Self::Box {
                w: pull_size(&mut fields, "w", 1.0)?,
                h: pull_size(&mut fields, "h", 1.0)?,
                d: pull_size(&mut fields, "d", 1.0)?,
            },
            PrimitiveKind::Sphere => Self::Sphere {
                r: pull_size(&mut fields, "r", 0.5)?,
                seg: pull_seg(&mut fields, "seg", 16)?,
            },
            PrimitiveKind::Cylinder => Self::Cylinder {
                r: pull_size(&mut fields, "r", 0.5)?,
                h: pull_size(&mut fields, "h", 1.0)?,
                seg: pull_seg(&mut fields, "seg", 16)?,
            },
            PrimitiveKind::Cone => Self::Cone {
                r: pull_size(&mut fields, "r", 0.5)?,
                h: pull_size(&mut fields, "h", 1.0)?,
                seg: pull_seg(&mut fields, "seg", 16)?,
            },
            PrimitiveKind::Torus => Self::Torus {
                big_r: pull_size(&mut fields, "R", 0.5)?,
                r: pull_size(&mut fields, "r", 0.2)?,
                seg: pull_seg(&mut fields, "seg", 24)?,
                ring_seg: pull_seg(&mut fields, "ring_seg", 12)?,
            },
            PrimitiveKind::Plane => Self::Plane {
                w: pull_size(&mut fields, "w", 1.0)?,
                h: pull_size(&mut fields, "h", 1.0)?,
            },
        };
        if let Some((key, _)) = fields.iter().next() {
            return Err(PrimitiveError::UnknownField((*key).to_owned()));
        }
        spec.validate()?;
        Ok(spec)
    }

    /// 尺寸/分段范围检查。形状对但参数非法（`r=0`、`seg=2`）依旧拒绝。
    pub fn validate(&self) -> Result<(), PrimitiveError> {
        let check_size = |value: f32| -> Result<(), PrimitiveError> {
            if !value.is_finite() || !(PRIMITIVE_MIN_SIZE..=PRIMITIVE_MAX_SIZE).contains(&value) {
                return Err(PrimitiveError::SizeOutOfRange);
            }
            Ok(())
        };
        let check_seg = |value: u32| -> Result<(), PrimitiveError> {
            if !(PRIMITIVE_MIN_SEG..=PRIMITIVE_MAX_SEG).contains(&value) {
                return Err(PrimitiveError::SegmentOutOfRange(value));
            }
            Ok(())
        };
        match *self {
            Self::Box { w, h, d } => {
                check_size(w)?;
                check_size(h)?;
                check_size(d)?;
            }
            Self::Sphere { r, seg } => {
                check_size(r)?;
                check_seg(seg)?;
            }
            Self::Cylinder { r, h, seg } | Self::Cone { r, h, seg } => {
                check_size(r)?;
                check_size(h)?;
                check_seg(seg)?;
            }
            Self::Torus {
                big_r,
                r,
                seg,
                ring_seg,
            } => {
                check_size(big_r)?;
                check_size(r)?;
                check_seg(seg)?;
                check_seg(ring_seg)?;
            }
            Self::Plane { w, h } => {
                check_size(w)?;
                check_size(h)?;
            }
        }
        Ok(())
    }

    /// 规范网格路径，也是渲染器的几何缓存键。
    #[must_use]
    pub fn mesh_path(&self) -> String {
        let kind = self.kind().token();
        match *self {
            Self::Box { w, h, d } => {
                format!("{PRIMITIVE_PREFIX}?kind={kind}&w={w}&h={h}&d={d}")
            }
            Self::Sphere { r, seg } => {
                format!("{PRIMITIVE_PREFIX}?kind={kind}&r={r}&seg={seg}")
            }
            Self::Cylinder { r, h, seg } => {
                format!("{PRIMITIVE_PREFIX}?kind={kind}&r={r}&h={h}&seg={seg}")
            }
            Self::Cone { r, h, seg } => {
                format!("{PRIMITIVE_PREFIX}?kind={kind}&r={r}&h={h}&seg={seg}")
            }
            Self::Torus {
                big_r,
                r,
                seg,
                ring_seg,
            } => format!(
                "{PRIMITIVE_PREFIX}?kind={kind}&R={big_r}&r={r}&seg={seg}&ring_seg={ring_seg}"
            ),
            Self::Plane { w, h } => {
                format!("{PRIMITIVE_PREFIX}?kind={kind}&w={w}&h={h}")
            }
        }
    }

    /// 这条规格属于哪一种基本体。
    #[must_use]
    pub fn kind(&self) -> PrimitiveKind {
        match self {
            Self::Box { .. } => PrimitiveKind::Box,
            Self::Sphere { .. } => PrimitiveKind::Sphere,
            Self::Cylinder { .. } => PrimitiveKind::Cylinder,
            Self::Cone { .. } => PrimitiveKind::Cone,
            Self::Torus { .. } => PrimitiveKind::Torus,
            Self::Plane { .. } => PrimitiveKind::Plane,
        }
    }

    /// 世界 AABB（中心在原点）。拾取与剔除按它算。
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        match *self {
            Self::Box { w, h, d } => ([-w / 2.0, -h / 2.0, -d / 2.0], [w / 2.0, h / 2.0, d / 2.0]),
            Self::Sphere { r, .. } => ([-r, -r, -r], [r, r, r]),
            Self::Cylinder { r, h, .. } | Self::Cone { r, h, .. } => {
                ([-r, -h / 2.0, -r], [r, h / 2.0, r])
            }
            Self::Torus { big_r, r, .. } => {
                let outer = big_r + r;
                ([-outer, -r, -outer], [outer, r, outer])
            }
            Self::Plane { w, h } => ([-w / 2.0, 0.0, -h / 2.0], [w / 2.0, 0.0, h / 2.0]),
        }
    }

    /// 顶点数（确定性契约的另一半读数：面数/顶点数随参数走）。
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.build_mesh().primitives[0].vertices.len()
    }

    /// 生成基本体网格。确定性契约：同一规格两次调用逐字节相同。
    #[must_use]
    pub fn build_mesh(&self) -> MeshAsset {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        match *self {
            Self::Box { w, h, d } => build_box(w, h, d, &mut vertices, &mut indices),
            Self::Sphere { r, seg } => build_sphere(r, seg, &mut vertices, &mut indices),
            Self::Cylinder { r, h, seg } => {
                build_cylinder(r, h, seg, &mut vertices, &mut indices);
            }
            Self::Cone { r, h, seg } => build_cone(r, h, seg, &mut vertices, &mut indices),
            Self::Torus {
                big_r,
                r,
                seg,
                ring_seg,
            } => build_torus(big_r, r, seg, ring_seg, &mut vertices, &mut indices),
            Self::Plane { w, h } => build_plane(w, h, &mut vertices, &mut indices),
        }
        let material = MaterialAsset {
            base_color: [0.62, 0.62, 0.66, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            double_sided: false,
            ..MaterialAsset::default()
        };
        MeshAsset {
            name: Some(PRIMITIVE_PREFIX.to_owned()),
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
}

impl fmt::Display for PrimitiveSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.mesh_path())
    }
}

fn pull_size(
    fields: &mut BTreeMap<&str, &str>,
    key: &str,
    default: f32,
) -> Result<f32, PrimitiveError> {
    match fields.remove(key) {
        Some(raw) => parse_number(raw, key),
        None => Ok(default),
    }
}

fn pull_seg(
    fields: &mut BTreeMap<&str, &str>,
    key: &str,
    default: u32,
) -> Result<u32, PrimitiveError> {
    let Some(raw) = fields.remove(key) else {
        return Ok(default);
    };
    let value = parse_number(raw, key)?;
    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
        return Err(PrimitiveError::MalformedField(format!("{key}={raw}")));
    }
    Ok(value as u32)
}

fn parse_number(raw: &str, key: &str) -> Result<f32, PrimitiveError> {
    raw.parse::<f32>()
        .map_err(|_| PrimitiveError::MalformedField(format!("{key}={raw}")))
}

fn vertex(position: [f32; 3], normal: [f32; 3], texcoord: [f32; 2]) -> MeshVertex {
    MeshVertex {
        position,
        normal,
        texcoord,
        tangent: [1.0, 0.0, 0.0, 1.0],
        joints: [0; 4],
        weights: [1.0, 0.0, 0.0, 0.0],
    }
}

/// 长方体六个面：角序沿用 [`crate::cellmesh`] 已验证朝外的绕序，按半边长缩放。
fn build_box(w: f32, h: f32, d: f32, vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>) {
    let half = [w / 2.0, h / 2.0, d / 2.0];
    // (法线, 四角符号)：符号 ±1 乘半边长。绕序与 cellmesh FACES 一致 ⇒ CCW 正面朝外。
    const FACES: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [1.0, 0.0, 0.0],
            [
                [1.0, -1.0, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, 1.0, 1.0],
                [1.0, -1.0, 1.0],
            ],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-1.0, -1.0, -1.0],
                [-1.0, -1.0, 1.0],
                [-1.0, 1.0, 1.0],
                [-1.0, 1.0, -1.0],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [
                [-1.0, 1.0, -1.0],
                [-1.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, -1.0],
            ],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-1.0, -1.0, -1.0],
                [1.0, -1.0, -1.0],
                [1.0, -1.0, 1.0],
                [-1.0, -1.0, 1.0],
            ],
        ),
        (
            [0.0, 0.0, 1.0],
            [
                [-1.0, -1.0, 1.0],
                [1.0, -1.0, 1.0],
                [1.0, 1.0, 1.0],
                [-1.0, 1.0, 1.0],
            ],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [-1.0, -1.0, -1.0],
                [-1.0, 1.0, -1.0],
                [1.0, 1.0, -1.0],
                [1.0, -1.0, -1.0],
            ],
        ),
    ];
    const UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for (normal, corners) in FACES {
        let base = vertices.len() as u32;
        for (corner, uv) in corners.iter().zip(UV) {
            vertices.push(vertex(
                [
                    corner[0] * half[0],
                    corner[1] * half[1],
                    corner[2] * half[2],
                ],
                normal,
                uv,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// XZ 平面，法线 +Y，单面（与 `__ground_plane__` 同朝向，尺寸参数化）。
fn build_plane(w: f32, h: f32, vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>) {
    let hx = w / 2.0;
    let hz = h / 2.0;
    let normal = [0.0, 1.0, 0.0];
    let corners = [
        [-hx, 0.0, -hz],
        [-hx, 0.0, hz],
        [hx, 0.0, hz],
        [hx, 0.0, -hz],
    ];
    let uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
    for (position, texcoord) in corners.iter().zip(uv) {
        vertices.push(vertex(*position, normal, texcoord));
    }
    indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
}

/// UV 球：纬向环数取 `max(2, seg/2)`，两极三角形退化但无害。
fn build_sphere(r: f32, seg: u32, vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>) {
    let rings = (seg / 2).max(2);
    let stride = seg + 1;
    for i in 0..=rings {
        let phi = PI * i as f32 / rings as f32;
        let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
        for j in 0..=seg {
            let theta = 2.0 * PI * j as f32 / seg as f32;
            let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
            let normal = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            vertices.push(vertex(
                [r * normal[0], r * normal[1], r * normal[2]],
                normal,
                [j as f32 / seg as f32, i as f32 / rings as f32],
            ));
        }
    }
    for i in 0..rings {
        for j in 0..seg {
            let v00 = i * stride + j;
            let v01 = v00 + 1;
            let v10 = (i + 1) * stride + j;
            let v11 = v10 + 1;
            indices.extend_from_slice(&[v00, v01, v11, v00, v11, v10]);
        }
    }
}

/// 圆柱：侧面（两环）+ 上下盖（三角扇）。
fn build_cylinder(
    r: f32,
    h: f32,
    seg: u32,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
) {
    let half = h / 2.0;
    let stride = seg + 1;
    // 侧面：ring 0 = 底（y=-half），ring 1 = 顶（y=+half）。
    for ring in 0..2u32 {
        let y = -half + ring as f32 * h;
        for j in 0..=seg {
            let theta = 2.0 * PI * j as f32 / seg as f32;
            let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
            vertices.push(vertex(
                [r * cos_theta, y, r * sin_theta],
                [cos_theta, 0.0, sin_theta],
                [j as f32 / seg as f32, ring as f32],
            ));
        }
    }
    for j in 0..seg {
        let b0 = j;
        let b1 = j + 1;
        let t0 = stride + j;
        let t1 = t0 + 1;
        indices.extend_from_slice(&[b0, t0, t1, b0, t1, b1]);
    }
    // 上盖（+Y）：扇形绕序 (center, j+1, j) 朝外。
    let top_center = vertices.len() as u32;
    vertices.push(vertex([0.0, half, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5]));
    let top_ring = vertices.len() as u32;
    for j in 0..=seg {
        let theta = 2.0 * PI * j as f32 / seg as f32;
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        vertices.push(vertex(
            [r * cos_theta, half, r * sin_theta],
            [0.0, 1.0, 0.0],
            [0.5 + 0.5 * cos_theta, 0.5 + 0.5 * sin_theta],
        ));
    }
    for j in 0..seg {
        indices.extend_from_slice(&[top_center, top_ring + j + 1, top_ring + j]);
    }
    // 下盖（-Y）：扇形绕序 (center, j, j+1) 朝外。
    let bottom_center = vertices.len() as u32;
    vertices.push(vertex([0.0, -half, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]));
    let bottom_ring = vertices.len() as u32;
    for j in 0..=seg {
        let theta = 2.0 * PI * j as f32 / seg as f32;
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        vertices.push(vertex(
            [r * cos_theta, -half, r * sin_theta],
            [0.0, -1.0, 0.0],
            [0.5 + 0.5 * cos_theta, 0.5 + 0.5 * sin_theta],
        ));
    }
    for j in 0..seg {
        indices.extend_from_slice(&[bottom_center, bottom_ring + j, bottom_ring + j + 1]);
    }
}

/// 圆锥：侧面（顶点扇）+ 底盖。顶点在 +Y，底在 -Y。
fn build_cone(r: f32, h: f32, seg: u32, vertices: &mut Vec<MeshVertex>, indices: &mut Vec<u32>) {
    let half = h / 2.0;
    // 侧面斜法线：径向分量 ∝ h、Y 分量 ∝ r（归一化）。
    let slant_len = (h * h + r * r).sqrt();
    let apex = vertices.len() as u32;
    vertices.push(vertex([0.0, half, 0.0], [0.0, 1.0, 0.0], [0.5, 1.0]));
    let base_ring = vertices.len() as u32;
    for j in 0..=seg {
        let theta = 2.0 * PI * j as f32 / seg as f32;
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        let normal = [
            h * cos_theta / slant_len,
            r / slant_len,
            h * sin_theta / slant_len,
        ];
        vertices.push(vertex(
            [r * cos_theta, -half, r * sin_theta],
            normal,
            [j as f32 / seg as f32, 0.0],
        ));
    }
    for j in 0..seg {
        indices.extend_from_slice(&[apex, base_ring + j + 1, base_ring + j]);
    }
    // 底盖（-Y）。
    let bottom_center = vertices.len() as u32;
    vertices.push(vertex([0.0, -half, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5]));
    let bottom_ring = vertices.len() as u32;
    for j in 0..=seg {
        let theta = 2.0 * PI * j as f32 / seg as f32;
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        vertices.push(vertex(
            [r * cos_theta, -half, r * sin_theta],
            [0.0, -1.0, 0.0],
            [0.5 + 0.5 * cos_theta, 0.5 + 0.5 * sin_theta],
        ));
    }
    for j in 0..seg {
        indices.extend_from_slice(&[bottom_center, bottom_ring + j, bottom_ring + j + 1]);
    }
}

/// 圆环：主环 `seg` 段 × 管环 `ring_seg` 段的网格。
fn build_torus(
    big_r: f32,
    r: f32,
    seg: u32,
    ring_seg: u32,
    vertices: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
) {
    let stride = ring_seg + 1;
    for i in 0..=seg {
        let theta = 2.0 * PI * i as f32 / seg as f32;
        let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
        for j in 0..=ring_seg {
            let phi = 2.0 * PI * j as f32 / ring_seg as f32;
            let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
            let normal = [cos_phi * cos_theta, sin_phi, cos_phi * sin_theta];
            let radial = big_r + r * cos_phi;
            vertices.push(vertex(
                [radial * cos_theta, r * sin_phi, radial * sin_theta],
                normal,
                [i as f32 / seg as f32, j as f32 / ring_seg as f32],
            ));
        }
    }
    for i in 0..seg {
        for j in 0..ring_seg {
            let v00 = i * stride + j;
            let v01 = v00 + 1;
            let v10 = (i + 1) * stride + j;
            let v11 = v10 + 1;
            indices.extend_from_slice(&[v00, v01, v11, v00, v11, v10]);
        }
    }
}

/// `__primitive__` 规格的解析/校验失败。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrimitiveError {
    #[error("primitive mesh path must start with {PRIMITIVE_PREFIX}")]
    MissingPrefix,
    #[error("malformed primitive field: {0}")]
    MalformedField(String),
    #[error("unknown primitive field: {0}")]
    UnknownField(String),
    #[error("duplicate primitive field: {0}")]
    DuplicateField(String),
    #[error("primitive field is required: {0}")]
    MissingField(&'static str),
    #[error("unknown primitive kind: {0}")]
    InvalidKind(String),
    #[error("primitive size out of range")]
    SizeOutOfRange,
    #[error("primitive segments must be {PRIMITIVE_MIN_SEG}..={PRIMITIVE_MAX_SEG}: {0}")]
    SegmentOutOfRange(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 朝外绕序的通用判据：每个三角形的几何法线与顶点法线均值同半球（dot>0）。
    /// 退化三角形（球两极）几何法线≈0，跳过。比 cellmesh 的逐轴符号判据更适合曲面。
    fn assert_outward(mesh: &MeshAsset) {
        let primitive = &mesh.primitives[0];
        for triangle in primitive.indices.chunks(3) {
            let [a, b, c] = [
                primitive.vertices[triangle[0] as usize],
                primitive.vertices[triangle[1] as usize],
                primitive.vertices[triangle[2] as usize],
            ];
            let u = [
                b.position[0] - a.position[0],
                b.position[1] - a.position[1],
                b.position[2] - a.position[2],
            ];
            let v = [
                c.position[0] - a.position[0],
                c.position[1] - a.position[1],
                c.position[2] - a.position[2],
            ];
            let g = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
            // 退化三角形（UV 球两极共用顶点）几何法线≈0，绕序无定义：按相对面积跳过，
            // 阈值随边长缩放，故对任意尺寸的基本体都成立。
            let u_len = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
            let v_len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if len <= 1e-6 * u_len * v_len {
                continue;
            }
            let n = [
                (a.normal[0] + b.normal[0] + c.normal[0]) / 3.0,
                (a.normal[1] + b.normal[1] + c.normal[1]) / 3.0,
                (a.normal[2] + b.normal[2] + c.normal[2]) / 3.0,
            ];
            let dot = g[0] * n[0] + g[1] * n[1] + g[2] * n[2];
            assert!(
                dot > 0.0,
                "triangle {triangle:?} winding inward: g={g:?} n={n:?}"
            );
        }
    }

    fn assert_normals_unit(mesh: &MeshAsset) {
        for v in &mesh.primitives[0].vertices {
            let len =
                (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                    .sqrt();
            assert!((len - 1.0).abs() < 1e-4, "normal not unit: {:?}", v.normal);
        }
    }

    fn all_specs() -> Vec<PrimitiveSpec> {
        vec![
            PrimitiveSpec::Box {
                w: 1.0,
                h: 2.0,
                d: 3.0,
            },
            PrimitiveSpec::Sphere { r: 0.5, seg: 16 },
            PrimitiveSpec::Cylinder {
                r: 0.5,
                h: 1.0,
                seg: 16,
            },
            PrimitiveSpec::Cone {
                r: 0.5,
                h: 1.0,
                seg: 16,
            },
            PrimitiveSpec::Torus {
                big_r: 0.5,
                r: 0.2,
                seg: 24,
                ring_seg: 12,
            },
            PrimitiveSpec::Plane { w: 2.0, h: 1.0 },
        ]
    }

    #[test]
    fn box_is_a_closed_cube() {
        let spec = PrimitiveSpec::Box {
            w: 1.0,
            h: 1.0,
            d: 1.0,
        };
        let mesh = spec.build_mesh();
        assert_eq!(mesh.primitives[0].vertices.len(), 24);
        assert_eq!(mesh.primitives[0].indices.len(), 36);
        assert_outward(&mesh);
        assert_normals_unit(&mesh);
        assert_eq!(spec.bounds(), ([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]));
    }

    #[test]
    fn plane_is_single_sided_up() {
        let spec = PrimitiveSpec::Plane { w: 2.0, h: 1.0 };
        let mesh = spec.build_mesh();
        assert_eq!(mesh.primitives[0].vertices.len(), 4);
        assert_eq!(mesh.primitives[0].indices.len(), 6);
        assert_outward(&mesh);
        assert!(
            mesh.primitives[0]
                .vertices
                .iter()
                .all(|v| v.normal == [0.0, 1.0, 0.0])
        );
        assert!(!mesh.materials[0].double_sided);
        assert_eq!(spec.bounds(), ([-1.0, 0.0, -0.5], [1.0, 0.0, 0.5]));
    }

    #[test]
    fn every_kind_winds_outward_with_unit_normals() {
        for spec in all_specs() {
            let mesh = spec.build_mesh();
            assert_outward(&mesh);
            assert_normals_unit(&mesh);
            assert!(!mesh.primitives[0].vertices.is_empty());
            assert!(!mesh.primitives[0].indices.is_empty());
            assert_eq!(mesh.primitives[0].indices.len() % 3, 0);
        }
    }

    #[test]
    fn sphere_vertex_and_index_counts() {
        // rings = max(2, seg/2) = 8；网格 (rings+1)*(seg+1) 顶点、rings*seg*6 索引。
        let spec = PrimitiveSpec::Sphere { r: 1.0, seg: 16 };
        let mesh = spec.build_mesh();
        assert_eq!(mesh.primitives[0].vertices.len(), 9 * 17);
        assert_eq!(mesh.primitives[0].indices.len(), 8 * 16 * 6);
        // 所有顶点落在半径 1 的球面上。
        for v in &mesh.primitives[0].vertices {
            let len = (v.position[0] * v.position[0]
                + v.position[1] * v.position[1]
                + v.position[2] * v.position[2])
                .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "off-sphere vertex {:?}",
                v.position
            );
        }
    }

    #[test]
    fn torus_counts_and_bounds() {
        let spec = PrimitiveSpec::Torus {
            big_r: 0.5,
            r: 0.2,
            seg: 24,
            ring_seg: 12,
        };
        let mesh = spec.build_mesh();
        assert_eq!(mesh.primitives[0].vertices.len(), 25 * 13);
        assert_eq!(mesh.primitives[0].indices.len(), 24 * 12 * 6);
        assert_eq!(spec.bounds(), ([-0.7, -0.2, -0.7], [0.7, 0.2, 0.7]));
    }

    #[test]
    fn building_is_deterministic() {
        for spec in all_specs() {
            let a = spec.build_mesh();
            let b = spec.build_mesh();
            assert_eq!(a.primitives[0].vertices, b.primitives[0].vertices);
            assert_eq!(a.primitives[0].indices, b.primitives[0].indices);
        }
    }

    #[test]
    fn path_round_trips_for_every_kind() {
        for spec in all_specs() {
            let path = spec.mesh_path();
            let parsed = PrimitiveSpec::parse(&path).expect("canonical path parses");
            assert_eq!(parsed, spec);
            assert_eq!(parsed.mesh_path(), path);
        }
    }

    #[test]
    fn defaults_parse_from_kind_only() {
        let cases = [
            (
                "__primitive__?kind=box",
                PrimitiveSpec::Box {
                    w: 1.0,
                    h: 1.0,
                    d: 1.0,
                },
            ),
            (
                "__primitive__?kind=sphere",
                PrimitiveSpec::Sphere { r: 0.5, seg: 16 },
            ),
            (
                "__primitive__?kind=plane",
                PrimitiveSpec::Plane { w: 1.0, h: 1.0 },
            ),
        ];
        for (path, expected) in cases {
            assert_eq!(PrimitiveSpec::parse(path).unwrap(), expected);
        }
    }

    #[test]
    fn field_order_does_not_matter() {
        let a = PrimitiveSpec::parse("__primitive__?kind=box&w=2&h=3&d=4").unwrap();
        let b = PrimitiveSpec::parse("__primitive__?d=4&kind=box&h=3&w=2").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn invalid_kind_and_unknown_fields_rejected() {
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=pyramid"),
            Err(PrimitiveError::InvalidKind(_))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=box&radius=1"),
            Err(PrimitiveError::UnknownField(_))
        ));
        // sphere 不认识 h：按 kind 拒绝，而不是全局放行。
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=sphere&h=1"),
            Err(PrimitiveError::UnknownField(_))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=box&w=1&w=2"),
            Err(PrimitiveError::DuplicateField(_))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?w=1"),
            Err(PrimitiveError::MissingField("kind"))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("wat?kind=box"),
            Err(PrimitiveError::MissingPrefix)
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__kind=box"),
            Err(PrimitiveError::MalformedField(_))
        ));
    }

    #[test]
    fn out_of_range_params_rejected() {
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=box&w=0"),
            Err(PrimitiveError::SizeOutOfRange)
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=sphere&r=nan"),
            Err(PrimitiveError::SizeOutOfRange)
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=sphere&r=inf"),
            Err(PrimitiveError::SizeOutOfRange)
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=box&w=65"),
            Err(PrimitiveError::SizeOutOfRange)
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=sphere&seg=2"),
            Err(PrimitiveError::SegmentOutOfRange(2))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=torus&ring_seg=129"),
            Err(PrimitiveError::SegmentOutOfRange(129))
        ));
        assert!(matches!(
            PrimitiveSpec::parse("__primitive__?kind=cylinder&seg=16.5"),
            Err(PrimitiveError::MalformedField(_))
        ));
    }

    #[test]
    fn is_primitive_path_matches_prefix_only() {
        assert!(PrimitiveSpec::is_primitive_path("__primitive__?kind=box"));
        // 前缀判定只看前缀（与 cellmesh 同口径）；`?` 与参数合法性由 parse 兜底。
        assert!(PrimitiveSpec::is_primitive_path("__primitive__nope"));
        assert!(PrimitiveSpec::parse("__primitive__nope").is_err());
        assert!(!PrimitiveSpec::is_primitive_path("__cells__?cell=1"));
        assert!(!PrimitiveSpec::is_primitive_path("assets/mesh.glb"));
    }

    /// 分段上限内顶点数有界：128×128 圆环 ≈ 1.6 万顶点，不会被一条坏规格吃光内存。
    #[test]
    fn max_segments_stay_bounded() {
        let spec = PrimitiveSpec::Torus {
            big_r: 1.0,
            r: 0.25,
            seg: PRIMITIVE_MAX_SEG,
            ring_seg: PRIMITIVE_MAX_SEG,
        };
        spec.validate().unwrap();
        let mesh = spec.build_mesh();
        assert_eq!(mesh.primitives[0].vertices.len(), 129 * 129);
        assert_outward(&mesh);
    }
}
