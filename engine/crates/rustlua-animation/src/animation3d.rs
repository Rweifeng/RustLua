//! 3D skeletal animation component for M8-07: bind-pose node hierarchy, a skin's
//! joint list with inverse bind matrices, and named clips. Each frame the renderer
//! asks for the joint palette produced by the active clip at the current time and
//! uploads it for GPU skinning.

use std::path::Path;

use rustlua_assets::gltf_asset::{GltfAsset, GltfChannelPath};
use rustlua_math::math3d::{Mat4, Quat, Vec3};

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

/// In-flight crossfade: the clip being faded out, its own time axis, and progress.
#[derive(Clone, Debug, PartialEq)]
struct Fade3D {
    from: usize,
    from_time: f32,
    elapsed: f32,
    duration: f32,
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
    fade: Option<Fade3D>,
    paused: bool,
    looped: bool,
}

/// Errors while building or driving a 3D animation.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Animation3dError {
    #[error("animation references missing skin {0}")]
    MissingSkin(usize),
    #[error("skin has no joints")]
    EmptySkin,
    #[error("skin references invalid joint node {0}")]
    InvalidJoint(usize),
    #[error("unknown 3D animation clip: {0}")]
    UnknownClip(String),
    #[error("duplicate 3D animation clip: {0}")]
    DuplicateClip(String),
    #[error("mesh is not skinned (no skin in the glTF asset)")]
    NotSkinned,
    #[error("Animation3D component is missing its mesh path")]
    MissingMesh,
    #[error("invalid Animation3D mesh path: {0}")]
    InvalidMeshPath(String),
    #[error("3D animation import failed: {0}")]
    Import(String),
    #[error("invalid Animation3D component string: {0}")]
    Malformed(String),
}

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
            fade: None,
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

    /// Switches the active clip, restarting time and cancelling any crossfade.
    /// Returns `true` on a change.
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
        self.fade = None;
        Ok(true)
    }

    /// Switches the active clip with a crossfade: the new clip starts at time 0
    /// while the outgoing clip (at its current time) fades out over `seconds`.
    /// A non-positive `seconds` degrades to an instant [`Self::set_clip`].
    /// Returns `true` on a change; re-selecting the active clip is a no-op.
    pub fn crossfade_to(&mut self, name: &str, seconds: f32) -> Result<bool, Animation3dError> {
        if self.current_clip().name == name {
            return Ok(false);
        }
        let index = self
            .clips
            .iter()
            .position(|clip| clip.name == name)
            .ok_or_else(|| Animation3dError::UnknownClip(name.to_owned()))?;
        // NaN 也走这条：`!(seconds > 0.0)` 比 `seconds <= 0.0` 多挡住 NaN。
        self.fade = if seconds > 0.0 {
            Some(Fade3D {
                from: self.current,
                from_time: self.time,
                elapsed: 0.0,
                duration: seconds,
            })
        } else {
            None
        };
        self.current = index;
        self.time = 0.0;
        Ok(true)
    }

    /// Whether a crossfade is still in flight (the outgoing clip is still blended in).
    #[must_use]
    pub fn is_fading(&self) -> bool {
        self.fade.is_some()
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
    /// clips clamp at their duration. An in-flight crossfade advances the outgoing
    /// clip on its own time axis and is dropped once `dt` has run out. Paused
    /// animations do not advance.
    pub fn advance(&mut self, delta_seconds: f32) {
        if self.paused || !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        let duration = self.current_clip().duration;
        self.time = advance_time(self.time, delta_seconds, duration, self.looped);
        let mut fade_finished = false;
        if let Some(fade) = &mut self.fade {
            fade.elapsed += delta_seconds;
            let from_duration = self.clips[fade.from].duration;
            fade.from_time =
                advance_time(fade.from_time, delta_seconds, from_duration, self.looped);
            fade_finished = fade.elapsed >= fade.duration;
        }
        if fade_finished {
            self.fade = None;
        }
    }

    /// docs/54 §4.4：这套骨骼动画在**运行时资源身份**里的键。
    ///
    /// 3D 片段来自 glTF mesh 资产（`Animation3D` 自己不存路径，路径在实体的 `MeshRenderer` 上），
    /// 因此身份 = `mesh + clip`：换网格或换片段都会得到不同的键，"这份骨骼动画是哪一个"可判定。
    #[must_use]
    pub fn resource_key(&self, mesh_path: &str) -> String {
        format!("animation3d:{}:{}", mesh_path, self.current_name())
    }

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
        let (mut translation, mut rotation, mut scale) = self.local_trs(self.current, node, time);
        if let Some(fade) = &self.fade {
            let amount = (fade.elapsed / fade.duration).clamp(0.0, 1.0);
            let (from_translation, from_rotation, from_scale) =
                self.local_trs(fade.from, node, fade.from_time);
            translation = from_translation.lerp(translation, amount);
            scale = from_scale.lerp(scale, amount);
            rotation = nlerp_quat(from_rotation, rotation, amount);
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

    /// Samples one clip's local translation/rotation/scale for `node` at `time`;
    /// channels the clip does not drive fall back to the bind pose.
    fn local_trs(&self, clip: usize, node: usize, time: f32) -> (Vec3, Quat, Vec3) {
        let mut translation = self.bind_translation[node];
        let mut rotation = self.bind_rotation[node];
        let mut scale = self.bind_scale[node];
        for channel in &self.clips[clip].channels {
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
        (translation, rotation, scale)
    }
}

/// Advances one clip's clock: looped clips wrap, others clamp at their duration.
fn advance_time(time: f32, delta: f32, duration: f32, looped: bool) -> f32 {
    let next = time + delta;
    if looped && duration > 0.0 {
        next % duration
    } else {
        next.min(duration.max(0.0))
    }
}

/// Normalized linear interpolation between two rotations; flips the sign when the
/// quaternions point to opposite hemispheres so the blend takes the short arc.
fn nlerp_quat(from: Quat, to: Quat, amount: f32) -> Quat {
    let dot = from.x * to.x + from.y * to.y + from.z * to.z + from.w * to.w;
    let to = if dot < 0.0 {
        Quat {
            x: -to.x,
            y: -to.y,
            z: -to.z,
            w: -to.w,
        }
    } else {
        to
    };
    let a = 1.0 - amount;
    Quat {
        x: from.x * a + to.x * amount,
        y: from.y * a + to.y * amount,
        z: from.z * a + to.z * amount,
        w: from.w * a + to.w * amount,
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
        rustlua_infra::games_dir().join("examples/assets/models/m8-skin-quad.glb")
    }

    fn quad_animation() -> Option<Animation3D> {
        let path = quad_path();
        if !path.is_file() {
            return None;
        }
        let asset = GltfAsset::load(path).ok()?;
        Animation3D::from_gltf(&asset, "Bend", true).ok()
    }

    #[test]
    fn clip_advances_loops_and_clamps() {
        let Some(mut animation) = quad_animation() else {
            return;
        };
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
        let Some(mut animation) = quad_animation() else {
            return;
        };
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
        let Some(mut animation) = quad_animation() else {
            return;
        };
        animation.advance(0.5);
        assert!(!animation.set_clip("Bend").unwrap());
        assert!(matches!(
            animation.set_clip("Missing"),
            Err(Animation3dError::UnknownClip(_))
        ));
        // Only one clip in the quad, so re-selecting it is a no-op.
        assert_eq!(animation.current_name(), "Bend");
    }

    /// A second clip for the quad that only slides Joint1 sideways, which makes
    /// "blended pose" distinguishable from both endpoints.
    fn slide_clip() -> AnimClip3D {
        AnimClip3D {
            name: "Slide".to_owned(),
            channels: vec![Channel3D {
                node: 1,
                path: GltfChannelPath::Translation,
                times: vec![0.0, 1.0],
                values: vec![[0.5, 0.0, 0.0, 0.0], [0.5, 0.0, 0.0, 0.0]],
                step: false,
            }],
            duration: 1.0,
        }
    }

    fn palette_close(actual: &[Mat4], expected: &[Mat4]) -> bool {
        actual.len() == expected.len()
            && actual.iter().zip(expected).all(|(a, b)| {
                a.columns
                    .iter()
                    .zip(&b.columns)
                    .all(|(ca, cb)| ca.iter().zip(cb).all(|(x, y)| (x - y).abs() < 1e-5))
            })
    }

    #[test]
    fn crossfade_blends_old_pose_into_new_clip() {
        let Some(mut animation) = quad_animation() else {
            return;
        };
        animation.set_looped(false);
        animation.advance(0.5);
        let bend = animation.joint_matrices();
        animation.clips.push(slide_clip());
        let mut reference = animation.clone();
        reference.set_clip("Slide").unwrap();
        let slide = reference.joint_matrices();

        assert!(animation.crossfade_to("Slide", 1.0).unwrap());
        assert!(animation.is_fading());
        assert_eq!(animation.current_name(), "Slide");
        // 淡入起点仍是旧 clip 的当前姿态（新 clip 从 0 秒开始）。
        assert!(palette_close(&animation.joint_matrices(), &bend));

        animation.advance(0.5);
        let middle = animation.joint_matrices();
        assert!(
            !palette_close(&middle, &bend),
            "halfway is not the old pose"
        );
        assert!(
            !palette_close(&middle, &slide),
            "halfway is not the new pose"
        );

        animation.advance(0.5);
        assert!(!animation.is_fading());
        assert_eq!(animation.joint_matrices(), slide);
    }

    #[test]
    fn crossfade_same_clip_is_noop_and_non_positive_is_instant() {
        let Some(mut animation) = quad_animation() else {
            return;
        };
        animation.advance(0.4);
        assert!(!animation.crossfade_to("Bend", 0.5).unwrap());
        assert!(!animation.is_fading());
        assert!(matches!(
            animation.crossfade_to("Missing", 0.5),
            Err(Animation3dError::UnknownClip(_))
        ));

        animation.clips.push(slide_clip());
        assert!(animation.crossfade_to("Slide", 0.0).unwrap());
        assert!(!animation.is_fading(), "0s degrades to an instant switch");
        assert_eq!(animation.current_name(), "Slide");
        assert_eq!(animation.time(), 0.0);

        // `set_clip` 是"硬切"：它同样会撤掉进行中的淡入。
        assert!(animation.crossfade_to("Bend", 0.5).unwrap());
        assert!(animation.is_fading());
        animation.set_clip("Slide").unwrap();
        assert!(!animation.is_fading());
    }

    #[test]
    fn paused_freezes_an_in_flight_crossfade() {
        let Some(mut animation) = quad_animation() else {
            return;
        };
        animation.clips.push(slide_clip());
        animation.crossfade_to("Slide", 1.0).unwrap();
        animation.advance(0.25);
        animation.set_paused(true);
        animation.advance(5.0);
        assert!(animation.is_fading(), "paused fade must not complete");
    }

    #[test]
    fn joint_palette_is_identity_at_bind_pose() {
        let Some(mut animation) = quad_animation() else {
            return;
        };
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
        let path = rustlua_infra::games_dir().join("examples/assets/models/m8-test-triangle.glb");
        let Some(asset) = GltfAsset::load(path).ok() else {
            return;
        };
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
