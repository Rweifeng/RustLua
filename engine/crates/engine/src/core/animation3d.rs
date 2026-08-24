//! 3D skeletal animation component for M8-07: bind-pose node hierarchy, a skin's
//! joint list with inverse bind matrices, and named clips. Each frame the renderer
//! asks for the joint palette produced by the active clip at the current time and
//! uploads it for GPU skinning.

use std::{error::Error, fmt, path::Path};

use crate::core::{
    gltf_asset::{GltfAsset, GltfChannelPath},
    math3d::{Mat4, Quat, Vec3},
};

/// One animated node property with keyframe times and values.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel3D {
    pub node: usize,
    pub path: GltfChannelPath,
    pub times: Vec<f32>,
    /// Translation/scale values are `[x, y, z, 0]`; rotation values are quaternions.
    pub values: Vec<[f32; 4]>,
    /// `true` holds the previous key (glTF STEP), `false` interpolates linearly.
    pub step: bool,
}

/// A named 3D animation clip (one glTF animation) with a computed duration.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimClip3D {
    pub name: String,
    pub channels: Vec<Channel3D>,
    pub duration: f32,
}

/// A skinned-animation component.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation3D {
    /// Parent node index per glTF document node (`None` = root).
    parent: Vec<Option<usize>>,
    bind_translation: Vec<Vec3>,
    bind_rotation: Vec<Quat>,
    bind_scale: Vec<Vec3>,
    /// Skin joint node indices into the document node list.
    joints: Vec<usize>,
    /// Inverse bind matrix per joint.
    inverse_bind: Vec<Mat4>,
    clips: Vec<AnimClip3D>,
    current: usize,
    time: f32,
    paused: bool,
    looped: bool,
}

/// Errors while building or driving a 3D animation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Animation3dError {
    MissingSkin(usize),
    EmptySkin,
    InvalidJoint(usize),
    UnknownClip(String),
    DuplicateClip(String),
    NotSkinned,
    MissingMesh,
    InvalidMeshPath(String),
    Import(String),
    Malformed(String),
}
impl fmt::Display for Animation3dError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSkin(index) => write!(f, "animation references missing skin {index}"),
            Self::EmptySkin => write!(f, "skin has no joints"),
            Self::InvalidJoint(index) => write!(f, "skin references invalid joint node {index}"),
            Self::UnknownClip(name) => write!(f, "unknown 3D animation clip: {name}"),
            Self::DuplicateClip(name) => write!(f, "duplicate 3D animation clip: {name}"),
            Self::NotSkinned => write!(f, "mesh is not skinned (no skin in the glTF asset)"),
            Self::MissingMesh => write!(f, "Animation3D component is missing its mesh path"),
            Self::InvalidMeshPath(value) => write!(f, "invalid Animation3D mesh path: {value}"),
            Self::Import(reason) => write!(f, "3D animation import failed: {reason}"),
            Self::Malformed(reason) => write!(f, "invalid Animation3D component string: {reason}"),
        }
    }
}
impl Error for Animation3dError {}

/// The `Animation3D` scene component value, parsed from the editor string:
/// `mesh=<path>;clip=<name>;looped=true|false`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Animation3dDescriptor {
    pub mesh: String,
    pub clip: String,
    pub looped: bool,
}

impl Animation3dDescriptor {
    pub fn from_component_str(value: &str) -> Result<Self, Animation3dError> {
        let mut mesh = None;
        let mut clip = None;
        let mut looped = true;
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| Animation3dError::Malformed(field.to_owned()))?;
            match key {
                "mesh" => mesh = Some(raw.to_owned()),
                "clip" => clip = Some(raw.to_owned()),
                "looped" => {
                    looped = match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(Animation3dError::Malformed(
                                "looped must be true/false".to_owned(),
                            ));
                        }
                    };
                }
                _ => return Err(Animation3dError::Malformed(field.to_owned())),
            }
        }
        Ok(Self {
            mesh: mesh.ok_or(Animation3dError::MissingMesh)?,
            clip: clip.ok_or_else(|| Animation3dError::Malformed("missing clip".to_owned()))?,
            looped,
        })
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "mesh={};clip={};looped={}",
            self.mesh, self.clip, self.looped
        )
    }
}

impl Animation3D {
    /// Builds a component from a loaded glTF asset: the bind pose hierarchy, the
    /// skin referenced by the first mesh node (falling back to skin 0), the
    /// skin's joints/inverse bind matrices, and every animation as a clip.
    pub fn from_gltf(
        asset: &GltfAsset,
        clip_name: &str,
        looped: bool,
    ) -> Result<Self, Animation3dError> {
        let skin_index = asset.nodes.iter().find_map(|node| node.skin).unwrap_or(0);
        let skin = asset
            .skins
            .get(skin_index)
            .ok_or(Animation3dError::NotSkinned)?;
        if skin.joints.is_empty() {
            return Err(Animation3dError::EmptySkin);
        }
        let mut parent = vec![None; asset.nodes.len()];
        for (index, node) in asset.nodes.iter().enumerate() {
            for &child in &node.children {
                if child >= asset.nodes.len() {
                    return Err(Animation3dError::InvalidJoint(child));
                }
                parent[child] = Some(index);
            }
        }
        let bind_translation = asset
            .nodes
            .iter()
            .map(|node| {
                Vec3::new(
                    node.translation[0],
                    node.translation[1],
                    node.translation[2],
                )
            })
            .collect();
        let bind_rotation = asset
            .nodes
            .iter()
            .map(|node| Quat {
                x: node.rotation[0],
                y: node.rotation[1],
                z: node.rotation[2],
                w: node.rotation[3],
            })
            .collect();
        let bind_scale = asset
            .nodes
            .iter()
            .map(|node| Vec3::new(node.scale[0], node.scale[1], node.scale[2]))
            .collect();
        for &joint in &skin.joints {
            if joint >= asset.nodes.len() {
                return Err(Animation3dError::InvalidJoint(joint));
            }
        }
        let inverse_bind = skin
            .inverse_bind_matrices
            .iter()
            .map(|matrix| Mat4 { columns: *matrix })
            .collect();
        let clips: Vec<AnimClip3D> = asset
            .animations
            .iter()
            .map(|animation| {
                let channels: Vec<Channel3D> = animation
                    .channels
                    .iter()
                    .map(|channel| Channel3D {
                        node: channel.node,
                        path: channel.path,
                        times: channel.times.clone(),
                        values: channel.values.clone(),
                        step: channel.step,
                    })
                    .collect();
                let duration = channels
                    .iter()
                    .filter_map(|channel| channel.times.last().copied())
                    .fold(0.0, f32::max);
                AnimClip3D {
                    name: animation.name.clone().unwrap_or_else(|| "clip".to_owned()),
                    channels,
                    duration,
                }
            })
            .collect();
        if clips.is_empty() {
            return Err(Animation3dError::Malformed(
                "glTF asset has no animation clips".to_owned(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for clip in &clips {
            if !seen.insert(clip.name.clone()) {
                return Err(Animation3dError::DuplicateClip(clip.name.clone()));
            }
        }
        let current = clips
            .iter()
            .position(|clip| clip.name == clip_name)
            .ok_or_else(|| Animation3dError::UnknownClip(clip_name.to_owned()))?;
        Ok(Self {
            parent,
            bind_translation,
            bind_rotation,
            bind_scale,
            joints: skin.joints.clone(),
            inverse_bind,
            clips,
            current,
            time: 0.0,
            paused: false,
            looped,
        })
    }

    /// Builds a component by loading a glTF mesh path from a project root.
    pub fn from_gltf_path(
        root: &Path,
        descriptor: &Animation3dDescriptor,
    ) -> Result<Self, Animation3dError> {
        let asset = GltfAsset::load(root.join(&descriptor.mesh))
            .map_err(|error| Animation3dError::Import(error.to_string()))?;
        Self::from_gltf(&asset, &descriptor.clip, descriptor.looped)
    }

    /// Switches the active clip, restarting time. Returns `true` on a change.
    pub fn set_clip(&mut self, name: &str) -> Result<bool, Animation3dError> {
        if self.current_clip().name == name {
            return Ok(false);
        }
        let index = self
            .clips
            .iter()
            .position(|clip| clip.name == name)
            .ok_or_else(|| Animation3dError::UnknownClip(name.to_owned()))?;
        self.current = index;
        self.time = 0.0;
        Ok(true)
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_looped(&mut self, looped: bool) {
        self.looped = looped;
    }

    #[must_use]
    pub fn current_clip(&self) -> &AnimClip3D {
        &self.clips[self.current]
    }

    #[must_use]
    pub fn current_name(&self) -> &str {
        &self.clips[self.current].name
    }

    /// Advances the active clip by `dt` seconds; looped clips wrap, non-looping
    /// clips clamp at their duration. Paused animations do not advance.
    pub fn advance(&mut self, delta_seconds: f32) {
        if self.paused || !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        let duration = self.current_clip().duration;
        self.time += delta_seconds;
        if self.looped && duration > 0.0 {
            self.time %= duration;
        } else {
            self.time = self.time.min(duration.max(0.0));
        }
    }

    #[must_use]
    pub fn time(&self) -> f32 {
        self.time
    }

    #[must_use]
    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    /// Lists the clip names this animation exposes.
    #[must_use]
    pub fn clip_names(&self) -> Vec<String> {
        self.clips.iter().map(|clip| clip.name.clone()).collect()
    }

    /// Computes the joint matrix palette for GPU skinning at the current time:
    /// `global(joint) * inverse_bind(joint)` per joint.
    #[must_use]
    pub fn joint_matrices(&self) -> Vec<Mat4> {
        let time = self.time;
        let mut cache = vec![None; self.parent.len()];
        self.joints
            .iter()
            .zip(&self.inverse_bind)
            .map(|(node, inverse_bind)| self.global_matrix(*node, time, &mut cache) * *inverse_bind)
            .collect()
    }

    /// Computes the bind-pose palette (used for shadow maps before animation).
    #[must_use]
    pub fn bind_palette(&self) -> Vec<Mat4> {
        let mut cache = vec![None; self.parent.len()];
        self.joints
            .iter()
            .zip(&self.inverse_bind)
            .map(|(node, inverse_bind)| self.bind_global(*node, &mut cache) * *inverse_bind)
            .collect()
    }

    fn global_matrix(&self, node: usize, time: f32, cache: &mut [Option<Mat4>]) -> Mat4 {
        if let Some(matrix) = cache[node] {
            return matrix;
        }
        let local = self.local_matrix(node, time);
        let result = match self.parent[node] {
            Some(parent) => self.global_matrix(parent, time, cache) * local,
            None => local,
        };
        cache[node] = Some(result);
        result
    }

    fn bind_global(&self, node: usize, cache: &mut [Option<Mat4>]) -> Mat4 {
        if let Some(matrix) = cache[node] {
            return matrix;
        }
        let local = Mat4::translation(self.bind_translation[node])
            * Mat4::from_quat(self.bind_rotation[node])
            * Mat4::scale(self.bind_scale[node]);
        let result = match self.parent[node] {
            Some(parent) => self.bind_global(parent, cache) * local,
            None => local,
        };
        cache[node] = Some(result);
        result
    }

    fn local_matrix(&self, node: usize, time: f32) -> Mat4 {
        let mut translation = self.bind_translation[node];
        let mut rotation = self.bind_rotation[node];
        let mut scale = self.bind_scale[node];
        for channel in &self.clips[self.current].channels {
            if channel.node != node {
                continue;
            }
            let value = sample_channel(&channel.times, &channel.values, time, channel.step);
            match channel.path {
                GltfChannelPath::Translation => {
                    translation = Vec3::new(value[0], value[1], value[2]);
                }
                GltfChannelPath::Rotation => {
                    rotation = Quat {
                        x: value[0],
                        y: value[1],
                        z: value[2],
                        w: value[3],
                    };
                }
                GltfChannelPath::Scale => {
                    scale = Vec3::new(value[0], value[1], value[2]);
                }
            }
        }
        let length = rotation.x * rotation.x
            + rotation.y * rotation.y
            + rotation.z * rotation.z
            + rotation.w * rotation.w;
        if length > 0.0 {
            let inv = 1.0 / length.sqrt();
            rotation = Quat {
                x: rotation.x * inv,
                y: rotation.y * inv,
                z: rotation.z * inv,
                w: rotation.w * inv,
            };
        }
        Mat4::translation(translation) * Mat4::from_quat(rotation) * Mat4::scale(scale)
    }
}

/// Samples a channel at `time`: STEP holds the previous key, LINEAR interpolates
/// (component-wise; the caller normalizes quaternions).
fn sample_channel(times: &[f32], values: &[[f32; 4]], time: f32, step: bool) -> [f32; 4] {
    let last = times.len() - 1;
    if time <= times[0] {
        return values[0];
    }
    if time >= times[last] {
        return values[last];
    }
    for index in 0..last {
        if time < times[index + 1] {
            if step {
                return values[index];
            }
            let segment = times[index + 1] - times[index];
            if segment <= f32::EPSILON {
                return values[index];
            }
            let amount = (time - times[index]) / segment;
            let a = values[index];
            let b = values[index + 1];
            return [
                a[0] + (b[0] - a[0]) * amount,
                a[1] + (b[1] - a[1]) * amount,
                a[2] + (b[2] - a[2]) * amount,
                a[3] + (b[3] - a[3]) * amount,
            ];
        }
    }
    values[last]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn quad_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../game_test/assets/models/m8-skin-quad.glb")
    }

    fn quad_animation() -> Animation3D {
        let asset = GltfAsset::load(quad_path()).unwrap();
        Animation3D::from_gltf(&asset, "Bend", true).unwrap()
    }

    #[test]
    fn clip_advances_loops_and_clamps() {
        let mut animation = quad_animation();
        assert_eq!(animation.current_name(), "Bend");
        assert_eq!(animation.time(), 0.0);
        // "Bend" is 1.0s long and looped.
        animation.advance(0.25);
        assert!((animation.time() - 0.25).abs() < 1e-5);
        animation.advance(0.9); // 1.15 -> wraps to 0.15
        assert!((animation.time() - 0.15).abs() < 1e-5);
        animation.set_looped(false);
        animation.advance(2.0);
        assert!((animation.time() - 1.0).abs() < 1e-5, "clamped at duration");
    }

    #[test]
    fn pause_freezes_time() {
        let mut animation = quad_animation();
        animation.advance(0.2);
        animation.set_paused(true);
        animation.advance(1.0);
        assert!((animation.time() - 0.2).abs() < 1e-5);
        animation.set_paused(false);
        animation.advance(0.1);
        assert!((animation.time() - 0.3).abs() < 1e-5);
    }

    #[test]
    fn switching_clip_restarts_and_rejects_unknown() {
        let mut animation = quad_animation();
        animation.advance(0.5);
        assert!(!animation.set_clip("Bend").unwrap());
        assert!(matches!(
            animation.set_clip("Missing"),
            Err(Animation3dError::UnknownClip(_))
        ));
        // Only one clip in the quad, so re-selecting it is a no-op.
        assert_eq!(animation.current_name(), "Bend");
    }

    #[test]
    fn joint_palette_is_identity_at_bind_pose() {
        let mut animation = quad_animation();
        animation.set_looped(false);
        animation.advance(0.0);
        let palette = animation.joint_matrices();
        assert_eq!(palette.len(), 2);
        // At t=0 the channel value is the identity rotation, so both joints are identity.
        for matrix in &palette {
            assert_eq!(*matrix, Mat4::identity());
        }
        // Mid-animation the palette differs from bind pose (Joint1 rotated).
        animation.advance(0.5);
        let moved = animation.joint_matrices();
        assert_ne!(moved[1], Mat4::identity(), "Joint1 should rotate mid-clip");
        // The bind palette is always identity for this asset.
        assert_eq!(
            animation.bind_palette(),
            vec![Mat4::identity(), Mat4::identity()]
        );
    }

    #[test]
    fn rejects_non_skinned_asset() {
        let asset = GltfAsset::load(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../game_test/assets/models/m8-test-triangle.glb"),
        )
        .unwrap();
        assert_eq!(
            Animation3D::from_gltf(&asset, "x", true),
            Err(Animation3dError::NotSkinned)
        );
    }

    #[test]
    fn descriptor_round_trips() {
        let descriptor = Animation3dDescriptor {
            mesh: "assets/models/kaykit-warrior.glb".into(),
            clip: "Battle".into(),
            looped: false,
        };
        assert_eq!(
            Animation3dDescriptor::from_component_str(&descriptor.to_component_str()).unwrap(),
            descriptor
        );
        assert!(Animation3dDescriptor::from_component_str("clip=Walk").is_err());
        assert!(
            Animation3dDescriptor::from_component_str("mesh=x;clip=Walk;looped=maybe").is_err()
        );
    }
}
