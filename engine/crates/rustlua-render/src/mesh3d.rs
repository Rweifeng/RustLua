//! GPU-friendly CPU mesh and material data for M8-03.

use std::path::Path;

use rustlua_assets::gltf_asset::{GltfAsset, GltfAssetError};
use rustlua_math::math3d::{Aabb3, Vec3};

// `.material` 的编解码住在运行时层（`rustlua-assets::mat_asset`），因为导入层校验必须用同一台
// 解析器又不能依赖本 crate。这里原样转发，渲染侧调用点（含 `pub use mesh3d::*`）不变。
pub use rustlua_assets::mat_asset::{
    MaterialAlphaMode, MaterialAsset, MaterialAssetError, MaterialTexture,
};

/// A 3D mesh renderer component attached to an entity, parsed from the
/// scene's `MeshRenderer` component string: `mesh=<relative glb/gltf path>`
/// with an optional `material=<relative .material path>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeshRenderer {
    /// Project-relative path to a `.glb`/`.gltf` mesh, or the built-in
    /// ground-plane sentinel `__ground_plane__`.
    pub mesh: String,
    /// Optional project-relative `.material` override; `None` uses the
    /// material embedded in the glTF.
    pub material: Option<String>,
}

/// The mesh path the renderer synthesizes a ground receiver plane for.
pub const GROUND_PLANE_SENTINEL: &str = "__ground_plane__";

/// Errors while parsing the scene's `MeshRenderer` component string.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MeshRendererError {
    /// A field was not `key=value` as the scene format requires.
    #[error("malformed MeshRenderer field: {0}")]
    MalformedField(String),
    /// The `mesh` field was missing or empty.
    #[error("missing MeshRenderer mesh path")]
    MissingMesh,
    /// The mesh path was absolute or escaped the project root.
    #[error("invalid MeshRenderer mesh path: {0}")]
    InvalidMeshPath(String),
    /// The mesh path did not end in `.glb`/`.gltf`.
    #[error("unsupported MeshRenderer mesh extension: {0}")]
    UnsupportedMeshExtension(String),
    /// The optional material path was not a project-relative `.material`.
    #[error("invalid MeshRenderer material path: {0}")]
    InvalidMaterialPath(String),
}

impl MeshRenderer {
    /// Parses the scene component string written by the editor:
    /// `mesh=<path>[;material=<path>]`.
    pub fn from_component_str(value: &str) -> Result<Self, MeshRendererError> {
        let mut mesh: Option<String> = None;
        let mut material: Option<String> = None;
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| MeshRendererError::MalformedField(field.to_owned()))?;
            match key {
                "mesh" => mesh = Some(raw.to_owned()),
                "material" if !raw.is_empty() => material = Some(raw.to_owned()),
                "material" => return Err(MeshRendererError::InvalidMaterialPath(raw.to_owned())),
                _ => return Err(MeshRendererError::MalformedField(field.to_owned())),
            }
        }
        let mesh = mesh.ok_or(MeshRendererError::MissingMesh)?;
        validate_mesh_path(&mesh)?;
        if let Some(material) = &material {
            validate_material_path(material)?;
        }
        Ok(Self { mesh, material })
    }

    /// Encodes the component back to the editor's string format.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        match &self.material {
            Some(material) => format!("mesh={};material={material}", self.mesh),
            None => format!("mesh={}", self.mesh),
        }
    }
}

/// 网格路径校验（`MeshRenderer` 与 P1-② `LodGroup` 共用；crate 内可见）。
pub(crate) fn validate_mesh_path(mesh: &str) -> Result<(), MeshRendererError> {
    if mesh.is_empty() {
        return Err(MeshRendererError::MissingMesh);
    }
    if mesh == GROUND_PLANE_SENTINEL {
        return Ok(());
    }
    // 与 `entity_script::EntityScript::validate` 共用 `rustlua_infra::paths` 的权威判定：
    // 只用 `Path::is_absolute()` 会放行 Windows 上的 `/etc/x.glb`（根相对但没有盘符），
    // schema 侧的 `FieldRule::ProjectPath` 用的是同一份实现（两边由对拍测试盯住）。
    if rustlua_infra::paths::is_absolute_path(mesh)
        || rustlua_infra::paths::escapes_project_root(mesh)
    {
        return Err(MeshRendererError::InvalidMeshPath(mesh.to_owned()));
    }
    let extension = Path::new(mesh)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "glb" && extension != "gltf" {
        return Err(MeshRendererError::UnsupportedMeshExtension(mesh.to_owned()));
    }
    Ok(())
}

fn validate_material_path(material: &str) -> Result<(), MeshRendererError> {
    if material.is_empty()
        || rustlua_infra::paths::is_absolute_path(material)
        || rustlua_infra::paths::escapes_project_root(material)
        || Path::new(material)
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("material"))
    {
        return Err(MeshRendererError::InvalidMaterialPath(material.to_owned()));
    }
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
    /// Tangent (xyz) + bitangent handedness (w). `w = 0` marks a vertex without a
    /// usable tangent basis (degenerate/mirrored UV or malformed authored data):
    /// the PBR shader then keeps the geometric normal and skips normal mapping.
    pub tangent: [f32; 4],
    /// Bone indices for GPU skinning; non-skinned vertices use `[0; 4]`.
    pub joints: [u32; 4],
    /// Bone weights for GPU skinning; non-skinned vertices use `[1, 0, 0, 0]`
    /// so bone 0 (identity) passes the vertex through.
    pub weights: [f32; 4],
}

/// Maximum bones the GPU skinning palette can hold.
pub const MAX_BONES: usize = 128;

/// Where a primitive's tangent basis came from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TangentBasis {
    /// Derived from positions/UVs; all vertices carry `w = ±1`.
    #[default]
    Generated,
    /// glTF TANGENT attribute was present and used as-is.
    Authored,
    /// UV degeneracy or mirrored winding made generation unreliable. All vertex
    /// tangents are zeroed (`w = 0`), so normal maps fall back to flat normals.
    Degenerate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshPrimitive {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// Whether the primitive carries real bone influences (JOINTS_0/WEIGHTS_0).
    pub skinned: bool,
    pub tangent_basis: TangentBasis,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshAsset {
    pub name: Option<String>,
    pub primitives: Vec<MeshPrimitive>,
    pub materials: Vec<MaterialAsset>,
}

/// 从三角形 UV 导数生成逐顶点切线（Gram-Schmidt 正交化 + 手性）。
///
/// UV 退化（|det|≈0）或同一图元内三角形手性不一致（镜像 UV）都会让生成结果
/// 互相抵消；这里**不硬算**，而是返回全零切线（`w=0`）标记 [`TangentBasis::Degenerate`]，
/// 让 PBR 着色器退回几何法线，避免法线贴图产生错误光照。
#[must_use]
pub fn generate_tangents(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    texcoords: &[[f32; 2]],
    indices: &[u32],
) -> (Vec<[f32; 4]>, TangentBasis) {
    let count = positions.len();
    if count == 0 {
        return (Vec::new(), TangentBasis::Generated);
    }
    let invalid = || (vec![[0.0; 4]; count], TangentBasis::Degenerate);
    if indices.is_empty() || !indices.len().is_multiple_of(3) {
        return invalid();
    }
    let mut tangent_sum = vec![Vec3::ZERO; count];
    let mut bitangent_sum = vec![Vec3::ZERO; count];
    let mut saw_positive = false;
    let mut saw_negative = false;
    for triangle in indices.chunks_exact(3) {
        let (i0, i1, i2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        if i0 >= count || i1 >= count || i2 >= count {
            return invalid();
        }
        let p0 = Vec3::new(positions[i0][0], positions[i0][1], positions[i0][2]);
        let edge1 = Vec3::new(positions[i1][0], positions[i1][1], positions[i1][2]) - p0;
        let edge2 = Vec3::new(positions[i2][0], positions[i2][1], positions[i2][2]) - p0;
        let duv1 = [
            texcoords[i1][0] - texcoords[i0][0],
            texcoords[i1][1] - texcoords[i0][1],
        ];
        let duv2 = [
            texcoords[i2][0] - texcoords[i0][0],
            texcoords[i2][1] - texcoords[i0][1],
        ];
        let determinant = duv1[0] * duv2[1] - duv2[0] * duv1[1];
        if !determinant.is_finite() || determinant.abs() <= 1e-12 {
            return invalid();
        }
        if determinant > 0.0 {
            saw_positive = true;
        } else {
            saw_negative = true;
        }
        let reciprocal = 1.0 / determinant;
        let tangent = (edge1 * duv2[1] - edge2 * duv1[1]) * reciprocal;
        let bitangent = (edge2 * duv1[0] - edge1 * duv2[0]) * reciprocal;
        for index in [i0, i1, i2] {
            tangent_sum[index] = tangent_sum[index] + tangent;
            bitangent_sum[index] = bitangent_sum[index] + bitangent;
        }
    }
    if saw_positive && saw_negative {
        return invalid();
    }
    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let normal = Vec3::new(normals[index][0], normals[index][1], normals[index][2]);
        let normal_length = normal.length();
        if !normal_length.is_finite() || normal_length <= 1e-12 {
            return invalid();
        }
        let normal = normal * (1.0 / normal_length);
        let tangent = tangent_sum[index] - normal * normal.dot(tangent_sum[index]);
        let tangent_length = tangent.length();
        if !tangent_length.is_finite() || tangent_length <= 1e-12 {
            return invalid();
        }
        let tangent = tangent * (1.0 / tangent_length);
        let handedness = normal.cross(tangent).dot(bitangent_sum[index]);
        if !handedness.is_finite() || handedness.abs() <= 1e-12 {
            return invalid();
        }
        output.push([
            tangent.x,
            tangent.y,
            tangent.z,
            if handedness < 0.0 { -1.0 } else { 1.0 },
        ]);
    }
    (output, TangentBasis::Generated)
}

/// 使用 glTF 自带的 TANGENT；零长度向量视为不可用（与生成失败同样回退）。
fn authored_tangents(tangents: &[[f32; 4]]) -> (Vec<[f32; 4]>, TangentBasis) {
    let mut output = Vec::with_capacity(tangents.len());
    for tangent in tangents {
        let xyz = Vec3::new(tangent[0], tangent[1], tangent[2]);
        let length = xyz.length();
        if !length.is_finite() || length <= 1e-12 {
            return (vec![[0.0; 4]; tangents.len()], TangentBasis::Degenerate);
        }
        let normalized = xyz * (1.0 / length);
        output.push([
            normalized.x,
            normalized.y,
            normalized.z,
            if tangent[3] < 0.0 { -1.0 } else { 1.0 },
        ]);
    }
    (output, TangentBasis::Authored)
}

impl MeshPrimitive {
    #[must_use]
    pub fn local_bounds(&self) -> Option<Aabb3> {
        Aabb3::from_points(
            &self
                .vertices
                .iter()
                .map(|vertex| Vec3::new(vertex.position[0], vertex.position[1], vertex.position[2]))
                .collect::<Vec<_>>(),
        )
    }
}

impl MeshAsset {
    #[must_use]
    pub fn local_bounds(&self) -> Option<Aabb3> {
        self.primitives
            .iter()
            .filter_map(MeshPrimitive::local_bounds)
            .reduce(Aabb3::union)
    }
    pub fn from_gltf(asset: &GltfAsset) -> Result<Vec<Self>, MeshAssetError> {
        asset
            .meshes
            .iter()
            .map(|mesh| {
                let primitives = mesh
                    .primitives
                    .iter()
                    .map(|primitive| {
                        let normals = if primitive.normals.is_empty() {
                            vec![Vec3::Y; primitive.positions.len()]
                        } else {
                            primitive.normals.clone()
                        };
                        let texcoords = if primitive.texcoords.is_empty() {
                            vec![[0.0, 0.0]; primitive.positions.len()]
                        } else {
                            primitive.texcoords.clone()
                        };
                        if primitive.positions.len() != normals.len()
                            || primitive.positions.len() != texcoords.len()
                            || !primitive.indices.len().is_multiple_of(3)
                            || primitive
                                .indices
                                .iter()
                                .any(|index| *index as usize >= primitive.positions.len())
                        {
                            return Err(MeshAssetError::InvalidPrimitive);
                        }
                        let skinned = !primitive.joints.is_empty();
                        let positions: Vec<[f32; 3]> = primitive
                            .positions
                            .iter()
                            .map(|position| [position.x, position.y, position.z])
                            .collect();
                        let normal_values: Vec<[f32; 3]> = normals
                            .iter()
                            .map(|normal| [normal.x, normal.y, normal.z])
                            .collect();
                        // 作者数据优先；缺失时从 UV 生成。生成失败会置零切线并标记
                        // `Degenerate`，由渲染层告警一次，法线贴图退回几何法线。
                        let (tangents, tangent_basis) = if primitive.tangents.is_empty() {
                            generate_tangents(
                                &positions,
                                &normal_values,
                                &texcoords,
                                &primitive.indices,
                            )
                        } else {
                            authored_tangents(&primitive.tangents)
                        };
                        let vertices = positions
                            .into_iter()
                            .zip(normals)
                            .zip(texcoords)
                            .enumerate()
                            .map(|(index, ((position, normal), texcoord))| {
                                let joints = if skinned {
                                    let raw = primitive.joints[index];
                                    [raw[0] as u32, raw[1] as u32, raw[2] as u32, raw[3] as u32]
                                } else {
                                    [0, 0, 0, 0]
                                };
                                let weights = if skinned {
                                    primitive.weights[index]
                                } else {
                                    [1.0, 0.0, 0.0, 0.0]
                                };
                                MeshVertex {
                                    position,
                                    normal: [normal.x, normal.y, normal.z],
                                    texcoord,
                                    tangent: tangents[index],
                                    joints,
                                    weights,
                                }
                            })
                            .collect();
                        Ok(MeshPrimitive {
                            vertices,
                            indices: primitive.indices.clone(),
                            material: primitive.material,
                            skinned,
                            tangent_basis,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self {
                    name: mesh.name.clone(),
                    primitives,
                    materials: asset.materials.iter().map(MaterialAsset::from).collect(),
                })
            })
            .collect()
    }

    pub fn from_gltf_path(path: impl AsRef<std::path::Path>) -> Result<Vec<Self>, MeshAssetError> {
        let asset = GltfAsset::load(path).map_err(MeshAssetError::Import)?;
        Self::from_gltf(&asset)
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum MeshAssetError {
    #[error("mesh import failed: {0}")]
    Import(#[source] GltfAssetError),
    #[error("mesh primitive has inconsistent attributes or indices")]
    InvalidPrimitive,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn computes_primitive_and_asset_local_bounds() {
        let primitive = MeshPrimitive {
            vertices: vec![
                MeshVertex {
                    position: [-2.0, 0.0, 1.0],
                    normal: [0.0, 1.0, 0.0],
                    texcoord: [0.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    joints: [0; 4],
                    weights: [1.0, 0.0, 0.0, 0.0],
                },
                MeshVertex {
                    position: [3.0, 4.0, -1.0],
                    normal: [0.0, 1.0, 0.0],
                    texcoord: [0.0, 0.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    joints: [0; 4],
                    weights: [1.0, 0.0, 0.0, 0.0],
                },
            ],
            indices: Vec::new(),
            material: None,
            skinned: false,
            tangent_basis: TangentBasis::Degenerate,
        };
        assert_eq!(
            primitive.local_bounds().unwrap().min,
            Vec3::new(-2.0, 0.0, -1.0)
        );
        let asset = MeshAsset {
            name: None,
            primitives: vec![primitive],
            materials: Vec::new(),
        };
        assert_eq!(asset.local_bounds().unwrap().max, Vec3::new(3.0, 4.0, 1.0));
    }
    #[test]
    fn parses_editor_component_string() {
        let renderer = MeshRenderer::from_component_str("mesh=assets/models/cube.glb").unwrap();
        assert_eq!(renderer.mesh, "assets/models/cube.glb");
        assert_eq!(renderer.to_component_str(), "mesh=assets/models/cube.glb");
        assert_eq!(
            MeshRenderer::from_component_str(&renderer.to_component_str()).unwrap(),
            renderer
        );
    }
    #[test]
    fn accepts_ground_plane_sentinel() {
        assert_eq!(
            MeshRenderer::from_component_str("mesh=__ground_plane__")
                .unwrap()
                .mesh,
            GROUND_PLANE_SENTINEL
        );
    }
    #[test]
    fn rejects_incomplete_or_invalid_component_strings() {
        assert!(matches!(
            MeshRenderer::from_component_str("mesh="),
            Err(MeshRendererError::MissingMesh)
        ));
        assert!(matches!(
            MeshRenderer::from_component_str("mesh=../escape.glb"),
            Err(MeshRendererError::InvalidMeshPath(_))
        ));
        assert!(matches!(
            MeshRenderer::from_component_str("mesh=assets/models/cube.obj"),
            Err(MeshRendererError::UnsupportedMeshExtension(_))
        ));
        assert!(matches!(
            MeshRenderer::from_component_str("file=assets/models/cube.glb"),
            Err(MeshRendererError::MalformedField(_))
        ));
    }

    fn asset_path() -> PathBuf {
        rustlua_infra::games_dir().join("examples/assets/models/m8-test-triangle.glb")
    }

    #[test]
    fn converts_gltf_to_interleaved_gpu_vertex_data() {
        let path = asset_path();
        if !path.is_file() {
            return;
        }
        let gltf = GltfAsset::load(path).unwrap();
        let meshes = MeshAsset::from_gltf(&gltf).unwrap();
        let primitive = &meshes[0].primitives[0];
        assert_eq!(primitive.vertices.len(), 3);
        assert_eq!(primitive.indices, vec![0, 1, 2]);
        assert_eq!(primitive.vertices[0].normal, [0.0, 1.0, 0.0]);
        assert!(meshes[0].materials.is_empty());
        assert_eq!(std::mem::size_of::<MeshVertex>(), 64);
        // Non-skinned primitives use the identity bone so skinning is a no-op.
        assert!(!primitive.skinned);
        assert_eq!(primitive.vertices[0].joints, [0, 0, 0, 0]);
        assert_eq!(primitive.vertices[0].weights, [1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn converts_skinned_gltf_with_bone_influences() {
        let path = rustlua_infra::games_dir().join("examples/assets/models/m8-skin-quad.glb");
        if !path.is_file() {
            return;
        }
        let gltf = GltfAsset::load(&path).unwrap();
        let meshes = MeshAsset::from_gltf(&gltf).unwrap();
        let primitive = &meshes[0].primitives[0];
        assert!(primitive.skinned);
        assert_eq!(primitive.vertices[1].joints, [0, 1, 0, 0]);
        assert_eq!(primitive.vertices[1].weights, [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn missing_normals_and_uvs_get_deterministic_defaults() {
        let path = asset_path();
        if !path.is_file() {
            return;
        }
        let gltf = GltfAsset::load(path).unwrap();
        let primitive = &MeshAsset::from_gltf(&gltf).unwrap()[0].primitives[0];
        assert!(
            primitive
                .vertices
                .iter()
                .all(|vertex| vertex.texcoord == [0.0, 0.0])
        );
    }

    #[test]
    fn mesh_renderer_carries_optional_material_override() {
        let renderer = MeshRenderer::from_component_str(
            "mesh=assets/models/cube.glb;material=assets/materials/rusted.material",
        )
        .unwrap();
        assert_eq!(
            renderer.material.as_deref(),
            Some("assets/materials/rusted.material")
        );
        assert_eq!(
            renderer.to_component_str(),
            "mesh=assets/models/cube.glb;material=assets/materials/rusted.material"
        );
        let reordered = MeshRenderer::from_component_str(
            "material=assets/materials/rusted.material;mesh=assets/models/cube.glb",
        )
        .unwrap();
        assert_eq!(reordered, renderer);
        assert_eq!(
            MeshRenderer::from_component_str("mesh=assets/models/cube.glb")
                .unwrap()
                .material,
            None
        );
        assert!(matches!(
            MeshRenderer::from_component_str("mesh=cube.glb;material=assets/rusted.txt"),
            Err(MeshRendererError::InvalidMaterialPath(_))
        ));
        assert!(matches!(
            MeshRenderer::from_component_str("mesh=cube.glb;material=../rusted.material"),
            Err(MeshRendererError::InvalidMaterialPath(_))
        ));
    }

    type QuadMesh = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>);

    fn quad() -> QuadMesh {
        (
            vec![
                [-2.0, 0.0, -2.0],
                [2.0, 0.0, -2.0],
                [2.0, 0.0, 2.0],
                [-2.0, 0.0, 2.0],
            ],
            vec![[0.0, 1.0, 0.0]; 4],
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            vec![0, 2, 1, 0, 3, 2],
        )
    }

    #[test]
    fn generates_orthonormal_tangents_with_handedness() {
        let (positions, normals, texcoords, indices) = quad();
        let (tangents, basis) = generate_tangents(&positions, &normals, &texcoords, &indices);
        assert_eq!(basis, TangentBasis::Generated);
        assert_eq!(tangents.len(), 4);
        for tangent in &tangents {
            assert!((tangent[0] - 1.0).abs() < 1e-6, "期望 +X 切线: {tangent:?}");
            assert!(tangent[1].abs() < 1e-6 && tangent[2].abs() < 1e-6);
            // 面朝 +Y、UV 的 v 轴朝 +Z：bitangent = cross(N,T)*w 必须指向 +Z。
            assert_eq!(tangent[3], -1.0);
        }
    }

    #[test]
    fn degenerate_uv_falls_back_to_flat_normals() {
        let (positions, normals, _, indices) = quad();
        let texcoords = vec![[0.0, 0.0]; 4];
        let (tangents, basis) = generate_tangents(&positions, &normals, &texcoords, &indices);
        assert_eq!(basis, TangentBasis::Degenerate);
        assert!(tangents.iter().all(|tangent| *tangent == [0.0; 4]));
    }

    #[test]
    fn mirrored_uv_triangles_fall_back_to_flat_normals() {
        let (positions, normals, mut texcoords, indices) = quad();
        // 第二个三角形翻转 u：同一图元内手性不一致，累积切线会互相抵消。
        texcoords[3] = [1.0, 1.0];
        texcoords[2] = [0.0, 1.0];
        let (tangents, basis) = generate_tangents(&positions, &normals, &texcoords, &indices);
        assert_eq!(basis, TangentBasis::Degenerate);
        assert!(tangents.iter().all(|tangent| *tangent == [0.0; 4]));
    }

    #[test]
    fn gltf_fixtures_report_tangent_basis() {
        let root = rustlua_infra::games_dir().join("examples/game_api/assets/models");
        let generated = MeshAsset::from_gltf_path(root.join("p3-r02-quad.gltf")).unwrap();
        assert_eq!(
            generated[0].primitives[0].tangent_basis,
            TangentBasis::Generated
        );
        assert!(
            generated[0].primitives[0]
                .vertices
                .iter()
                .all(|vertex| vertex.tangent[3] != 0.0)
        );
        let degenerate =
            MeshAsset::from_gltf_path(root.join("p3-r02-quad-degenerate.gltf")).unwrap();
        assert_eq!(
            degenerate[0].primitives[0].tangent_basis,
            TangentBasis::Degenerate
        );
        assert!(
            degenerate[0].primitives[0]
                .vertices
                .iter()
                .all(|vertex| vertex.tangent == [0.0; 4])
        );
        let authored = MeshAsset::from_gltf_path(root.join("p3-r02-material.gltf")).unwrap();
        assert_eq!(
            authored[0].primitives[0].tangent_basis,
            TangentBasis::Authored
        );
        assert_eq!(
            authored[0].primitives[0].vertices[0].tangent,
            [1.0, 0.0, 0.0, -1.0]
        );
    }

    #[test]
    fn authored_tangents_are_normalized_and_keep_handedness() {
        let (tangents, basis) = authored_tangents(&[[0.0, 0.0, 2.0, -1.0], [0.0, 0.0, 1.0, 1.0]]);
        assert_eq!(basis, TangentBasis::Authored);
        assert_eq!(tangents[0], [0.0, 0.0, 1.0, -1.0]);
        assert_eq!(tangents[1], [0.0, 0.0, 1.0, 1.0]);
        let (degenerate, basis) = authored_tangents(&[[0.0, 0.0, 0.0, 1.0]]);
        assert_eq!(basis, TangentBasis::Degenerate);
        assert_eq!(degenerate, vec![[0.0; 4]]);
    }
}
