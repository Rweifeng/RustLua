//! CPU-side glTF 2.0 and GLB importer for M8.

use crate::math3d::Vec3;
use std::{error::Error, fmt, path::Path};

pub const IMPORTER_VERSION: &str = "gltf-m8-01";

#[derive(Clone, Debug, PartialEq)]
pub struct GltfAsset {
    pub meshes: Vec<GltfMesh>,
    pub nodes: Vec<GltfNode>,
    pub materials: Vec<GltfMaterial>,
    pub image_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfMesh {
    pub name: Option<String>,
    pub primitives: Vec<GltfPrimitive>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfPrimitive {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub texcoords: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfNode {
    pub name: Option<String>,
    pub mesh: Option<usize>,
    pub children: Vec<usize>,
    pub transform: [[f32; 4]; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfMaterial {
    pub name: Option<String>,
    pub base_color_factor: [f32; 4],
    pub base_color_texture: Option<usize>,
}

impl GltfAsset {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, GltfAssetError> {
        let path = path.as_ref();
        let (document, buffers, images) =
            gltf::import(path).map_err(|error| GltfAssetError::Import {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
        let materials = document
            .materials()
            .map(|material| {
                let pbr = material.pbr_metallic_roughness();
                GltfMaterial {
                    name: material.name().map(str::to_owned),
                    base_color_factor: pbr.base_color_factor(),
                    base_color_texture: pbr.base_color_texture().map(|info| info.texture().index()),
                }
            })
            .collect();
        let mut meshes = Vec::new();
        for mesh in document.meshes() {
            let mut primitives = Vec::new();
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    return Err(GltfAssetError::Unsupported {
                        path: path.display().to_string(),
                        detail: format!(
                            "mesh {} uses unsupported primitive mode {:?}",
                            mesh.index(),
                            primitive.mode()
                        ),
                    });
                }
                let reader =
                    primitive.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
                let positions = reader
                    .read_positions()
                    .ok_or_else(|| GltfAssetError::MissingAttribute {
                        path: path.display().to_string(),
                        attribute: "POSITION",
                    })?
                    .map(|value| Vec3::new(value[0], value[1], value[2]))
                    .collect::<Vec<_>>();
                if positions.is_empty() || positions.iter().any(|value| !value.is_finite()) {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "POSITION contains no valid vertices".to_owned(),
                    });
                }
                let normals: Vec<Vec3> = reader
                    .read_normals()
                    .map(|values| {
                        values
                            .map(|value| Vec3::new(value[0], value[1], value[2]))
                            .collect()
                    })
                    .unwrap_or_default();
                if !normals.is_empty() && normals.len() != positions.len() {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "NORMAL count differs from POSITION count".to_owned(),
                    });
                }
                let texcoords: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|values| values.into_f32().collect())
                    .unwrap_or_default();
                if !texcoords.is_empty() && texcoords.len() != positions.len() {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "TEXCOORD_0 count differs from POSITION count".to_owned(),
                    });
                }
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|values| values.into_u32().collect())
                    .unwrap_or_else(|| (0..positions.len() as u32).collect());
                if !indices.len().is_multiple_of(3)
                    || indices
                        .iter()
                        .any(|index| *index as usize >= positions.len())
                {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "indices are not triangles or are out of bounds".to_owned(),
                    });
                }
                primitives.push(GltfPrimitive {
                    positions,
                    normals,
                    texcoords,
                    indices,
                    material: primitive.material().index(),
                });
            }
            meshes.push(GltfMesh {
                name: mesh.name().map(str::to_owned),
                primitives,
            });
        }
        let nodes = document
            .nodes()
            .map(|node| GltfNode {
                name: node.name().map(str::to_owned),
                mesh: node.mesh().map(|mesh| mesh.index()),
                children: node.children().map(|child| child.index()).collect(),
                transform: node.transform().matrix(),
            })
            .collect();
        Ok(Self {
            meshes,
            nodes,
            materials,
            image_count: images.len(),
        })
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }
    pub fn primitive_count(&self) -> usize {
        self.meshes.iter().map(|mesh| mesh.primitives.len()).sum()
    }
    pub fn vertex_count(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .map(|primitive| primitive.positions.len())
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GltfAssetError {
    Import {
        path: String,
        reason: String,
    },
    MissingAttribute {
        path: String,
        attribute: &'static str,
    },
    InvalidMesh {
        path: String,
        detail: String,
    },
    Unsupported {
        path: String,
        detail: String,
    },
}

impl fmt::Display for GltfAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Import { path, reason } => {
                write!(formatter, "cannot import glTF {path}: {reason}")
            }
            Self::MissingAttribute { path, attribute } => write!(
                formatter,
                "glTF {path} is missing required attribute {attribute}"
            ),
            Self::InvalidMesh { path, detail } => {
                write!(formatter, "invalid glTF mesh {path}: {detail}")
            }
            Self::Unsupported { path, detail } => {
                write!(formatter, "unsupported glTF feature {path}: {detail}")
            }
        }
    }
}
impl Error for GltfAssetError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn asset_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../game/assets/models/m8-test-triangle.glb")
    }

    #[test]
    fn loads_self_authored_glb_mesh() {
        let asset = GltfAsset::load(asset_path()).unwrap();
        assert_eq!(asset.mesh_count(), 1);
        assert_eq!(asset.primitive_count(), 1);
        assert_eq!(asset.vertex_count(), 3);
        assert_eq!(asset.meshes[0].primitives[0].indices, vec![0, 1, 2]);
        assert_eq!(asset.nodes[0].mesh, Some(0));
    }

    #[test]
    fn rejects_invalid_glb_with_path_context() {
        let path = std::env::temp_dir().join(format!("p2-invalid-gltf-{}.glb", std::process::id()));
        fs::write(&path, b"not a glb").unwrap();
        let error = GltfAsset::load(&path).unwrap_err().to_string();
        assert!(error.contains("invalid-gltf") && error.contains("cannot import glTF"));
        let _ = fs::remove_file(path);
    }
}
