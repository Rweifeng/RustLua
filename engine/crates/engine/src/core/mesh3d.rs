//! GPU-friendly CPU mesh and material data for M8-03.

use std::{error::Error, fmt, path::Path};

use crate::core::{
    gltf_asset::{GltfAsset, GltfAssetError},
    math3d::{Aabb3, Vec3},
};

/// A 3D mesh renderer component attached to an entity, parsed from the
/// scene's `MeshRenderer` component string: `mesh=<relative glb/gltf path>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeshRenderer {
    /// Project-relative path to a `.glb`/`.gltf` mesh, or the built-in
    /// ground-plane sentinel `__ground_plane__`.
    pub mesh: String,
}

/// The mesh path the renderer synthesizes a ground receiver plane for.
pub const GROUND_PLANE_SENTINEL: &str = "__ground_plane__";

/// Errors while parsing the scene's `MeshRenderer` component string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeshRendererError {
    /// A field was not `key=value` as the scene format requires.
    MalformedField(String),
    /// The `mesh` field was missing or empty.
    MissingMesh,
    /// The mesh path was absolute or escaped the project root.
    InvalidMeshPath(String),
    /// The mesh path did not end in `.glb`/`.gltf`.
    UnsupportedMeshExtension(String),
}
impl fmt::Display for MeshRendererError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed MeshRenderer field: {field}"),
            Self::MissingMesh => write!(f, "missing MeshRenderer mesh path"),
            Self::InvalidMeshPath(value) => write!(f, "invalid MeshRenderer mesh path: {value}"),
            Self::UnsupportedMeshExtension(value) => {
                write!(f, "unsupported MeshRenderer mesh extension: {value}")
            }
        }
    }
}
impl Error for MeshRendererError {}

impl MeshRenderer {
    /// Parses the scene component string written by the editor: `mesh=<path>`.
    pub fn from_component_str(value: &str) -> Result<Self, MeshRendererError> {
        let mut mesh: Option<String> = None;
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| MeshRendererError::MalformedField(field.to_owned()))?;
            match key {
                "mesh" => mesh = Some(raw.to_owned()),
                _ => return Err(MeshRendererError::MalformedField(field.to_owned())),
            }
        }
        let mesh = mesh.ok_or(MeshRendererError::MissingMesh)?;
        validate_mesh_path(&mesh)?;
        Ok(Self { mesh })
    }

    /// Encodes the component back to the editor's string format.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!("mesh={}", self.mesh)
    }
}

fn validate_mesh_path(mesh: &str) -> Result<(), MeshRendererError> {
    if mesh.is_empty() {
        return Err(MeshRendererError::MissingMesh);
    }
    if mesh == GROUND_PLANE_SENTINEL {
        return Ok(());
    }
    // 与 `entity_script::EntityScript::validate` 共用 `core::paths` 的权威判定：
    // 只用 `Path::is_absolute()` 会放行 Windows 上的 `/etc/x.glb`（根相对但没有盘符），
    // schema 侧的 `FieldRule::ProjectPath` 用的是同一份实现（两边由对拍测试盯住）。
    if crate::core::paths::is_absolute_path(mesh) || crate::core::paths::escapes_project_root(mesh)
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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
    /// Bone indices for GPU skinning; non-skinned vertices use `[0; 4]`.
    pub joints: [u32; 4],
    /// Bone weights for GPU skinning; non-skinned vertices use `[1, 0, 0, 0]`
    /// so bone 0 (identity) passes the vertex through.
    pub weights: [f32; 4],
}

/// Maximum bones the GPU skinning palette can hold.
pub const MAX_BONES: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct MeshPrimitive {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// Whether the primitive carries real bone influences (JOINTS_0/WEIGHTS_0).
    pub skinned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshAsset {
    pub name: Option<String>,
    pub primitives: Vec<MeshPrimitive>,
    pub materials: Vec<MaterialAsset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialAsset {
    pub name: Option<String>,
    pub base_color_factor: [f32; 4],
    pub base_color_texture: Option<usize>,
    /// Decoded base-color texture pixels (RGBA8) when the material has one.
    pub base_color_image: Option<crate::core::gltf_asset::GltfTextureImage>,
    pub metallic_factor: f32,
    pub roughness_factor: f32,
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
                        let vertices = primitive
                            .positions
                            .iter()
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
                                    position: [position.x, position.y, position.z],
                                    normal: [normal.x, normal.y, normal.z],
                                    texcoord,
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

impl From<&crate::core::gltf_asset::GltfMaterial> for MaterialAsset {
    fn from(material: &crate::core::gltf_asset::GltfMaterial) -> Self {
        Self {
            name: material.name.clone(),
            base_color_factor: material.base_color_factor,
            base_color_texture: material.base_color_texture,
            base_color_image: material.base_color_image.clone(),
            metallic_factor: material.metallic_factor,
            roughness_factor: material.roughness_factor,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeshAssetError {
    Import(GltfAssetError),
    InvalidPrimitive,
}

impl std::fmt::Display for MeshAssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Import(error) => write!(formatter, "mesh import failed: {error}"),
            Self::InvalidPrimitive => write!(
                formatter,
                "mesh primitive has inconsistent attributes or indices"
            ),
        }
    }
}
impl std::error::Error for MeshAssetError {}

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
                    joints: [0; 4],
                    weights: [1.0, 0.0, 0.0, 0.0],
                },
                MeshVertex {
                    position: [3.0, 4.0, -1.0],
                    normal: [0.0, 1.0, 0.0],
                    texcoord: [0.0, 0.0],
                    joints: [0; 4],
                    weights: [1.0, 0.0, 0.0, 0.0],
                },
            ],
            indices: Vec::new(),
            material: None,
            skinned: false,
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
        crate::core::games_dir().join("examples/assets/models/m8-test-triangle.glb")
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
        let path = crate::core::games_dir().join("examples/assets/models/m8-skin-quad.glb");
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
}
