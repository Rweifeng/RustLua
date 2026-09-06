//! CPU-side glTF 2.0 and GLB importer for M8.

use crate::core::{math3d::Vec3, pack};
use std::{error::Error, fmt, path::Path};

pub const IMPORTER_VERSION: &str = "gltf-m8-07";

/// Identity matrix used for absent inverse bind matrices.
const IDENTITY_MAT4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[derive(Clone, Debug, PartialEq)]
pub struct GltfAsset {
    pub meshes: Vec<GltfMesh>,
    pub nodes: Vec<GltfNode>,
    pub materials: Vec<GltfMaterial>,
    pub skins: Vec<GltfSkin>,
    pub animations: Vec<GltfAnimation>,
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
    /// Skinned vertex bone indices (JOINTS_0); empty when the mesh is static.
    pub joints: Vec<[u16; 4]>,
    /// Skinned vertex bone weights (WEIGHTS_0); empty when the mesh is static.
    pub weights: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfNode {
    pub name: Option<String>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    pub children: Vec<usize>,
    /// Bind local transform as translation / rotation (quaternion) / scale.
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// A glTF skin: the joint node indices and their inverse bind matrices.
#[derive(Clone, Debug, PartialEq)]
pub struct GltfSkin {
    pub joints: Vec<usize>,
    pub inverse_bind_matrices: Vec<[[f32; 4]; 4]>,
}

/// The node property an animation channel drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GltfChannelPath {
    Translation,
    Rotation,
    Scale,
}

/// One animation channel: `values` are the animated local TRS at `times`.
/// Translation/scale values are `[x, y, z, 0]`; rotation values are quaternions.
#[derive(Clone, Debug, PartialEq)]
pub struct GltfChannel {
    pub node: usize,
    pub path: GltfChannelPath,
    pub times: Vec<f32>,
    pub values: Vec<[f32; 4]>,
    /// `true` = STEP interpolation (hold previous key), `false` = linear.
    pub step: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfAnimation {
    pub name: Option<String>,
    pub channels: Vec<GltfChannel>,
}

/// Decoded base-color texture pixels (RGBA8) ready for GPU upload.
#[derive(Clone, Debug, PartialEq)]
pub struct GltfTextureImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GltfMaterial {
    pub name: Option<String>,
    pub base_color_factor: [f32; 4],
    pub base_color_texture: Option<usize>,
    /// Decoded base-color texture pixels when the material references one.
    pub base_color_image: Option<GltfTextureImage>,
    pub metallic_factor: f32,
    pub roughness_factor: f32,
}

impl GltfAsset {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, GltfAssetError> {
        let path = path.as_ref();
        let raw = std::fs::read(path).map_err(|error| GltfAssetError::Import {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let decoded = pack::decode(&raw).map_err(|reason| GltfAssetError::Import {
            path: path.display().to_string(),
            reason,
        })?;
        let (document, buffers, images) = if decoded.starts_with(pack::PACK_MAGIC) {
            unreachable!("pack::decode removes pack magic")
        } else if raw.starts_with(pack::PACK_MAGIC) {
            gltf::import_slice(&decoded).map_err(|error| GltfAssetError::Import {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?
        } else {
            gltf::import(path).map_err(|error| GltfAssetError::Import {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?
        };
        let materials = document
            .materials()
            .map(|material| {
                let pbr = material.pbr_metallic_roughness();
                let base_color_texture =
                    pbr.base_color_texture().map(|info| info.texture().index());
                let base_color_image = base_color_texture.and_then(|tex_index| {
                    let image_index = document.textures().nth(tex_index)?.source().index();
                    let image = images.get(image_index)?;
                    Some(GltfTextureImage {
                        width: image.width,
                        height: image.height,
                        rgba: image.pixels.clone(),
                    })
                });
                GltfMaterial {
                    name: material.name().map(str::to_owned),
                    base_color_factor: pbr.base_color_factor(),
                    base_color_texture,
                    base_color_image,
                    metallic_factor: pbr.metallic_factor(),
                    roughness_factor: pbr.roughness_factor(),
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
                let joints: Vec<[u16; 4]> = reader
                    .read_joints(0)
                    .map(|values| values.into_u16().collect())
                    .unwrap_or_default();
                let weights: Vec<[f32; 4]> = reader
                    .read_weights(0)
                    .map(|values| values.into_f32().collect())
                    .unwrap_or_default();
                if joints.is_empty() != weights.is_empty() {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "skinned mesh must provide both JOINTS_0 and WEIGHTS_0".to_owned(),
                    });
                }
                if !joints.is_empty()
                    && (joints.len() != positions.len() || weights.len() != positions.len())
                {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "JOINTS_0/WEIGHTS_0 count differs from POSITION count".to_owned(),
                    });
                }
                primitives.push(GltfPrimitive {
                    positions,
                    normals,
                    texcoords,
                    indices,
                    material: primitive.material().index(),
                    joints,
                    weights,
                });
            }
            meshes.push(GltfMesh {
                name: mesh.name().map(str::to_owned),
                primitives,
            });
        }
        let nodes = document
            .nodes()
            .map(|node| {
                let (translation, rotation, scale) = node.transform().decomposed();
                GltfNode {
                    name: node.name().map(str::to_owned),
                    mesh: node.mesh().map(|mesh| mesh.index()),
                    skin: node.skin().map(|skin| skin.index()),
                    children: node.children().map(|child| child.index()).collect(),
                    translation,
                    rotation,
                    scale,
                }
            })
            .collect();
        let skins = document
            .skins()
            .map(|skin| {
                let joints: Vec<usize> = skin.joints().map(|node| node.index()).collect();
                let reader =
                    skin.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
                let inverse_bind_matrices: Vec<[[f32; 4]; 4]> = reader
                    .read_inverse_bind_matrices()
                    .map(|values| values.collect())
                    .unwrap_or_else(|| vec![IDENTITY_MAT4; joints.len()]);
                if inverse_bind_matrices.len() != joints.len() {
                    return Err(GltfAssetError::InvalidMesh {
                        path: path.display().to_string(),
                        detail: "skin inverse bind matrix count differs from joint count"
                            .to_owned(),
                    });
                }
                Ok(GltfSkin {
                    joints,
                    inverse_bind_matrices,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let animations = document
            .animations()
            .map(|animation| {
                let mut channels = Vec::new();
                for channel in animation.channels() {
                    let target = channel.target();
                    let channel_path = match target.property() {
                        gltf::animation::Property::Translation => GltfChannelPath::Translation,
                        gltf::animation::Property::Rotation => GltfChannelPath::Rotation,
                        gltf::animation::Property::Scale => GltfChannelPath::Scale,
                        gltf::animation::Property::MorphTargetWeights => {
                            return Err(GltfAssetError::Unsupported {
                                path: path.display().to_string(),
                                detail: "morph target animation channels are not supported"
                                    .to_owned(),
                            });
                        }
                    };
                    let interpolation = channel.sampler().interpolation();
                    if interpolation != gltf::animation::Interpolation::Linear
                        && interpolation != gltf::animation::Interpolation::Step
                    {
                        return Err(GltfAssetError::Unsupported {
                            path: path.display().to_string(),
                            detail: "only linear and step animation interpolation are supported"
                                .to_owned(),
                        });
                    }
                    let reader =
                        channel.reader(|buffer| buffers.get(buffer.index()).map(|data| &data[..]));
                    let times: Vec<f32> = reader
                        .read_inputs()
                        .ok_or_else(|| GltfAssetError::MissingAttribute {
                            path: path.display().to_string(),
                            attribute: "animation sampler input",
                        })?
                        .collect();
                    if times.is_empty() {
                        return Err(GltfAssetError::InvalidMesh {
                            path: path.display().to_string(),
                            detail: "animation sampler needs at least one keyframe".to_owned(),
                        });
                    }
                    let outputs =
                        reader
                            .read_outputs()
                            .ok_or_else(|| GltfAssetError::MissingAttribute {
                                path: path.display().to_string(),
                                attribute: "animation sampler output",
                            })?;
                    let values: Vec<[f32; 4]> = match (&channel_path, outputs) {
                        (
                            GltfChannelPath::Translation | GltfChannelPath::Scale,
                            gltf::animation::util::ReadOutputs::Translations(iter)
                            | gltf::animation::util::ReadOutputs::Scales(iter),
                        ) => iter
                            .map(|value| [value[0], value[1], value[2], 0.0])
                            .collect(),
                        (
                            GltfChannelPath::Rotation,
                            gltf::animation::util::ReadOutputs::Rotations(rotations),
                        ) => rotations.into_f32().collect(),
                        _ => {
                            return Err(GltfAssetError::InvalidMesh {
                                path: path.display().to_string(),
                                detail:
                                    "animation sampler output type does not match the channel path"
                                        .to_owned(),
                            });
                        }
                    };
                    if values.len() != times.len() {
                        return Err(GltfAssetError::InvalidMesh {
                            path: path.display().to_string(),
                            detail: "animation sampler output count differs from input count"
                                .to_owned(),
                        });
                    }
                    channels.push(GltfChannel {
                        node: target.node().index(),
                        path: channel_path,
                        times,
                        values,
                        step: interpolation == gltf::animation::Interpolation::Step,
                    });
                }
                Ok(GltfAnimation {
                    name: animation.name().map(str::to_owned),
                    channels,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            meshes,
            nodes,
            materials,
            skins,
            animations,
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
        crate::core::games_dir().join("game_showcase/assets/models/m8-test-triangle.glb")
    }

    #[test]
    fn loads_self_authored_glb_mesh() {
        let path = asset_path();
        if !path.is_file() {
            return;
        }
        let asset = GltfAsset::load(path).unwrap();
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

    fn skin_asset_path() -> PathBuf {
        crate::core::games_dir().join("game_showcase/assets/models/m8-skin-quad.glb")
    }

    #[test]
    fn imports_skinned_animated_quad() {
        let path = skin_asset_path();
        if !path.is_file() {
            return;
        }
        let asset = GltfAsset::load(path).unwrap();
        // Two joints and one "Bend" rotation channel on Joint1.
        assert_eq!(asset.skins.len(), 1);
        assert_eq!(asset.skins[0].joints, vec![1, 2]);
        assert_eq!(asset.skins[0].inverse_bind_matrices.len(), 2);
        assert_eq!(asset.animations.len(), 1);
        assert_eq!(asset.animations[0].name.as_deref(), Some("Bend"));
        assert_eq!(asset.animations[0].channels.len(), 1);
        let channel = &asset.animations[0].channels[0];
        assert_eq!(channel.node, 2);
        assert_eq!(channel.path, GltfChannelPath::Rotation);
        assert_eq!(channel.times, vec![0.0, 1.0]);
        assert_eq!(channel.values.len(), 2);
        // The mesh node is skinned by skin 0 and the primitive carries joints/weights.
        assert_eq!(asset.nodes[3].mesh, Some(0));
        assert_eq!(asset.nodes[3].skin, Some(0));
        let primitive = &asset.meshes[0].primitives[0];
        assert_eq!(primitive.joints.len(), 4);
        assert_eq!(primitive.weights.len(), 4);
        assert_eq!(primitive.joints[1], [0, 1, 0, 0]);
        assert_eq!(primitive.weights[1], [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn imports_cc0_kaykit_warrior_skin_and_animations() {
        let path = crate::core::games_dir().join("game_showcase/assets/models/kaykit-warrior.glb");
        if !path.is_file() {
            return;
        }
        let asset = GltfAsset::load(&path).unwrap();
        assert!(
            !asset.skins.is_empty(),
            "warrior should have at least one skin"
        );
        assert!(
            !asset.animations.is_empty(),
            "warrior should have animation clips"
        );
        // Every primitive that uses a skinned mesh must carry joints and weights.
        let skinned_primitives = asset
            .meshes
            .iter()
            .flat_map(|mesh| &mesh.primitives)
            .filter(|primitive| !primitive.joints.is_empty());
        assert!(
            skinned_primitives.count() > 0,
            "warrior should have skinned primitives"
        );
        // Animation channels target valid nodes with ordered keyframe times.
        for animation in &asset.animations {
            for channel in &animation.channels {
                assert!(channel.node < asset.nodes.len());
                for pair in channel.times.windows(2) {
                    assert!(pair[0] <= pair[1], "keyframe times must be non-decreasing");
                }
                assert_eq!(channel.times.len(), channel.values.len());
            }
        }
    }
}
