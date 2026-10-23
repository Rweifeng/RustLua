//! P3-E01-4c：`.anim` 编辑会话——轨道/关键帧/事件的增删移动、帧吸附与快照撤销。
//!
//! 会话持有剪辑、保存点快照与撤销/重做栈。所有修改先落在克隆上，校验通过才提交，
//! 所以失败的操作不会留下半成品；`is_dirty` 直接比较当前剪辑与 [`AnimEditSession::mark_saved`]
//! 的快照，撤销回保存点就会自动变回干净。
//!
//! 不变量与 [`crate::anim_asset`] 的解析器保持一致：关键帧时间严格递增且落在
//! `[0, duration]`；每条轨道至少保留一个关键帧；事件按时间非递减，同一时刻同名事件唯一。
//! 四元数在写入时归一化（与解析时的归一化同一规则）。

use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::{Quat, Vec3};

use crate::anim_asset::{
    AnimClip, AnimEvent, AnimKeyframe, AnimPath, AnimTrack, AnimValue, TransformChannel,
};
use crate::animation_track::Interpolation;

/// 撤销栈上限：编辑器里连续拖拽会产生很多快照，保留最近 128 步足够回退且不会无界增长。
const UNDO_LIMIT: usize = 128;

/// 编辑操作被拒绝的原因。所有错误都不改变会话状态。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AnimEditError {
    #[error("track index {index} out of range (clip has {count} tracks)")]
    TrackOutOfRange { index: usize, count: usize },
    #[error("keyframe index {index} out of range (track has {count} keyframes)")]
    KeyframeOutOfRange { index: usize, count: usize },
    #[error("event index {index} out of range (clip has {count} events)")]
    EventOutOfRange { index: usize, count: usize },
    #[error("time {time} must be finite")]
    TimeNotFinite { time: f32 },
    #[error("time {time} is outside [0, {duration}]")]
    TimeOutOfRange { time: f32, duration: f32 },
    #[error("a keyframe already exists at {time}")]
    DuplicateKeyframe { time: f32 },
    #[error("track `{path}` already exists")]
    DuplicateTrack { path: String },
    #[error("cannot remove the last keyframe of a track")]
    LastKeyframe,
    #[error("`{path}` expects {expected}, got {got}")]
    ValueKindMismatch {
        path: String,
        expected: &'static str,
        got: &'static str,
    },
    #[error("invalid value for `{path}`: {detail}")]
    InvalidValue { path: String, detail: String },
    #[error("`{path}` is discrete and only accepts step interpolation")]
    DiscreteInterpolation { path: String },
    #[error("invalid event name `{name}`: {detail}")]
    InvalidEventName { name: String, detail: &'static str },
    #[error("event `{name}` already exists at {time}")]
    DuplicateEvent { time: f32, name: String },
    #[error("fps must be finite and positive, got {fps}")]
    InvalidFps { fps: f32 },
    #[error("`duration` must be finite and positive, got {duration}")]
    InvalidDuration { duration: f32 },
    #[error("duration {duration} is below existing content at {time}")]
    DurationBelowContent { duration: f32, time: f32 },
    #[error("invalid clip name `{name}` (must be non-empty, single-line, no surrounding spaces)")]
    InvalidName { name: String },
    #[error("invalid track path `{path}`")]
    InvalidPath { path: String },
}

/// 一段 `.anim` 的编辑会话。
#[derive(Clone, Debug)]
pub struct AnimEditSession {
    clip: AnimClip,
    saved: AnimClip,
    undo: Vec<AnimClip>,
    redo: Vec<AnimClip>,
}

impl AnimEditSession {
    /// 以当前剪辑为保存点开一个干净会话。
    #[must_use]
    pub fn new(clip: AnimClip) -> Self {
        Self {
            saved: clip.clone(),
            clip,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    #[must_use]
    pub fn clip(&self) -> &AnimClip {
        &self.clip
    }

    /// 最近一次 [`Self::mark_saved`] 时的剪辑。
    #[must_use]
    pub fn saved(&self) -> &AnimClip {
        &self.saved
    }

    /// 当前剪辑的规范文本（即将要写盘的字节）。
    #[must_use]
    pub fn text(&self) -> String {
        self.clip.to_text()
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.clip != self.saved
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// 调用方成功写盘后调用：把当前剪辑设为保存点。撤销历史保留，撤销回旧内容会再次变脏。
    pub fn mark_saved(&mut self) {
        self.saved = self.clip.clone();
    }

    /// 丢弃未保存修改回到保存点（不可撤销）。
    pub fn revert_to_saved(&mut self) {
        self.clip = self.saved.clone();
        self.undo.clear();
        self.redo.clear();
    }

    /// 回退一步；没有历史时返回 `false`。
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.clip, previous));
        true
    }

    /// 重做一步；没有历史时返回 `false`。
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.clip, next));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        true
    }

    /// 把 `time` 吸附到 `fps` 帧网格（四舍五入）并夹到 `[0, duration]`。
    pub fn snap_time(&self, time: f32, fps: f32) -> Result<f32, AnimEditError> {
        if !time.is_finite() {
            return Err(AnimEditError::TimeNotFinite { time });
        }
        if !fps.is_finite() || fps <= 0.0 {
            return Err(AnimEditError::InvalidFps { fps });
        }
        let frames = (time * fps).round();
        let snapped = if frames.is_finite() {
            frames / fps
        } else {
            self.clip.duration
        };
        Ok(snapped.clamp(0.0, self.clip.duration))
    }

    /// 改剪辑名。只接受非空、单行、无首尾空白的名字（否则写回的文本读不回来）。
    pub fn set_name(&mut self, name: impl Into<String>) -> Result<(), AnimEditError> {
        let name = name.into();
        if name.is_empty() || name.trim() != name || name.contains('\n') || name.contains('\r') {
            return Err(AnimEditError::InvalidName { name });
        }
        if name == self.clip.name {
            return Ok(());
        }
        self.commit(|clip| {
            clip.name = name.clone();
            Ok(())
        })
    }

    pub fn set_looped(&mut self, looped: bool) -> Result<(), AnimEditError> {
        if looped == self.clip.looped {
            return Ok(());
        }
        self.commit(|clip| {
            clip.looped = looped;
            Ok(())
        })
    }

    /// 改时长。不能短于任何关键帧或事件时间。
    pub fn set_duration(&mut self, duration: f32) -> Result<(), AnimEditError> {
        if !duration.is_finite() || duration <= 0.0 {
            return Err(AnimEditError::InvalidDuration { duration });
        }
        let overflowing = self
            .clip
            .tracks
            .iter()
            .flat_map(|track| track.keyframes.iter())
            .map(|keyframe| keyframe.time)
            .chain(self.clip.events.iter().map(|event| event.time))
            .find(|time| *time > duration);
        if let Some(time) = overflowing {
            return Err(AnimEditError::DurationBelowContent { duration, time });
        }
        if duration == self.clip.duration {
            return Ok(());
        }
        self.commit(|clip| {
            clip.duration = duration;
            Ok(())
        })
    }

    /// 追加一条只有首键的轨道。
    pub fn add_track(
        &mut self,
        path: AnimPath,
        interpolation: Interpolation,
        time: f32,
        value: AnimValue,
    ) -> Result<usize, AnimEditError> {
        validate_path(&path)?;
        if path.is_discrete() && interpolation != Interpolation::Step {
            return Err(AnimEditError::DiscreteInterpolation {
                path: path.as_text(),
            });
        }
        check_time(time, self.clip.duration)?;
        let value = validate_value(&path, value)?;
        let path_text = path.as_text();
        self.commit(|clip| {
            if clip.tracks.iter().any(|track| track.path == path) {
                return Err(AnimEditError::DuplicateTrack { path: path_text });
            }
            let index = clip.tracks.len();
            clip.tracks.push(AnimTrack {
                path: path.clone(),
                interpolation,
                keyframes: vec![AnimKeyframe { time, value }],
            });
            Ok(index)
        })
    }

    pub fn remove_track(&mut self, track: usize) -> Result<AnimTrack, AnimEditError> {
        let count = self.clip.tracks.len();
        if track >= count {
            return Err(AnimEditError::TrackOutOfRange {
                index: track,
                count,
            });
        }
        self.commit(|clip| Ok(clip.tracks.remove(track)))
    }

    pub fn set_track_interpolation(
        &mut self,
        track: usize,
        interpolation: Interpolation,
    ) -> Result<(), AnimEditError> {
        let count = self.clip.tracks.len();
        let target = self
            .clip
            .tracks
            .get(track)
            .ok_or(AnimEditError::TrackOutOfRange {
                index: track,
                count,
            })?;
        if target.path.is_discrete() && interpolation != Interpolation::Step {
            return Err(AnimEditError::DiscreteInterpolation {
                path: target.path.as_text(),
            });
        }
        if target.interpolation == interpolation {
            return Ok(());
        }
        self.commit(|clip| {
            clip.tracks[track].interpolation = interpolation;
            Ok(())
        })
    }

    /// 插入关键帧并返回排序后的下标；同一时间已有关键帧则拒绝。
    pub fn add_keyframe(
        &mut self,
        track: usize,
        time: f32,
        value: AnimValue,
    ) -> Result<usize, AnimEditError> {
        let track_count = self.clip.tracks.len();
        let target = self
            .clip
            .tracks
            .get(track)
            .ok_or(AnimEditError::TrackOutOfRange {
                index: track,
                count: track_count,
            })?;
        let path = target.path.clone();
        check_time(time, self.clip.duration)?;
        let value = validate_value(&path, value)?;
        self.commit(|clip| {
            let keyframes = &mut clip.tracks[track].keyframes;
            if keyframes.iter().any(|keyframe| keyframe.time == time) {
                return Err(AnimEditError::DuplicateKeyframe { time });
            }
            let index = keyframes.partition_point(|keyframe| keyframe.time < time);
            keyframes.insert(index, AnimKeyframe { time, value });
            Ok(index)
        })
    }

    /// 删除关键帧；轨道至少要留一个（空轨道写不回合法资产）。
    pub fn remove_keyframe(
        &mut self,
        track: usize,
        keyframe: usize,
    ) -> Result<AnimKeyframe, AnimEditError> {
        let track_count = self.clip.tracks.len();
        let target = self
            .clip
            .tracks
            .get(track)
            .ok_or(AnimEditError::TrackOutOfRange {
                index: track,
                count: track_count,
            })?;
        let keyframe_count = target.keyframes.len();
        if keyframe >= keyframe_count {
            return Err(AnimEditError::KeyframeOutOfRange {
                index: keyframe,
                count: keyframe_count,
            });
        }
        if keyframe_count <= 1 {
            return Err(AnimEditError::LastKeyframe);
        }
        self.commit(|clip| Ok(clip.tracks[track].keyframes.remove(keyframe)))
    }

    /// 移动关键帧到新时间并保持时间排序；返回移动后的下标。
    pub fn move_keyframe(
        &mut self,
        track: usize,
        keyframe: usize,
        time: f32,
    ) -> Result<usize, AnimEditError> {
        let track_count = self.clip.tracks.len();
        let target = self
            .clip
            .tracks
            .get(track)
            .ok_or(AnimEditError::TrackOutOfRange {
                index: track,
                count: track_count,
            })?;
        let keyframe_count = target.keyframes.len();
        if keyframe >= keyframe_count {
            return Err(AnimEditError::KeyframeOutOfRange {
                index: keyframe,
                count: keyframe_count,
            });
        }
        check_time(time, self.clip.duration)?;
        if target.keyframes[keyframe].time == time {
            return Ok(keyframe);
        }
        self.commit(|clip| {
            let keyframes = &mut clip.tracks[track].keyframes;
            if keyframes.iter().any(|existing| existing.time == time) {
                return Err(AnimEditError::DuplicateKeyframe { time });
            }
            let mut moved = keyframes.remove(keyframe);
            moved.time = time;
            let index = keyframes.partition_point(|existing| existing.time < time);
            keyframes.insert(index, moved);
            Ok(index)
        })
    }

    /// 先按 `fps` 吸附再移动——时间线拖拽的常用组合。
    pub fn move_keyframe_snapped(
        &mut self,
        track: usize,
        keyframe: usize,
        time: f32,
        fps: f32,
    ) -> Result<usize, AnimEditError> {
        let snapped = self.snap_time(time, fps)?;
        self.move_keyframe(track, keyframe, snapped)
    }

    pub fn set_keyframe_value(
        &mut self,
        track: usize,
        keyframe: usize,
        value: AnimValue,
    ) -> Result<(), AnimEditError> {
        let track_count = self.clip.tracks.len();
        let target = self
            .clip
            .tracks
            .get(track)
            .ok_or(AnimEditError::TrackOutOfRange {
                index: track,
                count: track_count,
            })?;
        let keyframe_count = target.keyframes.len();
        if keyframe >= keyframe_count {
            return Err(AnimEditError::KeyframeOutOfRange {
                index: keyframe,
                count: keyframe_count,
            });
        }
        let path = target.path.clone();
        let value = validate_value(&path, value)?;
        if target.keyframes[keyframe].value == value {
            return Ok(());
        }
        self.commit(|clip| {
            clip.tracks[track].keyframes[keyframe].value = value;
            Ok(())
        })
    }

    /// 插入事件并返回排序后的下标；同一时刻同名事件视为重复。
    pub fn add_event(
        &mut self,
        time: f32,
        name: impl Into<String>,
    ) -> Result<usize, AnimEditError> {
        let name = name.into();
        validate_event_name(&name)?;
        check_time(time, self.clip.duration)?;
        self.commit(|clip| {
            if clip
                .events
                .iter()
                .any(|event| event.time == time && event.name == name)
            {
                return Err(AnimEditError::DuplicateEvent {
                    time,
                    name: name.clone(),
                });
            }
            let index = clip.events.partition_point(|event| event.time <= time);
            clip.events.insert(
                index,
                AnimEvent {
                    time,
                    name: name.clone(),
                },
            );
            Ok(index)
        })
    }

    pub fn remove_event(&mut self, index: usize) -> Result<AnimEvent, AnimEditError> {
        let count = self.clip.events.len();
        if index >= count {
            return Err(AnimEditError::EventOutOfRange { index, count });
        }
        self.commit(|clip| Ok(clip.events.remove(index)))
    }

    /// 移动事件到新时间并保持顺序；返回移动后的下标。
    pub fn move_event(&mut self, index: usize, time: f32) -> Result<usize, AnimEditError> {
        let count = self.clip.events.len();
        if index >= count {
            return Err(AnimEditError::EventOutOfRange { index, count });
        }
        check_time(time, self.clip.duration)?;
        if self.clip.events[index].time == time {
            return Ok(index);
        }
        self.commit(|clip| {
            let mut event = clip.events.remove(index);
            if clip
                .events
                .iter()
                .any(|existing| existing.time == time && existing.name == event.name)
            {
                return Err(AnimEditError::DuplicateEvent {
                    time,
                    name: event.name.clone(),
                });
            }
            event.time = time;
            let index = clip
                .events
                .partition_point(|existing| existing.time <= time);
            clip.events.insert(index, event);
            Ok(index)
        })
    }

    fn commit<T>(
        &mut self,
        mutate: impl FnOnce(&mut AnimClip) -> Result<T, AnimEditError>,
    ) -> Result<T, AnimEditError> {
        let mut next = self.clip.clone();
        let output = mutate(&mut next)?;
        self.undo.push(std::mem::replace(&mut self.clip, next));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(output)
    }
}

fn check_time(time: f32, duration: f32) -> Result<(), AnimEditError> {
    if !time.is_finite() {
        return Err(AnimEditError::TimeNotFinite { time });
    }
    if !(0.0..=duration).contains(&time) {
        return Err(AnimEditError::TimeOutOfRange { time, duration });
    }
    Ok(())
}

fn validate_path(path: &AnimPath) -> Result<(), AnimEditError> {
    if let AnimPath::UiAnimation(property) = path
        && (property.is_empty()
            || property
                .chars()
                .any(|c| c.is_whitespace() || c == ';' || c == '='))
    {
        return Err(AnimEditError::InvalidPath {
            path: path.as_text(),
        });
    }
    Ok(())
}

fn validate_event_name(name: &str) -> Result<(), AnimEditError> {
    let detail = if name.is_empty() {
        Some("event name must not be empty")
    } else if name.chars().any(char::is_whitespace) {
        Some("event name must not contain whitespace")
    } else if name.contains(';') || name.contains('=') || name.contains(':') {
        Some("event name must not contain `;`, `=` or `:`")
    } else {
        None
    };
    match detail {
        Some(detail) => Err(AnimEditError::InvalidEventName {
            name: name.to_owned(),
            detail,
        }),
        None => Ok(()),
    }
}

fn expected_value(path: &AnimPath) -> &'static str {
    match path {
        AnimPath::SpriteRegion => "a region (non-negative integer)",
        AnimPath::Transform(TransformChannel::Position | TransformChannel::Scale) => "vec2",
        AnimPath::Transform(TransformChannel::Rotation) => "scalar",
        AnimPath::Transform3D(TransformChannel::Position | TransformChannel::Scale) => "vec3",
        AnimPath::Transform3D(TransformChannel::Rotation) => "quat",
        AnimPath::UiAnimation(_) => "scalar",
        AnimPath::LightIntensity => "a non-negative scalar",
    }
}

fn value_label(value: AnimValue) -> &'static str {
    match value {
        AnimValue::Vec2(_) => "vec2",
        AnimValue::Vec3(_) => "vec3",
        AnimValue::Quat(_) => "quat",
        AnimValue::Scalar(_) => "scalar",
        AnimValue::Region(_) => "region",
    }
}

fn validate_value(path: &AnimPath, value: AnimValue) -> Result<AnimValue, AnimEditError> {
    let matches_expected = matches!(
        (path, value),
        (
            AnimPath::Transform(TransformChannel::Position | TransformChannel::Scale),
            AnimValue::Vec2(_)
        ) | (
            AnimPath::Transform(TransformChannel::Rotation),
            AnimValue::Scalar(_)
        ) | (
            AnimPath::Transform3D(TransformChannel::Position | TransformChannel::Scale),
            AnimValue::Vec3(_)
        ) | (
            AnimPath::Transform3D(TransformChannel::Rotation),
            AnimValue::Quat(_)
        ) | (AnimPath::SpriteRegion, AnimValue::Region(_))
            | (AnimPath::UiAnimation(_), AnimValue::Scalar(_))
            | (AnimPath::LightIntensity, AnimValue::Scalar(_))
    );
    if !matches_expected {
        return Err(AnimEditError::ValueKindMismatch {
            path: path.as_text(),
            expected: expected_value(path),
            got: value_label(value),
        });
    }
    let path_text = path.as_text();
    let invalid = |detail: &str| AnimEditError::InvalidValue {
        path: path_text.clone(),
        detail: detail.to_owned(),
    };
    match value {
        AnimValue::Vec2(Vec2 { x, y }) if !(x.is_finite() && y.is_finite()) => {
            Err(invalid("components must be finite"))
        }
        AnimValue::Vec3(Vec3 { x, y, z }) if !(x.is_finite() && y.is_finite() && z.is_finite()) => {
            Err(invalid("components must be finite"))
        }
        AnimValue::Scalar(value) if !value.is_finite() => Err(invalid("value must be finite")),
        AnimValue::Scalar(value) if matches!(path, AnimPath::LightIntensity) && value < 0.0 => {
            Err(invalid("Light.intensity must be non-negative"))
        }
        AnimValue::Quat(quat) => normalize_quat(quat)
            .map(AnimValue::Quat)
            .ok_or_else(|| invalid("quaternion must be non-zero and finite")),
        _ => Ok(value),
    }
}

fn normalize_quat(quat: Quat) -> Option<Quat> {
    let length_squared = quat.x * quat.x + quat.y * quat.y + quat.z * quat.z + quat.w * quat.w;
    if !length_squared.is_finite() || length_squared <= 1e-12 {
        return None;
    }
    let inverse = length_squared.sqrt().recip();
    Some(Quat {
        x: quat.x * inverse,
        y: quat.y * inverse,
        z: quat.z * inverse,
        w: quat.w * inverse,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
version=1
name=walk
duration=1
looped=true
track=Transform.position;interp=linear;keys=0:0,0;0.5:0,1;1:0,0
track=Sprite.region;interp=step;keys=0:0;0.5:1
event=0.5:step
";

    fn session() -> AnimEditSession {
        AnimEditSession::new(AnimClip::from_text("walk.anim", TEXT).expect("fixture parses"))
    }

    fn keyframe_times(session: &AnimEditSession, track: usize) -> Vec<f32> {
        session.clip().tracks[track]
            .keyframes
            .iter()
            .map(|keyframe| keyframe.time)
            .collect()
    }

    fn event_names(session: &AnimEditSession) -> Vec<&str> {
        session
            .clip()
            .events
            .iter()
            .map(|event| event.name.as_str())
            .collect()
    }

    #[test]
    fn add_keyframe_inserts_sorted_and_undo_restores_bytes() {
        let mut session = session();
        let index = session
            .add_keyframe(0, 0.25, AnimValue::Vec2(Vec2::new(0.0, 0.5)))
            .expect("insert between existing keys");
        assert_eq!(index, 1);
        assert_eq!(keyframe_times(&session, 0), vec![0.0, 0.25, 0.5, 1.0]);
        assert!(session.is_dirty());
        let edited = session.text();
        let reparsed = AnimClip::from_text("walk.anim", &edited).expect("编辑后仍是合法资产");
        assert_eq!(reparsed, *session.clip(), "写回文本逐值往返");

        assert!(session.undo());
        assert_eq!(session.text(), TEXT, "撤销回原始字节");
        assert!(!session.is_dirty());
        assert!(session.redo());
        assert_eq!(session.text(), edited);
    }

    #[test]
    fn removing_the_only_keyframe_is_rejected_without_mutation() {
        let mut single = AnimEditSession::new(
            AnimClip::from_text(
                "one.anim",
                "version=1\nname=one\nduration=1\n\
                 track=Transform.rotation;interp=linear;keys=0:0\n",
            )
            .expect("fixture parses"),
        );
        let before = single.text();
        assert_eq!(
            single.remove_keyframe(0, 0),
            Err(AnimEditError::LastKeyframe)
        );
        assert_eq!(single.text(), before);
        assert!(!single.can_undo(), "被拒绝的操作不产生撤销步");

        let mut session = session();
        let removed = session
            .remove_keyframe(1, 1)
            .expect("sprite track has two keys");
        assert_eq!(removed.time, 0.5);
        assert_eq!(keyframe_times(&session, 1), vec![0.0]);
        session.undo();
        assert_eq!(session.text(), TEXT);
    }

    #[test]
    fn move_keyframe_reorders_and_rejects_collisions() {
        let mut session = session();
        let index = session
            .move_keyframe(0, 2, 0.25)
            .expect("move last key back");
        assert_eq!(index, 1);
        assert_eq!(keyframe_times(&session, 0), vec![0.0, 0.25, 0.5]);

        let before = session.text();
        assert_eq!(
            session.move_keyframe(0, 2, 0.25),
            Err(AnimEditError::DuplicateKeyframe { time: 0.25 })
        );
        assert_eq!(
            session.move_keyframe(0, 0, 5.0),
            Err(AnimEditError::TimeOutOfRange {
                time: 5.0,
                duration: 1.0,
            })
        );
        assert_eq!(session.text(), before, "被拒绝的移动不提交");

        let snapped = session.snap_time(0.31, 30.0).expect("snap");
        assert!(
            (snapped - 0.3).abs() < 1e-6,
            "0.31s @30fps → 0.3s，got {snapped}"
        );
        let index = session
            .move_keyframe_snapped(0, 2, 0.31, 30.0)
            .expect("move to snapped time");
        assert_eq!(index, 2);
        assert!((keyframe_times(&session, 0)[2] - 0.3).abs() < 1e-6);
        assert_eq!(session.snap_time(5.0, 30.0).expect("clamp"), 1.0);
        assert!(matches!(
            session.snap_time(0.1, 0.0),
            Err(AnimEditError::InvalidFps { .. })
        ));
        assert!(matches!(
            session.snap_time(f32::NAN, 30.0),
            Err(AnimEditError::TimeNotFinite { .. })
        ));
        assert!(session.undo());
        assert_eq!(session.text(), before);
    }

    #[test]
    fn undo_redo_follow_the_saved_snapshot_and_new_edits_clear_redo() {
        let mut session = session();
        assert!(!session.can_undo() && !session.can_redo());
        assert!(!session.undo() && !session.redo(), "空历史返回 false");

        session.set_name("walk_fast").expect("rename");
        assert!(session.is_dirty());
        assert_eq!(session.undo_depth(), 1);
        session.mark_saved();
        assert!(!session.is_dirty());
        assert!(session.can_undo(), "保存不清撤销历史");

        session.add_event(0.75, "land").expect("add event");
        assert!(session.is_dirty());
        session.undo();
        assert!(!session.is_dirty(), "撤销回保存点即干净");
        assert!(session.can_redo());
        session.redo();
        assert!(session.is_dirty());

        session.undo();
        assert!(session.can_redo());
        session
            .add_event(0.8, "jump")
            .expect("new edit clears redo");
        assert!(!session.can_redo());
        session.revert_to_saved();
        assert!(!session.is_dirty());
        assert!(!session.can_undo());
        assert_eq!(session.clip().name, "walk_fast");
        assert_eq!(session.clip().events.len(), 1, "revert 丢掉未保存的事件");

        let depth = session.undo_depth();
        session.set_name("walk_fast").expect("no-op");
        assert_eq!(session.undo_depth(), depth, "无变化的设置不产生撤销步");
    }

    #[test]
    fn value_kind_and_constraints_are_enforced() {
        let mut session = session();
        let before = session.text();
        assert!(matches!(
            session.add_keyframe(0, 0.25, AnimValue::Scalar(1.0)),
            Err(AnimEditError::ValueKindMismatch { .. })
        ));
        assert!(matches!(
            session.add_keyframe(0, 0.25, AnimValue::Vec2(Vec2::new(f32::NAN, 0.0))),
            Err(AnimEditError::InvalidValue { .. })
        ));
        assert_eq!(session.text(), before);

        let index = session
            .add_track(
                AnimPath::LightIntensity,
                Interpolation::Linear,
                0.0,
                AnimValue::Scalar(1.0),
            )
            .expect("add light track");
        assert_eq!(index, 2);
        assert!(matches!(
            session.add_keyframe(2, 0.5, AnimValue::Scalar(-1.0)),
            Err(AnimEditError::InvalidValue { .. })
        ));
        assert!(
            matches!(
                session.set_track_interpolation(1, Interpolation::Linear),
                Err(AnimEditError::DiscreteInterpolation { .. })
            ),
            "Sprite.region 只接受 step"
        );

        let mut quat_session = AnimEditSession::new(
            AnimClip::from_text(
                "q.anim",
                "version=1\nname=q\nduration=1\n\
                 track=Transform3D.rotation;interp=linear;keys=0:0,0,0,1\n",
            )
            .expect("fixture parses"),
        );
        quat_session
            .add_keyframe(
                0,
                1.0,
                AnimValue::Quat(Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 2.0,
                }),
            )
            .expect("non-unit quat accepted and normalized");
        match quat_session.clip().tracks[0].keyframes[1].value {
            AnimValue::Quat(quat) => {
                assert!(quat.is_unit(), "写入时归一化：{quat:?}");
                assert!((quat.w - 1.0).abs() < 1e-6);
            }
            other => panic!("expected quat, got {other:?}"),
        }
        assert!(matches!(
            quat_session.add_keyframe(
                0,
                0.5,
                AnimValue::Quat(Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 0.0,
                }),
            ),
            Err(AnimEditError::InvalidValue { .. })
        ));
    }

    #[test]
    fn event_editing_keeps_time_order_and_rejects_duplicates() {
        let mut session = session();
        assert_eq!(session.add_event(0.75, "land").expect("append"), 1);
        assert_eq!(session.add_event(0.25, "jump").expect("prepend"), 0);
        assert_eq!(event_names(&session), vec!["jump", "step", "land"]);

        let before = session.text();
        assert_eq!(
            session.add_event(0.5, "step"),
            Err(AnimEditError::DuplicateEvent {
                time: 0.5,
                name: "step".to_owned(),
            })
        );
        assert!(matches!(
            session.add_event(0.5, "bad name"),
            Err(AnimEditError::InvalidEventName { .. })
        ));
        assert!(matches!(
            session.add_event(0.5, "a;b"),
            Err(AnimEditError::InvalidEventName { .. })
        ));
        assert_eq!(session.text(), before);

        assert_eq!(session.move_event(0, 0.9).expect("move jump"), 2);
        let times: Vec<f32> = session
            .clip()
            .events
            .iter()
            .map(|event| event.time)
            .collect();
        assert_eq!(times, vec![0.5, 0.75, 0.9]);
        let removed = session.remove_event(1).expect("remove land");
        assert_eq!(removed.name, "land");
        session.undo();
        assert_eq!(session.clip().events.len(), 3);
        assert!(matches!(
            session.remove_event(9),
            Err(AnimEditError::EventOutOfRange { .. })
        ));
    }

    #[test]
    fn track_add_remove_validates_path_kind_and_duplicates() {
        let mut session = session();
        let before = session.text();
        assert_eq!(
            session.add_track(
                AnimPath::Transform(TransformChannel::Position),
                Interpolation::Linear,
                0.0,
                AnimValue::Vec2(Vec2::new(0.0, 0.0)),
            ),
            Err(AnimEditError::DuplicateTrack {
                path: "Transform.position".to_owned(),
            })
        );
        assert_eq!(
            session.add_track(
                AnimPath::SpriteRegion,
                Interpolation::Linear,
                0.0,
                AnimValue::Region(0),
            ),
            Err(AnimEditError::DiscreteInterpolation {
                path: "Sprite.region".to_owned(),
            })
        );
        assert!(matches!(
            session.add_track(
                AnimPath::UiAnimation(String::new()),
                Interpolation::Linear,
                0.0,
                AnimValue::Scalar(1.0),
            ),
            Err(AnimEditError::InvalidPath { .. })
        ));
        assert_eq!(session.text(), before);

        let index = session
            .add_track(
                AnimPath::UiAnimation("opacity".to_owned()),
                Interpolation::Linear,
                0.0,
                AnimValue::Scalar(1.0),
            )
            .expect("add ui track");
        assert_eq!(index, 2);
        assert!(
            session
                .text()
                .contains("track=UiAnimation.opacity;interp=linear;keys=0:1")
        );
        let removed = session.remove_track(2).expect("remove ui track");
        assert_eq!(removed.path, AnimPath::UiAnimation("opacity".to_owned()));
        assert_eq!(session.text(), before);
        assert!(matches!(
            session.remove_track(9),
            Err(AnimEditError::TrackOutOfRange { .. })
        ));
    }

    #[test]
    fn header_edits_validate_against_content() {
        let mut session = session();
        assert_eq!(
            session.set_duration(0.75),
            Err(AnimEditError::DurationBelowContent {
                duration: 0.75,
                time: 1.0,
            })
        );
        assert!(matches!(
            session.set_duration(f32::NAN),
            Err(AnimEditError::InvalidDuration { .. })
        ));
        assert_eq!(session.set_duration(2.0), Ok(()));
        assert_eq!(session.clip().duration, 2.0);
        assert!(session.text().contains("duration=2"));
        assert!(matches!(
            session.set_name("two\nlines"),
            Err(AnimEditError::InvalidName { .. })
        ));
        assert_eq!(session.set_looped(false), Ok(()));
        assert!(!session.clip().looped);
        assert!(session.undo());
        assert!(session.clip().looped);
        assert_eq!(session.clip().duration, 2.0, "恢复 looped 不回退时长");
    }
}
