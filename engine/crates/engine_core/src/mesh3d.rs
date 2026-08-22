//! GPU-friendly CPU mesh and material data for M8-03.

use crate::{
    gltf_asset::{GltfAsset, GltfAssetError},
    math3d::Vec3,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshPrimitive {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
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
}

impl MeshAsset {
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
                        let vertices = primitive
                            .positions
                            .iter()
                            .zip(normals)
                            .zip(texcoords)
                            .map(|((position, normal), texcoord)| MeshVertex {
                                position: [position.x, position.y, position.z],
                                normal: [normal.x, normal.y, normal.z],
                                texcoord,
                            })
                            .collect();
                        Ok(MeshPrimitive {
                            vertices,
                            indices: primitive.indices.clone(),
                            material: primitive.material,
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

impl From<&crate::gltf_asset::GltfMaterial> for MaterialAsset {
    fn from(material: &crate::gltf_asset::GltfMaterial) -> Self {
        Self {
            name: material.name.clone(),
            base_color_factor: material.base_color_factor,
            base_color_texture: material.base_color_texture,
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

    fn asset_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../game/assets/models/m8-test-triangle.glb")
    }

    #[test]
    fn converts_gltf_to_interleaved_gpu_vertex_data() {
        let gltf = GltfAsset::load(asset_path()).unwrap();
        let meshes = MeshAsset::from_gltf(&gltf).unwrap();
        let primitive = &meshes[0].primitives[0];
        assert_eq!(primitive.vertices.len(), 3);
        assert_eq!(primitive.indices, vec![0, 1, 2]);
        assert_eq!(primitive.vertices[0].normal, [0.0, 1.0, 0.0]);
        assert!(meshes[0].materials.is_empty());
        assert_eq!(std::mem::size_of::<MeshVertex>(), 32);
    }

    #[test]
    fn missing_normals_and_uvs_get_deterministic_defaults() {
        let gltf = GltfAsset::load(asset_path()).unwrap();
        let primitive = &MeshAsset::from_gltf(&gltf).unwrap()[0].primitives[0];
        assert!(
            primitive
                .vertices
                .iter()
                .all(|vertex| vertex.texcoord == [0.0, 0.0])
        );
    }
}
