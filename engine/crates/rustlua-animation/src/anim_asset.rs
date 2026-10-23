//! P3-E01-1：`.anim` 文本资产（版本 1）的解析、编码与采样。
//!
//! 现有的 [`crate::animation_track::AnimationTracks`] 是**运行时视图**：它把帧动画、Transform 与
//! 骨骼 pose 统一成 `TrackTarget`（四类粗粒度目标）。`.anim` 需要的是**属性路径**——同一个实体上
//! 可以同时有 `UiAnimation.opacity` 与 `UiAnimation.scale`，也可能有 `Light.intensity`，这些在粗粒度
//! 目标里会互相覆盖。因此 `.anim` 有自己的数据模型（[`AnimPath`] / [`AnimValue`]），不与旧视图强行
//! 合并；E01-2 的播放器直接消费这份模型，旧 `Animation` 状态机路径保持不动。
//!
//! 文本格式（严格、版本化）：
//!
//! ```text
//! version=1
//! name=hero_run
//! duration=1.2
//! looped=true
//! track=Transform.position;interp=linear;keys=0:0,0;0.6:0,1.5;1.2:0,0
//! track=Sprite.region;interp=step;keys=0:0;0.2:1;0.4:2
//! event=0.6:footstep
//! ```
//!
//! 规则：
//!
//! - `version` / `name` / `duration` 必填；`looped` 缺省 `false`。未知字段、重复字段直接报错。
//! - 每条 `track=` 的第一段是属性路径，随后是 `interp=step|linear` 与 `keys=`。第一个关键帧跟在
//!   `keys=` 后面，其余关键帧用 `;` 追加（与上面的示例一致）。
//! - 关键帧时间是**严格递增**的有限数，落在 `[0, duration]`；值必须是有限数。
//! - `Sprite.region` 只接受非负整数（帧号）与 `step`；其余路径按标量/向量/四元数的元数校验。
//! - 四元数在解析时归一化（零四元数报错）；`Light.intensity` 不接受负数。
//! - 事件名不能含空白、`;`、`=` 或 `:`；事件时间非递减，同一时刻同名事件视为重复。
//!
//! v1 的属性路径：`Transform.position|rotation|scale`（2D）、
//! `Transform3D.position|rotation|scale`（3D，rotation 是四元数）、`Sprite.region`、
//! `UiAnimation.<property>`（任意非空属性名，与 `rustlua-ui` 的自由属性一致）、`Light.intensity`。

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::{Quat, Vec3};

use crate::animation_track::{Interpolation, lerp_quat};

/// 当前 `.anim` 文本版本。
pub const ANIM_ASSET_VERSION: u32 = 1;

/// 2D/3D Transform 的属性通道。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransformChannel {
    Position,
    Rotation,
    Scale,
}

impl TransformChannel {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "position" => Some(Self::Position),
            "rotation" => Some(Self::Rotation),
            "scale" => Some(Self::Scale),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Rotation => "rotation",
            Self::Scale => "scale",
        }
    }
}

/// `.anim` 的属性路径（同实体，v1 不做跨实体路径）。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum AnimPath {
    /// 2D `Transform` 的位置/旋转/缩放。
    Transform(TransformChannel),
    /// 3D `Transform3D` 的位置/旋转/缩放（rotation 是四元数）。
    Transform3D(TransformChannel),
    /// Sprite 的帧号（播放器按实体的帧网格换算 UV）。
    SpriteRegion,
    /// `rustlua-ui` 的标量属性（`UiAnimation.opacity` 等）。
    UiAnimation(String),
    /// 3D 灯光强度。
    LightIntensity,
}

impl AnimPath {
    /// 离散目标只允许 `step`（对帧号做"插值"没有意义）。
    #[must_use]
    pub fn is_discrete(&self) -> bool {
        matches!(self, Self::SpriteRegion)
    }

    /// 解析属性路径。编辑器的「添加轨道」输入框与 `.anim` 解析器共用这一套规则。
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw == "Sprite.region" {
            return Ok(Self::SpriteRegion);
        }
        if raw == "Light.intensity" {
            return Ok(Self::LightIntensity);
        }
        if let Some(channel) = raw.strip_prefix("Transform3D.") {
            return TransformChannel::parse(channel)
                .map(Self::Transform3D)
                .ok_or_else(|| {
                    format!("unsupported Transform3D channel `{channel}` (position|rotation|scale)")
                });
        }
        if let Some(channel) = raw.strip_prefix("Transform.") {
            return TransformChannel::parse(channel)
                .map(Self::Transform)
                .ok_or_else(|| {
                    format!("unsupported Transform channel `{channel}` (position|rotation|scale)")
                });
        }
        if let Some(property) = raw.strip_prefix("UiAnimation.") {
            if property.is_empty()
                || property
                    .chars()
                    .any(|c| c.is_whitespace() || c == ';' || c == '=')
            {
                return Err(format!("invalid UiAnimation property `{property}`"));
            }
            return Ok(Self::UiAnimation(property.to_owned()));
        }
        Err(format!(
            "unsupported track path `{raw}` \
             (Transform.*|Transform3D.*|Sprite.region|UiAnimation.*|Light.intensity)"
        ))
    }

    /// 从路径解析值；`AnimValue` 的元数在这里定死。
    fn parse_value(&self, raw: &str) -> Result<AnimValue, String> {
        if raw.trim().is_empty() {
            return Err("empty keyframe value".to_owned());
        }
        match self {
            Self::SpriteRegion => raw
                .trim()
                .parse::<u32>()
                .map(AnimValue::Region)
                .map_err(|_| format!("Sprite.region must be a non-negative integer, got `{raw}`")),
            Self::Transform(TransformChannel::Position | TransformChannel::Scale) => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 2)?;
                Ok(AnimValue::Vec2(Vec2::new(numbers[0], numbers[1])))
            }
            Self::Transform(TransformChannel::Rotation) => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 1)?;
                Ok(AnimValue::Scalar(numbers[0]))
            }
            Self::Transform3D(TransformChannel::Position | TransformChannel::Scale) => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 3)?;
                Ok(AnimValue::Vec3(Vec3::new(
                    numbers[0], numbers[1], numbers[2],
                )))
            }
            Self::Transform3D(TransformChannel::Rotation) => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 4)?;
                let length_squared = numbers[0] * numbers[0]
                    + numbers[1] * numbers[1]
                    + numbers[2] * numbers[2]
                    + numbers[3] * numbers[3];
                if !length_squared.is_finite() || length_squared <= 1e-12 {
                    return Err(format!(
                        "quaternion must be non-zero and finite, got `{raw}`"
                    ));
                }
                let inverse = length_squared.sqrt().recip();
                Ok(AnimValue::Quat(Quat {
                    x: numbers[0] * inverse,
                    y: numbers[1] * inverse,
                    z: numbers[2] * inverse,
                    w: numbers[3] * inverse,
                }))
            }
            Self::UiAnimation(_) => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 1)?;
                Ok(AnimValue::Scalar(numbers[0]))
            }
            Self::LightIntensity => {
                let numbers = parse_numbers(raw)?;
                require_arity(self, &numbers, 1)?;
                if numbers[0] < 0.0 {
                    return Err(format!("Light.intensity must be non-negative, got `{raw}`"));
                }
                Ok(AnimValue::Scalar(numbers[0]))
            }
        }
    }

    /// 写回文本时使用的规范路径。
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Transform(channel) => format!("Transform.{}", channel.as_str()),
            Self::Transform3D(channel) => format!("Transform3D.{}", channel.as_str()),
            Self::SpriteRegion => "Sprite.region".to_owned(),
            Self::UiAnimation(property) => format!("UiAnimation.{property}"),
            Self::LightIntensity => "Light.intensity".to_owned(),
        }
    }
}

impl std::fmt::Display for AnimPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_text())
    }
}

/// 一个关键帧的值。`Copy` 让采样不需要 `Cow`：这些值最大也只有 4 个 `f32`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AnimValue {
    /// 2D 位置/缩放。
    Vec2(Vec2),
    /// 3D 位置/缩放。
    Vec3(Vec3),
    /// 3D 旋转（解析时已归一化）。
    Quat(Quat),
    /// 标量（2D 旋转、UI 属性、灯光强度）。
    Scalar(f32),
    /// Sprite 帧号。
    Region(u32),
}

impl AnimValue {
    fn lerp(self, next: Self, alpha: f32) -> Self {
        match (self, next) {
            (Self::Vec2(from), Self::Vec2(to)) => Self::Vec2(Vec2::new(
                from.x + (to.x - from.x) * alpha,
                from.y + (to.y - from.y) * alpha,
            )),
            (Self::Vec3(from), Self::Vec3(to)) => Self::Vec3(from.lerp(to, alpha)),
            (Self::Quat(from), Self::Quat(to)) => Self::Quat(lerp_quat(from, to, alpha)),
            (Self::Scalar(from), Self::Scalar(to)) => Self::Scalar(from + (to - from) * alpha),
            // 离散值（Region）与类型不匹配都退回前一个值——解析器已挡住这两种情况，
            // 走到这里说明是程序内构造的非法轨道，不猜。
            (value, _) => value,
        }
    }

    fn write_text(self, out: &mut String) {
        match self {
            Self::Vec2(value) => {
                out.push_str(&format_number(value.x));
                out.push(',');
                out.push_str(&format_number(value.y));
            }
            Self::Vec3(value) => {
                out.push_str(&format_number(value.x));
                out.push(',');
                out.push_str(&format_number(value.y));
                out.push(',');
                out.push_str(&format_number(value.z));
            }
            Self::Quat(value) => {
                out.push_str(&format_number(value.x));
                out.push(',');
                out.push_str(&format_number(value.y));
                out.push(',');
                out.push_str(&format_number(value.z));
                out.push(',');
                out.push_str(&format_number(value.w));
            }
            Self::Scalar(value) => out.push_str(&format_number(value)),
            Self::Region(index) => out.push_str(&index.to_string()),
        }
    }
}

/// 一个关键帧。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimKeyframe {
    pub time: f32,
    pub value: AnimValue,
}

/// 一条属性轨道。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimTrack {
    pub path: AnimPath,
    pub interpolation: Interpolation,
    pub keyframes: Vec<AnimKeyframe>,
}

impl AnimTrack {
    /// 在 `time` 采样。早于首键取首值、晚于末键取末值（循环由 [`AnimClip::wrap_time`] 折回）。
    #[must_use]
    pub fn sample(&self, time: f32) -> Option<AnimValue> {
        let first = self.keyframes.first()?;
        if !time.is_finite() || time <= first.time {
            return Some(first.value);
        }
        let last = self.keyframes.last()?;
        if time >= last.time {
            return Some(last.value);
        }
        let mut previous = first;
        for keyframe in &self.keyframes[1..] {
            if time < keyframe.time {
                let value = match self.interpolation {
                    Interpolation::Step => previous.value,
                    Interpolation::Linear => {
                        let span = keyframe.time - previous.time;
                        let alpha = if span > 0.0 {
                            ((time - previous.time) / span).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        previous.value.lerp(keyframe.value, alpha)
                    }
                };
                return Some(value);
            }
            previous = keyframe;
        }
        Some(previous.value)
    }
}

/// 一个事件标记。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimEvent {
    pub time: f32,
    pub name: String,
}

/// 一段 `.anim` 剪辑。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimClip {
    pub name: String,
    pub duration: f32,
    pub looped: bool,
    pub tracks: Vec<AnimTrack>,
    pub events: Vec<AnimEvent>,
}

impl AnimClip {
    /// 解析 `.anim` 文本。规则见模块文档；所有错误都带源路径与行号。
    pub fn from_text(path: impl AsRef<Path>, text: &str) -> Result<Self, AnimAssetError> {
        let path = path.as_ref().display().to_string();
        let error = |line: usize, detail: String| AnimAssetError {
            path: path.clone(),
            line,
            detail,
        };
        let mut entries: Vec<(usize, &str, &str)> = Vec::new();
        for (index, raw_line) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim_end_matches('\r').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (raw_key, raw_value) = line
                .split_once('=')
                .ok_or_else(|| error(line_number, format!("expected key=value, got `{line}`")))?;
            let key = raw_key.trim();
            if !matches!(
                key,
                "version" | "name" | "duration" | "looped" | "track" | "event"
            ) {
                return Err(error(line_number, format!("unknown field `{key}`")));
            }
            entries.push((line_number, key, raw_value.trim()));
        }

        let mut version: Option<u32> = None;
        let mut name: Option<String> = None;
        let mut duration: Option<f32> = None;
        let mut looped: Option<bool> = None;
        for &(line, key, value) in &entries {
            match key {
                "version" => {
                    if version.is_some() {
                        return Err(error(line, "duplicate `version`".to_owned()));
                    }
                    let parsed = value.parse::<u32>().map_err(|_| {
                        error(line, format!("version must be an integer, got `{value}`"))
                    })?;
                    if parsed != ANIM_ASSET_VERSION {
                        return Err(error(
                            line,
                            format!(
                                "unsupported anim version {parsed} (expected {ANIM_ASSET_VERSION})"
                            ),
                        ));
                    }
                    version = Some(parsed);
                }
                "name" => {
                    if name.is_some() {
                        return Err(error(line, "duplicate `name`".to_owned()));
                    }
                    if value.is_empty() {
                        return Err(error(line, "`name` is empty".to_owned()));
                    }
                    name = Some(value.to_owned());
                }
                "duration" => {
                    if duration.is_some() {
                        return Err(error(line, "duplicate `duration`".to_owned()));
                    }
                    let parsed = parse_scalar(value).map_err(|detail| error(line, detail))?;
                    if parsed <= 0.0 {
                        return Err(error(
                            line,
                            format!("`duration` must be positive, got `{value}`"),
                        ));
                    }
                    duration = Some(parsed);
                }
                "looped" => {
                    if looped.is_some() {
                        return Err(error(line, "duplicate `looped`".to_owned()));
                    }
                    looped = Some(match value {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(error(
                                line,
                                format!("`looped` must be true or false, got `{value}`"),
                            ));
                        }
                    });
                }
                _ => {}
            }
        }
        if version.is_none() {
            return Err(error(1, format!("missing `version={ANIM_ASSET_VERSION}`")));
        }
        let name = name.ok_or_else(|| error(1, "missing `name=`".to_owned()))?;
        let duration = duration.ok_or_else(|| error(1, "missing `duration=`".to_owned()))?;
        let looped = looped.unwrap_or(false);

        let mut tracks: Vec<AnimTrack> = Vec::new();
        let mut events: Vec<AnimEvent> = Vec::new();
        let mut paths: HashSet<AnimPath> = HashSet::new();
        let mut previous_event_time: Option<f32> = None;
        for &(line, key, value) in &entries {
            match key {
                "track" => {
                    let track = parse_track(value).map_err(|detail| error(line, detail))?;
                    if !paths.insert(track.path.clone()) {
                        return Err(error(line, format!("duplicate track for `{}`", track.path)));
                    }
                    for keyframe in &track.keyframes {
                        if keyframe.time > duration {
                            return Err(error(
                                line,
                                format!(
                                    "keyframe time {} exceeds duration {duration}",
                                    keyframe.time
                                ),
                            ));
                        }
                    }
                    tracks.push(track);
                }
                "event" => {
                    let event = parse_event(value).map_err(|detail| error(line, detail))?;
                    if event.time > duration {
                        return Err(error(
                            line,
                            format!("event time {} exceeds duration {duration}", event.time),
                        ));
                    }
                    if let Some(previous) = previous_event_time
                        && event.time < previous
                    {
                        return Err(error(
                            line,
                            format!(
                                "event times must be non-decreasing: {previous} -> {}",
                                event.time
                            ),
                        ));
                    }
                    if events.iter().any(|existing: &AnimEvent| {
                        existing.time == event.time && existing.name == event.name
                    }) {
                        return Err(error(
                            line,
                            format!("duplicate event `{}` at {}", event.name, event.time),
                        ));
                    }
                    previous_event_time = Some(event.time);
                    events.push(event);
                }
                _ => {}
            }
        }

        Ok(Self {
            name,
            duration,
            looped,
            tracks,
            events,
        })
    }

    /// 从合法剪辑编码出规范文本（与模块文档的示例同一形状）。
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "version={ANIM_ASSET_VERSION}");
        let _ = writeln!(out, "name={}", self.name);
        let _ = writeln!(out, "duration={}", format_number(self.duration));
        let _ = writeln!(out, "looped={}", self.looped);
        for track in &self.tracks {
            let _ = write!(
                out,
                "track={};interp={};keys=",
                track.path,
                interpolation_text(track.interpolation)
            );
            for (index, keyframe) in track.keyframes.iter().enumerate() {
                if index > 0 {
                    out.push(';');
                }
                out.push_str(&format_number(keyframe.time));
                out.push(':');
                keyframe.value.write_text(&mut out);
            }
            out.push('\n');
        }
        for event in &self.events {
            let _ = writeln!(out, "event={}:{}", format_number(event.time), event.name);
        }
        out
    }

    /// 某个属性路径的轨道。
    #[must_use]
    pub fn track(&self, path: &AnimPath) -> Option<&AnimTrack> {
        self.tracks.iter().find(|track| &track.path == path)
    }

    /// 采样某个属性路径（没有该轨道 ⇒ `None`）。
    #[must_use]
    pub fn sample(&self, path: &AnimPath, time: f32) -> Option<AnimValue> {
        self.track(path)?.sample(time)
    }

    /// 把时间折回 `[0, duration)`（循环）或夹到 `[0, duration]`（不循环）。
    ///
    /// 循环档在 `time == duration` 处回到 `0`：这是"下一圈的开头"，不是把最后一帧停住。
    #[must_use]
    pub fn wrap_time(&self, time: f32) -> f32 {
        if !time.is_finite() || time <= 0.0 {
            return 0.0;
        }
        if self.looped && self.duration > 0.0 {
            let wrapped = time % self.duration;
            if wrapped < 0.0 {
                wrapped + self.duration
            } else {
                wrapped
            }
        } else {
            time.min(self.duration.max(0.0))
        }
    }

    /// `[from, to]` 内的事件（闭区间），按时间顺序返回一次。
    ///
    /// 循环剪辑跨过 `duration` 时，`from..duration` 与 `0..to-duration` 两段都算到达过——与
    /// [`crate::animation_track::EventTrack::events_in_interval`] 同一语义。调用方应保证单步
    /// 跨度小于 `duration`；跨度覆盖多圈时这里也只返回每个事件一次（不猜测要补几次）。
    #[must_use]
    pub fn events_in_interval(&self, from: f32, to: f32) -> Vec<AnimEvent> {
        if !from.is_finite() || !to.is_finite() || to < from {
            return Vec::new();
        }
        let wrapped = self.looped && self.duration > 0.0 && to >= self.duration;
        let wrapped_to = if wrapped { to - self.duration } else { to };
        self.events
            .iter()
            .filter(|event| {
                if wrapped {
                    event.time >= from || event.time <= wrapped_to
                } else {
                    event.time >= from && event.time <= to
                }
            })
            .cloned()
            .collect()
    }
}

/// `.anim` 解析错误（含源路径与行号）。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("anim {path}: line {line}: {detail}")]
pub struct AnimAssetError {
    pub path: String,
    pub line: usize,
    pub detail: String,
}

fn parse_track(raw: &str) -> Result<AnimTrack, String> {
    let mut segments = raw.split(';');
    let path_raw = segments.next().unwrap_or_default().trim();
    if path_raw.is_empty() {
        return Err("track needs a property path before `;`".to_owned());
    }
    let path = AnimPath::parse(path_raw)?;
    let mut interpolation: Option<Interpolation> = None;
    let mut keyframes: Vec<AnimKeyframe> = Vec::new();
    let mut saw_keys = false;
    for segment in segments {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err("empty track segment (stray `;`)".to_owned());
        }
        if let Some(raw_interpolation) = segment.strip_prefix("interp=") {
            if interpolation.is_some() {
                return Err("duplicate `interp=`".to_owned());
            }
            interpolation = Some(parse_interpolation(raw_interpolation.trim())?);
        } else if let Some(raw_keys) = segment.strip_prefix("keys=") {
            if saw_keys {
                return Err("duplicate `keys=`".to_owned());
            }
            saw_keys = true;
            keyframes.push(parse_keyframe(&path, raw_keys.trim())?);
        } else if saw_keys {
            keyframes.push(parse_keyframe(&path, segment)?);
        } else {
            return Err(format!("expected `interp=` or `keys=`, got `{segment}`"));
        }
    }
    let interpolation = interpolation.ok_or_else(|| "track is missing `interp=`".to_owned())?;
    if !saw_keys {
        return Err("track is missing `keys=`".to_owned());
    }
    if path.is_discrete() && interpolation != Interpolation::Step {
        return Err(format!(
            "`{path}` is discrete and only accepts step interpolation"
        ));
    }
    for pair in keyframes.windows(2) {
        if pair[1].time <= pair[0].time {
            return Err(format!(
                "keyframe times must be strictly increasing: {} -> {}",
                pair[0].time, pair[1].time
            ));
        }
    }
    Ok(AnimTrack {
        path,
        interpolation,
        keyframes,
    })
}

fn parse_keyframe(path: &AnimPath, raw: &str) -> Result<AnimKeyframe, String> {
    let Some((raw_time, raw_value)) = raw.split_once(':') else {
        return Err(format!("keyframe must be `time:value`, got `{raw}`"));
    };
    if raw_value.contains(':') {
        return Err(format!("keyframe value contains a stray `:`, got `{raw}`"));
    }
    let time = parse_scalar(raw_time)?;
    if time < 0.0 {
        return Err(format!("keyframe time must be >= 0, got {time}"));
    }
    Ok(AnimKeyframe {
        time,
        value: path.parse_value(raw_value)?,
    })
}

fn parse_event(raw: &str) -> Result<AnimEvent, String> {
    let Some((raw_time, raw_name)) = raw.split_once(':') else {
        return Err(format!("event must be `time:name`, got `{raw}`"));
    };
    if raw_name.contains(':') {
        return Err(format!("event name contains a stray `:`, got `{raw}`"));
    }
    let time = parse_scalar(raw_time)?;
    if time < 0.0 {
        return Err(format!("event time must be >= 0, got {time}"));
    }
    let name = raw_name.trim();
    if name.is_empty() {
        return Err("event name is empty".to_owned());
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c == ';' || c == '=')
    {
        return Err(format!(
            "event name must not contain whitespace, `;` or `=`, got `{name}`"
        ));
    }
    Ok(AnimEvent {
        time,
        name: name.to_owned(),
    })
}

fn parse_interpolation(raw: &str) -> Result<Interpolation, String> {
    match raw {
        "step" => Ok(Interpolation::Step),
        "linear" => Ok(Interpolation::Linear),
        _ => Err(format!("unknown interpolation `{raw}` (step|linear)")),
    }
}

fn parse_numbers(raw: &str) -> Result<Vec<f32>, String> {
    raw.split(',').map(parse_scalar).collect()
}

fn parse_scalar(raw: &str) -> Result<f32, String> {
    let value = raw
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("`{raw}` is not a number"))?;
    if !value.is_finite() {
        return Err(format!("number must be finite, got `{raw}`"));
    }
    Ok(value)
}

fn require_arity(path: &AnimPath, numbers: &[f32], expected: usize) -> Result<(), String> {
    if numbers.len() != expected {
        return Err(format!(
            "`{path}` expects {expected} numbers, got {}",
            numbers.len()
        ));
    }
    Ok(())
}

fn interpolation_text(interpolation: Interpolation) -> &'static str {
    match interpolation {
        Interpolation::Step => "step",
        Interpolation::Linear => "linear",
    }
}

/// 最短往返形式的 `f32`（`-0.0` 归一成 `0`），保证 `to_text` → `from_text` 逐值不变。
fn format_number(value: f32) -> String {
    if value == 0.0 {
        "0".to_owned()
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "\
version=1
name=hero_run
duration=1.2
looped=true
track=Transform.position;interp=linear;keys=0:0,0;0.6:0,1.5;1.2:0,0
track=Sprite.region;interp=step;keys=0:0;0.2:1;0.4:2
event=0.6:footstep
";

    fn example() -> AnimClip {
        AnimClip::from_text("hero.anim", EXAMPLE).expect("documented example parses")
    }

    fn with_body(body: &str) -> String {
        format!("version=1\nname=t\nduration=1\nlooped=false\n{body}\n")
    }

    fn rejection(text: &str, needle: &str) {
        let error = AnimClip::from_text("bad.anim", text).expect_err("must be rejected");
        assert!(
            error.detail.contains(needle),
            "expected `{needle}` in error, got: {error}"
        );
        assert_eq!(error.path, "bad.anim");
        assert!(error.line >= 1, "错误必须带行号：{error}");
    }

    /// 文档里的示例必须**逐字节**往返：示例是资产格式的契约，不是示意图。
    #[test]
    fn documented_example_round_trips_byte_for_byte() {
        let clip = example();
        assert_eq!(clip.name, "hero_run");
        assert!((clip.duration - 1.2).abs() < 1e-6);
        assert!(clip.looped);
        assert_eq!(clip.tracks.len(), 2);
        assert_eq!(clip.events.len(), 1);
        assert_eq!(clip.events[0].name, "footstep");
        assert_eq!(clip.to_text(), EXAMPLE);
        assert_eq!(
            AnimClip::from_text("hero.anim", &clip.to_text()).expect("reparse"),
            clip
        );
    }

    /// Step 取值、Linear 中点、边界夹取与循环折回。
    #[test]
    fn step_and_linear_sampling_hit_endpoints_and_midpoints() {
        let clip = example();
        let position = AnimPath::Transform(TransformChannel::Position);
        match clip.sample(&position, 0.3).expect("position") {
            AnimValue::Vec2(value) => {
                assert!(value.x.abs() < 1e-6, "x 应保持 0：{value:?}");
                assert!((value.y - 0.75).abs() < 1e-6, "y 中点应为 0.75：{value:?}");
            }
            other => panic!("expected Vec2, got {other:?}"),
        }
        assert_eq!(
            clip.sample(&position, 0.6),
            Some(AnimValue::Vec2(Vec2::new(0.0, 1.5)))
        );
        assert_eq!(
            clip.sample(&position, -1.0),
            Some(AnimValue::Vec2(Vec2::new(0.0, 0.0))),
            "早于首键取首值"
        );
        assert_eq!(
            clip.sample(&position, 9.0),
            Some(AnimValue::Vec2(Vec2::new(0.0, 0.0))),
            "晚于末键取末值（折回由 wrap_time 负责）"
        );

        let region = AnimPath::SpriteRegion;
        assert_eq!(clip.sample(&region, 0.0), Some(AnimValue::Region(0)));
        assert_eq!(
            clip.sample(&region, 0.19),
            Some(AnimValue::Region(0)),
            "帧内保持不变"
        );
        assert_eq!(
            clip.sample(&region, 0.2),
            Some(AnimValue::Region(1)),
            "边界属于后一帧"
        );
        assert_eq!(clip.sample(&region, 1.0), Some(AnimValue::Region(2)));

        assert!((clip.wrap_time(1.3) - 0.1).abs() < 1e-6);
        assert_eq!(clip.wrap_time(1.2), 0.0, "恰好一圈回到开头");
        assert_eq!(clip.wrap_time(-0.5), 0.0);
        assert_eq!(clip.wrap_time(f32::NAN), 0.0);
    }

    /// 3D 位置线性插值 + 四元数归一化；`to_text` 写回的是归一化后的值。
    #[test]
    fn spatial_tracks_interpolate_and_normalize_quaternions() {
        let text = with_body(
            "track=Transform3D.position;interp=linear;keys=0:0,0,0;1:2,4,6\n\
             track=Transform3D.rotation;interp=linear;keys=0:0,0,0,1;1:0,0,0,2",
        );
        let clip = AnimClip::from_text("spatial.anim", &text).expect("parses");
        assert_eq!(
            clip.sample(&AnimPath::Transform3D(TransformChannel::Position), 0.5),
            Some(AnimValue::Vec3(Vec3::new(1.0, 2.0, 3.0)))
        );
        match clip
            .sample(&AnimPath::Transform3D(TransformChannel::Rotation), 0.5)
            .expect("rotation")
        {
            AnimValue::Quat(value) => {
                assert!(value.is_unit(), "采样结果必须是单位四元数：{value:?}");
                assert!(value.w.abs() > 0.99, "同向四元数不应产生翻转：{value:?}");
            }
            other => panic!("expected Quat, got {other:?}"),
        }
        assert!(
            clip.to_text().contains("keys=0:0,0,0,1;1:0,0,0,1"),
            "解析时归一化，写回是单位四元数：{}",
            clip.to_text()
        );
    }

    /// 循环回绕的事件只返回一次；不循环的同一区间什么也不返回。
    #[test]
    fn events_fire_once_across_a_loop_wrap() {
        let clip = example();
        assert!(clip.events_in_interval(0.0, 0.5).is_empty());
        let due = clip.events_in_interval(0.6, 0.6);
        assert_eq!(due.len(), 1, "闭区间端点要触发");
        let wrapped = clip.events_in_interval(1.1, 1.9);
        assert_eq!(wrapped.len(), 1, "回绕后每个事件只触发一次：{wrapped:?}");
        assert_eq!(wrapped[0].name, "footstep");

        let non_looping = AnimClip::from_text(
            "x.anim",
            "version=1\nname=t\nduration=1\nlooped=false\nevent=0.6:footstep\n",
        )
        .expect("parses");
        assert!(non_looping.events_in_interval(1.1, 1.9).is_empty());
    }

    /// 头字段：版本/必填项/未知字段/布尔值与重复字段。
    #[test]
    fn rejects_broken_headers_and_unknown_fields() {
        rejection(
            "version=2\nname=t\nduration=1\n",
            "unsupported anim version",
        );
        rejection("name=t\nduration=1\n", "missing `version=1`");
        rejection("version=1\nduration=1\n", "missing `name=`");
        rejection("version=1\nname=t\n", "missing `duration=`");
        rejection(&with_body("mystery=1"), "unknown field `mystery`");
        rejection(
            "version=1\nname=t\nduration=1\nlooped=yes\n",
            "`looped` must be true or false",
        );
        rejection(
            "version=1\nversion=1\nname=t\nduration=1\n",
            "duplicate `version`",
        );
        rejection(
            "version=1\nname=t\nduration=0\n",
            "`duration` must be positive",
        );
    }

    /// 轨道：乱序/重复/越界时间、未知路径、重复轨道、坏插值、区域插值。
    #[test]
    fn rejects_broken_track_shapes() {
        rejection(
            &with_body("track=Transform.position;interp=linear;keys=0:0,0;0:1,1"),
            "strictly increasing",
        );
        rejection(
            &with_body("track=Transform.position;interp=linear;keys=0.5:0,0;0.2:1,1"),
            "strictly increasing",
        );
        rejection(
            &with_body("track=Transform.position;interp=linear;keys=0:0,0;2:1,1"),
            "exceeds duration",
        );
        rejection(
            &with_body("track=Nope.position;interp=linear;keys=0:0"),
            "unsupported track path",
        );
        rejection(
            &with_body(
                "track=Transform.position;interp=linear;keys=0:0,0\n\
                 track=Transform.position;interp=linear;keys=0:0,0",
            ),
            "duplicate track",
        );
        rejection(
            &with_body("track=Transform.position;interp=smooth;keys=0:0,0"),
            "unknown interpolation",
        );
        rejection(
            &with_body("track=Sprite.region;interp=linear;keys=0:0;0.5:1"),
            "only accepts step",
        );
        rejection(
            &with_body("track=Transform.position;keys=0:0,0"),
            "missing `interp=`",
        );
        rejection(
            &with_body("track=Transform.position;interp=linear;interp=step;keys=0:0,0"),
            "duplicate `interp=`",
        );
    }

    /// 值：元数、非有限数、负时间、区域帧号、灯光强度、四元数零值。
    #[test]
    fn rejects_broken_values() {
        rejection(
            &with_body("track=Transform.position;interp=linear;keys=0:0"),
            "expects 2 numbers",
        );
        rejection(
            &with_body("track=Transform3D.position;interp=linear;keys=0:0,0"),
            "expects 3 numbers",
        );
        rejection(
            &with_body("track=Transform3D.rotation;interp=linear;keys=0:0,0,0"),
            "expects 4 numbers",
        );
        rejection(
            &with_body("track=Transform3D.rotation;interp=linear;keys=0:0,0,0,0"),
            "quaternion must be non-zero",
        );
        rejection(
            &with_body("track=Transform.rotation;interp=linear;keys=0:inf"),
            "must be finite",
        );
        rejection(
            &with_body("track=Transform.rotation;interp=linear;keys=-1:0"),
            "must be >= 0",
        );
        rejection(
            &with_body("track=Sprite.region;interp=step;keys=0:1.5"),
            "non-negative integer",
        );
        rejection(
            &with_body("track=Light.intensity;interp=linear;keys=0:-1"),
            "must be non-negative",
        );
        rejection(
            &with_body("track=UiAnimation.;interp=linear;keys=0:1"),
            "invalid UiAnimation property",
        );
        rejection(
            &with_body("track=UiAnimation.opacity;interp=linear;keys=0:1,2"),
            "expects 1 numbers",
        );
    }

    /// 事件：坏形状、乱序、重复、越界、非法名字。
    #[test]
    fn rejects_broken_events() {
        rejection(&with_body("event=0.5"), "event must be `time:name`");
        rejection(&with_body("event=0.5:"), "event name is empty");
        rejection(&with_body("event=0.5:a b"), "must not contain whitespace");
        rejection(&with_body("event=0.5:a=1"), "must not contain whitespace");
        rejection(&with_body("event=0.8:a\nevent=0.2:b"), "non-decreasing");
        rejection(&with_body("event=0.5:a\nevent=0.5:a"), "duplicate event");
        rejection(&with_body("event=2:a"), "exceeds duration");
    }

    /// `looped` 缺省 false；无轨道但有事件是合法剪辑；`-0` 归一成 `0`。
    #[test]
    fn defaults_and_edge_shapes_are_explicit() {
        let clip = AnimClip::from_text(
            "events.anim",
            "version=1\nname=only_events\nduration=1\nevent=0.1:a\n",
        )
        .expect("parses");
        assert!(!clip.looped);
        assert!(clip.tracks.is_empty());
        assert_eq!(clip.events.len(), 1);
        assert_eq!(
            clip.wrap_time(5.0),
            1.0,
            "不循环剪辑夹在 duration，而不是折回"
        );

        let text = with_body("track=Transform.rotation;interp=linear;keys=0:-0;1:0");
        let clip = AnimClip::from_text("-0.anim", &text).expect("parses");
        assert!(clip.to_text().contains("keys=0:0;1:0"));
    }
}
