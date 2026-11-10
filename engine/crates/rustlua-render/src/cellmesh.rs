//! P3-R213：`__cells__` 哨兵——把一组格子生成一张实心体网格，给「可挖巢体」一个几何前置。
//!
//! 巢在模拟侧本来就是逐格的（挖开的腔室就是空格子），但引擎里所有 3D 几何要么来自 `.glb`
//! 文件，要么来自 `__terrain__` / `__water__` 这类程序化哨兵，**没有「按格子出网格」的入口**
//! ⇒ 巢体只能是一颗预制球壳：剖开里面是空的，也无从「挖」。这里补的就是这一格。
//!
//! 规格和地形一样**编进网格路径**：
//!
//! ```text
//! __cells__?cell=1&ox=-6&oy=0&oz=-6&w=12&h=8&d=12&bits=<base64url>
//! ```
//!
//! `bits` 是 `w*h*d` 个实心标志按位打包（`index = x + w * (z + d * y)`，低位在前），一次编辑
//! ⇒ 一个新路径 ⇒ 渲染器缓存里的一份新几何；高度查询、网格与缓存键仍是同一个值，不留第二份
//! 隐藏状态（与 [`crate::terrain::TerrainEdit`] 同一条理由）。
//!
//! 生成的是**只保留暴露面**的实心体：相邻两个实心格之间的面直接不生成，所以面数正比于表面积
//! 而不是体积（2×2×2 满块 24 面而非 48 面；3×3×3 满块 54 面，中心格 0 面）。默认 `ds=0` ⇒
//! 单面网格，正是 P3-R211 那条双面强制档第一条真吃得到的几何。

use crate::mesh3d::{MaterialAsset, MeshAsset, MeshPrimitive, MeshVertex, TangentBasis};
use std::fmt;

/// `MeshRenderer.mesh` 里请求内建逐格实心体的前缀。
pub const CELLS_PREFIX: &str = "__cells__";
/// 每个轴最多多少格（64³ 已经远超一个巢体，主要是防一条坏场景串吃光内存）。
pub const CELLS_MAX_PER_AXIS: u32 = 64;
/// 一格最多多少立方米世界单位。
pub const CELLS_MAX_TOTAL: usize = 32_768;
pub const CELLS_MIN_SIZE: f32 = 0.05;
pub const CELLS_MAX_SIZE: f32 = 64.0;

const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 面方向表：法线（同时也是邻居偏移）、四个角在格内的偏移、UV 取哪两条世界轴。
/// 角的顺序保证 `(p1 - p0) × (p2 - p0)` 指向法线方向 ⇒ wgpu 默认的 CCW 正面就是外侧。
struct Face {
    normal: [f32; 3],
    neighbor: [i32; 3],
    corners: [[f32; 3]; 4],
    uv_axes: [usize; 2],
}

const FACES: [Face; 6] = [
    Face {
        normal: [1.0, 0.0, 0.0],
        neighbor: [1, 0, 0],
        corners: [
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ],
        uv_axes: [2, 1],
    },
    Face {
        normal: [-1.0, 0.0, 0.0],
        neighbor: [-1, 0, 0],
        corners: [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
        ],
        uv_axes: [2, 1],
    },
    Face {
        normal: [0.0, 1.0, 0.0],
        neighbor: [0, 1, 0],
        corners: [
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
        uv_axes: [0, 2],
    },
    Face {
        normal: [0.0, -1.0, 0.0],
        neighbor: [0, -1, 0],
        corners: [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
        uv_axes: [0, 2],
    },
    Face {
        normal: [0.0, 0.0, 1.0],
        neighbor: [0, 0, 1],
        corners: [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        uv_axes: [0, 1],
    },
    Face {
        normal: [0.0, 0.0, -1.0],
        neighbor: [0, 0, -1],
        corners: [
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
        ],
        uv_axes: [0, 1],
    },
];

/// 一条 `__cells__` 规格：区域（格子边长 + 左下角 + 三轴格数）与实心位图。
#[derive(Clone, Debug, PartialEq)]
pub struct CellMeshSpec {
    /// 一格的世界边长（正方体，三轴同尺寸）。
    pub cell: f32,
    /// 区域左下角（世界坐标，也就是格 `(0,0,0)` 的角）。
    pub origin: [f32; 3],
    /// 三轴格数 `(w, h, d)` = `(x, y, z)`。
    pub dims: [u32; 3],
    /// 出双面网格（`ds=1`）；关 ⇒ 单面，背面第一遍就被硬件剔除。
    pub double_sided: bool,
    /// 实心位图，长度恒为 `(w*h*d + 7) / 8`，末字节未用位必须为 0。
    pub bits: Vec<u8>,
}

impl Default for CellMeshSpec {
    fn default() -> Self {
        Self {
            cell: 1.0,
            origin: [0.0, 0.0, 0.0],
            dims: [1, 1, 1],
            double_sided: false,
            bits: vec![0],
        }
    }
}

impl CellMeshSpec {
    /// 一格区域：`cell` 边长、左下角 `origin`、三轴格数 `dims`，全空。
    #[must_use]
    pub fn new(cell: f32, origin: [f32; 3], dims: [u32; 3]) -> Self {
        let total = (dims[0] as usize) * (dims[1] as usize) * (dims[2] as usize);
        Self {
            cell,
            origin,
            dims,
            double_sided: false,
            bits: vec![0u8; total.div_ceil(8)],
        }
    }

    /// True when a `MeshRenderer.mesh` string asks for the built-in cell body.
    #[must_use]
    pub fn is_cells_path(path: &str) -> bool {
        path.starts_with(CELLS_PREFIX)
    }

    /// Parses a `__cells__?...` mesh path; every field is validated, unknown
    /// fields are rejected rather than silently ignored.
    pub fn parse(path: &str) -> Result<Self, CellMeshError> {
        let query = path
            .strip_prefix(CELLS_PREFIX)
            .ok_or(CellMeshError::MissingPrefix)?;
        let query = query
            .strip_prefix('?')
            .ok_or_else(|| CellMeshError::MalformedField(path.to_owned()))?;
        let mut spec = Self {
            cell: f32::NAN,
            origin: [f32::NAN; 3],
            dims: [0, 0, 0],
            double_sided: false,
            bits: Vec::new(),
        };
        let mut has_bits = false;
        for field in query.split('&').filter(|field| !field.is_empty()) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| CellMeshError::MalformedField(field.to_owned()))?;
            match key {
                "cell" => spec.cell = parse_number(raw, key)?,
                "ox" => spec.origin[0] = parse_number(raw, key)?,
                "oy" => spec.origin[1] = parse_number(raw, key)?,
                "oz" => spec.origin[2] = parse_number(raw, key)?,
                "w" => spec.dims[0] = parse_axis(raw, key)?,
                "h" => spec.dims[1] = parse_axis(raw, key)?,
                "d" => spec.dims[2] = parse_axis(raw, key)?,
                // 只认 0/1，与地形的 `sect`/`cheap` 同规则：未知语义不悄悄当开。
                "ds" => {
                    let value = parse_number(raw, key)?;
                    if value != 0.0 && value != 1.0 {
                        return Err(CellMeshError::MalformedField(field.to_owned()));
                    }
                    spec.double_sided = value == 1.0;
                }
                "bits" => {
                    spec.bits = decode_bits(raw)?;
                    has_bits = true;
                }
                _ => return Err(CellMeshError::UnknownField(key.to_owned())),
            }
        }
        if !has_bits {
            return Err(CellMeshError::MissingField("bits"));
        }
        spec.validate()?;
        Ok(spec)
    }

    /// Region + cell size checks, plus the exact bitmap length the dims imply.
    pub fn validate(&self) -> Result<(), CellMeshError> {
        if !self.cell.is_finite()
            || !(CELLS_MIN_SIZE..=CELLS_MAX_SIZE).contains(&self.cell)
            || self.origin.iter().any(|value| !value.is_finite())
        {
            return Err(CellMeshError::SizeOutOfRange);
        }
        for axis in self.dims {
            if axis == 0 || axis > CELLS_MAX_PER_AXIS {
                return Err(CellMeshError::AxisOutOfRange(axis));
            }
        }
        let total = self.total_cells();
        if total > CELLS_MAX_TOTAL {
            return Err(CellMeshError::TooManyCells(total));
        }
        if self.bits.len() != total.div_ceil(8) {
            return Err(CellMeshError::BitmapLength {
                expected: total.div_ceil(8),
                found: self.bits.len(),
            });
        }
        let spare = total % 8;
        if spare != 0 {
            let used_mask = (1u8 << spare) - 1;
            if let Some(&last) = self.bits.last()
                && last & !used_mask != 0
            {
                return Err(CellMeshError::NonCanonicalBits);
            }
        }
        Ok(())
    }

    /// Canonical mesh path; also the renderer's geometry cache key.
    #[must_use]
    pub fn mesh_path(&self) -> String {
        let mut path = format!(
            "{CELLS_PREFIX}?cell={}&ox={}&oy={}&oz={}&w={}&h={}&d={}",
            self.cell,
            self.origin[0],
            self.origin[1],
            self.origin[2],
            self.dims[0],
            self.dims[1],
            self.dims[2]
        );
        if self.double_sided {
            path.push_str("&ds=1");
        }
        path.push_str(&format!("&bits={}", encode_bits(&self.bits)));
        path
    }

    #[must_use]
    pub fn total_cells(&self) -> usize {
        (self.dims[0] as usize) * (self.dims[1] as usize) * (self.dims[2] as usize)
    }

    /// Flat cell index for `(x, y, z)`; x-fastest, then z, then y (layer-major).
    /// Out-of-region coordinates return `None` rather than wrapping.
    #[must_use]
    pub fn index(&self, x: u32, y: u32, z: u32) -> Option<usize> {
        if x >= self.dims[0] || y >= self.dims[1] || z >= self.dims[2] {
            return None;
        }
        Some(
            (x as usize)
                + (self.dims[0] as usize) * ((z as usize) + (self.dims[2] as usize) * (y as usize)),
        )
    }

    /// `(x, y, z)` of a flat index, inverse of [`Self::index`].
    #[must_use]
    pub fn xyz(&self, index: usize) -> Option<(u32, u32, u32)> {
        if index >= self.total_cells() {
            return None;
        }
        let w = self.dims[0] as usize;
        let d = self.dims[2] as usize;
        let x = index % w;
        let rest = index / w;
        let z = rest % d;
        let y = rest / d;
        Some((x as u32, y as u32, z as u32))
    }

    #[must_use]
    pub fn is_solid(&self, index: usize) -> bool {
        let byte = index / 8;
        let bit = index % 8;
        self.bits
            .get(byte)
            .is_some_and(|value| value & (1 << bit) != 0)
    }

    #[must_use]
    pub fn solid_at(&self, x: u32, y: u32, z: u32) -> bool {
        self.index(x, y, z)
            .is_some_and(|index| self.is_solid(index))
    }

    /// 出实心格数：面数、顶点数与路径长度都随它变，是门里最省事的读数。
    #[must_use]
    pub fn solid_count(&self) -> usize {
        (0..self.total_cells())
            .filter(|index| self.is_solid(*index))
            .count()
    }

    /// Sets one cell; the caller then asks for a fresh `mesh_path()`. Bounds are
    /// checked rather than clamped so a wrong index cannot dig a neighbour.
    pub fn set_cell(&mut self, index: usize, solid: bool) -> Result<(), CellMeshError> {
        if index >= self.total_cells() {
            return Err(CellMeshError::IndexOutOfRange(index));
        }
        let byte = index / 8;
        let bit = index % 8;
        if solid {
            self.bits[byte] |= 1 << bit;
        } else {
            self.bits[byte] &= !(1 << bit);
        }
        Ok(())
    }

    /// 区域的世界 AABB（不含逐格取舍，偏保守）：拾取与剔除都按它算，
    /// 所以挖空一角不会让网格被误剔，代价是空区域仍占一份包围盒。
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let span = [
            self.cell * self.dims[0] as f32,
            self.cell * self.dims[1] as f32,
            self.cell * self.dims[2] as f32,
        ];
        (
            [self.origin[0], self.origin[1], self.origin[2]],
            [
                self.origin[0] + span[0],
                self.origin[1] + span[1],
                self.origin[2] + span[2],
            ],
        )
    }

    /// 暴露面数（= 索引数 / 6 = 顶点数 / 4）；相邻实心格之间的面不计。
    #[must_use]
    pub fn face_count(&self) -> usize {
        let mut faces = 0usize;
        for face in &FACES {
            for y in 0..self.dims[1] {
                for z in 0..self.dims[2] {
                    for x in 0..self.dims[0] {
                        if !self.solid_at(x, y, z) {
                            continue;
                        }
                        if self.solid_neighbor(x, y, z, face) {
                            continue;
                        }
                        faces += 1;
                    }
                }
            }
        }
        faces
    }

    fn solid_neighbor(&self, x: u32, y: u32, z: u32, face: &Face) -> bool {
        let nx = x as i64 + i64::from(face.neighbor[0]);
        let ny = y as i64 + i64::from(face.neighbor[1]);
        let nz = z as i64 + i64::from(face.neighbor[2]);
        if nx < 0 || ny < 0 || nz < 0 {
            return false;
        }
        self.solid_at(nx as u32, ny as u32, nz as u32)
    }

    /// 生成只含暴露面的实心体网格。确定性契约：同一规格两次调用逐字节相同。
    #[must_use]
    pub fn build_mesh(&self) -> MeshAsset {
        let faces = self.face_count();
        let mut vertices = Vec::with_capacity(faces * 4);
        let mut indices = Vec::with_capacity(faces * 6);
        for y in 0..self.dims[1] {
            for z in 0..self.dims[2] {
                for x in 0..self.dims[0] {
                    if !self.solid_at(x, y, z) {
                        continue;
                    }
                    let corner = [
                        self.origin[0] + x as f32 * self.cell,
                        self.origin[1] + y as f32 * self.cell,
                        self.origin[2] + z as f32 * self.cell,
                    ];
                    for face in &FACES {
                        if self.solid_neighbor(x, y, z, face) {
                            continue;
                        }
                        let base = vertices.len() as u32;
                        for offset in face.corners {
                            let position = [
                                corner[0] + offset[0] * self.cell,
                                corner[1] + offset[1] * self.cell,
                                corner[2] + offset[2] * self.cell,
                            ];
                            vertices.push(MeshVertex {
                                position,
                                normal: face.normal,
                                texcoord: [position[face.uv_axes[0]], position[face.uv_axes[1]]],
                                tangent: [1.0, 0.0, 0.0, 1.0],
                                joints: [0; 4],
                                weights: [1.0, 0.0, 0.0, 0.0],
                            });
                        }
                        indices.extend_from_slice(&[
                            base,
                            base + 1,
                            base + 2,
                            base,
                            base + 2,
                            base + 3,
                        ]);
                    }
                }
            }
        }
        let material = MaterialAsset {
            base_color: [0.42, 0.33, 0.24, 1.0],
            metallic: 0.0,
            roughness: 0.95,
            double_sided: self.double_sided,
            ..MaterialAsset::default()
        };
        MeshAsset {
            name: Some(CELLS_PREFIX.to_owned()),
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

impl fmt::Display for CellMeshSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.mesh_path())
    }
}

fn parse_number(raw: &str, key: &str) -> Result<f32, CellMeshError> {
    raw.parse::<f32>()
        .map_err(|_| CellMeshError::MalformedField(format!("{key}={raw}")))
}

fn parse_axis(raw: &str, key: &str) -> Result<u32, CellMeshError> {
    let value = parse_number(raw, key)?;
    if !value.is_finite() || value.fract() != 0.0 || value <= 0.0 {
        return Err(CellMeshError::MalformedField(format!("{key}={raw}")));
    }
    Ok(value as u32)
}

/// 位图 → base64url（无填充）：字符集刻意不含 `&`/`=`/`:`/`+`/`/`，能原样进网格路径。
fn encode_bits(bits: &[u8]) -> String {
    let mut out = String::with_capacity(bits.len().div_ceil(3) * 4);
    for chunk in bits.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let packed = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(BASE64_URL[(packed >> 18) as usize & 63] as char);
        out.push(BASE64_URL[(packed >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(BASE64_URL[(packed >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64_URL[(packed & 63) as usize] as char);
        }
    }
    out
}

/// base64url → 位图；未知字符、掉尾字符与长度不符都直接报错，不做宽容解码。
fn decode_bits(raw: &str) -> Result<Vec<u8>, CellMeshError> {
    let values: Vec<u32> = raw
        .chars()
        .map(|ch| {
            let index = ch as usize;
            if index >= 128 {
                return Err(CellMeshError::BadBitmap);
            }
            BASE64_URL
                .iter()
                .position(|marker| *marker as usize == index)
                .map(|position| position as u32)
                .ok_or(CellMeshError::BadBitmap)
        })
        .collect::<Result<_, _>>()?;
    let mut out = Vec::with_capacity(values.len() / 4 * 3 + 1);
    for chunk in values.chunks(4) {
        let packed = match chunk.len() {
            4 => (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6) | chunk[3],
            3 => (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6),
            2 => (chunk[0] << 18) | (chunk[1] << 12),
            1 => return Err(CellMeshError::BadBitmap),
            _ => 0,
        };
        out.push((packed >> 16) as u8);
        if chunk.len() > 2 {
            out.push((packed >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(packed as u8);
        }
    }
    Ok(out)
}

/// `__cells__` 规格的解析/校验失败。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CellMeshError {
    #[error("cells mesh path must start with {CELLS_PREFIX}")]
    MissingPrefix,
    #[error("malformed cells field: {0}")]
    MalformedField(String),
    #[error("unknown cells field: {0}")]
    UnknownField(String),
    #[error("cells field is required: {0}")]
    MissingField(&'static str),
    #[error("cells bitmap is not valid base64url")]
    BadBitmap,
    #[error("cells edge length or origin out of range")]
    SizeOutOfRange,
    #[error("cells per axis must be 1..={CELLS_MAX_PER_AXIS}: {0}")]
    AxisOutOfRange(u32),
    #[error("cells region must be at most {CELLS_MAX_TOTAL}: {0}")]
    TooManyCells(usize),
    #[error("cells bitmap length mismatch: expected {expected}, found {found}")]
    BitmapLength { expected: usize, found: usize },
    #[error("cells bitmap has non-zero bits past the region")]
    NonCanonicalBits,
    #[error("cells index out of region: {0}")]
    IndexOutOfRange(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_region(dims: [u32; 3]) -> CellMeshSpec {
        let mut spec = CellMeshSpec::new(1.0, [0.0, 0.0, 0.0], dims);
        for index in 0..spec.total_cells() {
            spec.set_cell(index, true).unwrap();
        }
        spec
    }

    fn with_indices(dims: [u32; 3], indices: &[usize]) -> CellMeshSpec {
        let mut spec = CellMeshSpec::new(1.0, [0.0, 0.0, 0.0], dims);
        for &index in indices {
            spec.set_cell(index, true).unwrap();
        }
        spec
    }

    /// 几何法线 = 顶点法线，而且朝外：每个三角形都按 `(p1-p0)×(p2-p0)` 复算。
    fn assert_outward_winding(mesh: &MeshAsset) {
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
            let geometric = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            for (axis, &geometric_axis) in geometric.iter().enumerate() {
                assert_eq!(
                    geometric_axis > 0.0,
                    a.normal[axis] > 0.0,
                    "triangle {triangle:?} winding disagrees with normal {:?}",
                    a.normal
                );
            }
        }
    }

    #[test]
    fn single_cell_makes_a_closed_box() {
        let spec = with_indices([1, 1, 1], &[0]);
        assert_eq!(spec.face_count(), 6);
        let mesh = spec.build_mesh();
        let primitive = &mesh.primitives[0];
        assert_eq!(primitive.vertices.len(), 24);
        assert_eq!(primitive.indices.len(), 36);
        assert_outward_winding(&mesh);
    }

    #[test]
    fn shared_faces_are_culled() {
        // 面数正比表面积而非体积：这三条是「实心体不等于六面×体积」的契约。
        assert_eq!(with_indices([2, 1, 1], &[0, 1]).face_count(), 10);
        assert_eq!(box_region([2, 2, 2]).face_count(), 24);
        assert_eq!(box_region([3, 3, 3]).face_count(), 54);
    }

    #[test]
    fn dug_cell_reveals_its_six_inner_faces() {
        let mut spec = box_region([3, 3, 3]);
        let center = spec.index(1, 1, 1).unwrap();
        spec.set_cell(center, false).unwrap();
        // 外壳 54 面不变，腔室多 6 面。
        assert_eq!(spec.face_count(), 60);
        assert_eq!(spec.solid_count(), 26);
    }

    #[test]
    fn normals_are_axis_aligned_and_indices_round_trip() {
        let spec = box_region([2, 2, 2]);
        let mesh = spec.build_mesh();
        for vertex in &mesh.primitives[0].vertices {
            let nonzero = vertex.normal.iter().filter(|n| **n != 0.0).count();
            assert_eq!(nonzero, 1, "normal {:?}", vertex.normal);
            assert!(
                vertex.normal.iter().any(|n| *n == 1.0 || *n == -1.0),
                "normal {:?}",
                vertex.normal
            );
        }
        assert_outward_winding(&mesh);
        for index in 0..spec.total_cells() {
            let (x, y, z) = spec.xyz(index).unwrap();
            assert_eq!(spec.index(x, y, z), Some(index));
        }
        assert_eq!(spec.index(2, 0, 0), None);
    }

    #[test]
    fn uv_is_world_planar_per_face_axis() {
        // 只留 (0,0,0) 一格：顶面的 uv 就是 (x, z)，所以四条角是 (0,0)(0,1)(1,1)(1,0)，
        // cell=1 时一格恰好铺满一个贴图瓦片、跨格连续不在格边接缝。
        let spec = with_indices([2, 1, 1], &[0]);
        let mesh = spec.build_mesh();
        let mut top: Vec<[f32; 2]> = mesh.primitives[0]
            .vertices
            .iter()
            .filter(|v| v.normal[1] > 0.5)
            .map(|v| v.texcoord)
            .collect();
        top.sort_by(|a, b| {
            a[0].partial_cmp(&b[0])
                .unwrap_or(core::cmp::Ordering::Equal)
                .then(
                    a[1].partial_cmp(&b[1])
                        .unwrap_or(core::cmp::Ordering::Equal),
                )
        });
        assert_eq!(top.len(), 4, "one quad on the top face");
        assert!(top.iter().all(|uv| uv[0] >= 0.0 && uv[0] <= 1.0));
        assert!(top.iter().all(|uv| uv[1] >= 0.0 && uv[1] <= 1.0));
        assert!(top.iter().any(|uv| uv[0] == 0.0 && uv[1] == 0.0));
        assert!(top.iter().any(|uv| uv[0] == 1.0 && uv[1] == 1.0));
        // 侧面走另外两条轴：+X 面的 uv 必须是 (z, y)，与顶面不同轴。
        let side = mesh.primitives[0]
            .vertices
            .iter()
            .find(|v| v.normal[0] > 0.5 && v.position[2] == 0.0 && v.position[1] == 0.0)
            .expect("the +X face corner at z=0,y=0");
        assert_eq!(side.texcoord, [0.0, 0.0]);
        let side_far = mesh.primitives[0]
            .vertices
            .iter()
            .find(|v| v.normal[0] > 0.5 && v.position[2] == 1.0 && v.position[1] == 1.0)
            .expect("the +X face corner at z=1,y=1");
        assert_eq!(side_far.texcoord, [1.0, 1.0]);
    }

    #[test]
    fn bounds_cover_the_region_even_when_dug_out() {
        let mut spec = CellMeshSpec::new(2.0, [-6.0, 0.0, 3.0], [3, 2, 4]);
        spec.set_cell(0, true).unwrap();
        let (min, max) = spec.bounds();
        assert_eq!(min, [-6.0, 0.0, 3.0]);
        assert_eq!(max, [0.0, 4.0, 11.0]);
        // 挖空唯一实心格 ⇒ 没有几何，但包围盒仍按区域给：剔除不会因此抖动。
        spec.set_cell(0, false).unwrap();
        assert_eq!(spec.face_count(), 0);
        assert_eq!(spec.bounds(), ([-6.0, 0.0, 3.0], [0.0, 4.0, 11.0]));
    }

    #[test]
    fn mesh_building_is_deterministic() {
        let spec = box_region([3, 3, 3]);
        let a = spec.build_mesh();
        let b = spec.build_mesh();
        assert_eq!(a.primitives[0].vertices, b.primitives[0].vertices);
        assert_eq!(a.primitives[0].indices, b.primitives[0].indices);
    }

    #[test]
    fn spec_path_round_trips() {
        let mut spec = box_region([4, 3, 2]);
        spec.origin = [-1.5, 0.25, 2.0];
        spec.cell = 0.75;
        spec.set_cell(5, false).unwrap();
        let parsed = CellMeshSpec::parse(&spec.mesh_path()).expect("canonical path parses");
        assert_eq!(parsed, spec);
        assert_eq!(parsed.mesh_path(), spec.mesh_path());
    }

    #[test]
    fn double_sided_is_opt_in() {
        let mut spec = box_region([2, 2, 2]);
        assert!(!spec.build_mesh().materials[0].double_sided);
        assert!(!spec.mesh_path().contains("&ds="));
        spec.double_sided = true;
        let parsed = CellMeshSpec::parse(&spec.mesh_path()).expect("ds path parses");
        assert!(parsed.double_sided);
        assert!(parsed.build_mesh().materials[0].double_sided);
        assert_eq!(parsed, spec);
    }

    #[test]
    fn non_canonical_specs_reject_their_own_path() {
        // 手写 `&bits=AAAA` 而不是 `mesh_path()`：区域不同必须红，不能悄悄当成全空。
        let forged = format!(
            "__cells__?cell=1&ox=0&oy=0&oz=0&w=4&h=4&d=4&bits={}",
            encode_bits(&[0])
        );
        assert!(matches!(
            CellMeshSpec::parse(&forged),
            Err(CellMeshError::BitmapLength { .. })
        ));
        // mesh_path() 从不发这种串，所以渲染器只会看到报错、不会看到半张网格。
        assert!(
            !CellMeshSpec::new(1.0, [0.0, 0.0, 0.0], [4, 4, 4])
                .mesh_path()
                .contains("&bits=AAAA&")
        );
    }

    #[test]
    fn malformed_specs_are_rejected() {
        let good = box_region([2, 2, 2]).mesh_path();
        assert!(matches!(
            CellMeshSpec::parse("wat?cell=1"),
            Err(CellMeshError::MissingPrefix)
        ));
        assert!(matches!(
            CellMeshSpec::parse(&good.replace("&w=", "&hue=")),
            Err(CellMeshError::UnknownField(_))
        ));
        assert!(matches!(
            CellMeshSpec::parse(good.split("&bits=").next().unwrap()),
            Err(CellMeshError::MissingField("bits"))
        ));
        // 截断的位图 ⇒ 长度与区域不符（1 字符解出 0 字节）。
        let truncated = {
            let (head, bits) = good.split_once("&bits=").expect("bits field");
            format!("{head}&bits={}", &bits[..bits.len() - 1])
        };
        assert!(CellMeshSpec::parse(&truncated).is_err());
        // 非规范位图：末字节的未用位被置起。
        let mut leaky = box_region([2, 2, 1]);
        let spare = leaky.total_cells() % 8;
        assert_ne!(spare, 0, "fixture needs a partial last byte");
        let last = leaky.bits.len() - 1;
        leaky.bits[last] |= 1 << spare;
        assert!(matches!(
            leaky.validate(),
            Err(CellMeshError::NonCanonicalBits)
        ));
        assert!(matches!(
            CellMeshSpec::new(1.0, [0.0; 3], [65, 1, 1]).validate(),
            Err(CellMeshError::AxisOutOfRange(65))
        ));
        assert!(matches!(
            CellMeshSpec::new(1.0, [0.0; 3], [40, 40, 40]).validate(),
            Err(CellMeshError::TooManyCells(64_000))
        ));
        assert!(CellMeshSpec::parse(&good.replace("&w=2", "&w=2.5")).is_err());
        assert!(matches!(
            leaky.set_cell(leaky.total_cells(), true),
            Err(CellMeshError::IndexOutOfRange(_))
        ));
        assert!(matches!(
            CellMeshSpec::parse(&good.replace("&w=2", "&w=0")),
            Err(CellMeshError::MalformedField(_))
        ));
        assert!(matches!(
            CellMeshSpec::parse(&good.replace("&w=2", "&w=65")),
            Err(CellMeshError::AxisOutOfRange(65))
        ));
    }

    #[test]
    fn empty_region_builds_no_faces() {
        let spec = CellMeshSpec::new(1.0, [0.0, 0.0, 0.0], [3, 3, 3]);
        assert_eq!(spec.face_count(), 0);
        assert_eq!(spec.solid_count(), 0);
        let mesh = spec.build_mesh();
        assert!(mesh.primitives[0].vertices.is_empty());
        assert!(mesh.primitives[0].indices.is_empty());
    }

    #[test]
    fn bitmap_encoding_uses_path_safe_alphabet() {
        let path = box_region([8, 4, 4]).mesh_path();
        let encoded = path.split("&bits=").nth(1).expect("bits field");
        assert!(
            encoded.chars().all(|ch| BASE64_URL.contains(&(ch as u8))),
            "bitmap escaped the url-safe alphabet: {encoded}"
        );
        assert!(!encoded.contains('='));
    }

    /// §21.500 接线前的尺寸核对：Myrmex 的画法规格（0.5 m 一格、21×9×21）落在文档承诺的上限内，
    /// 而且**面数在这个尺寸上仍然只随表面积走**——不剔除共用面会是 6×3969 = 23 814 面，而不是
    /// 1638 面；剖出的封闭空腔再按空腔自己的表面积加 54 面。规格串是这份几何的唯一状态
    /// （Lua 每帧比它、渲染器拿它当缓存键），所以长度也在这里定量，而不是留到真机上才发现。
    #[test]
    fn myrmex_sized_nest_stays_within_limits() {
        let mut spec = CellMeshSpec::new(0.5, [-5.25, -4.5, -5.25], [21, 9, 21]);
        assert_eq!(spec.total_cells(), 21 * 9 * 21);
        for index in 0..spec.total_cells() {
            spec.set_cell(index, true).unwrap();
        }
        spec.validate()
            .expect("Myrmex 的 21×9×21 / 0.5 m 应该在上限内");
        assert_eq!(spec.face_count(), 2 * (21 * 9 + 21 * 21 + 9 * 21));

        // 一间完全在内部的室（3×3×3）：挖掉它不减外表面，只加空腔的 6×9 面。
        for y in 4..7 {
            for z in 9..12 {
                for x in 9..12 {
                    spec.set_cell(spec.index(x, y, z).unwrap(), false).unwrap();
                }
            }
        }
        assert_eq!(spec.solid_count(), spec.total_cells() - 27);
        assert_eq!(spec.face_count(), 1638 + 54);

        let path = spec.mesh_path();
        println!(
            "myrmex_cells_reading: cells={} faces={} path_bytes={}",
            spec.solid_count(),
            spec.face_count(),
            path.len()
        );
        assert!(
            path.len() < 1024,
            "Myrmex 尺寸的规格串长度失控：{} 字节",
            path.len()
        );
        let parsed = CellMeshSpec::parse(&path).expect("Myrmex 尺寸的路径要能原样解析回来");
        assert_eq!(parsed, spec);
        assert!(!parsed.solid_at(10, 5, 10), "腔体在往返后必须还是空的");
        assert!(parsed.solid_at(0, 0, 0), "外壳在往返后必须还是实心的");
    }
}
