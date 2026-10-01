//! docs/54 §4.4：**统一的动画轨道抽象**（属性轨道 + 事件轨道）。
//!
//! §4.4 的原文是"动画统一为属性轨道和事件轨道：2D 帧动画是 Sprite region 轨道，2D 骨骼动画是
//! Transform 轨道，3D 骨骼动画是 pose/palette 轨道，底层采样器可继续专用"。
//!
//! 此前每一种动画都有自己的说法与采样器：2D 帧动画是 `AnimationClip { cols, frame_rate, .. }` 里的
//! 取模算术，帧事件是 `advance()` 里的区间比较，3D 骨骼动画又是另一套 palette。它们**行为上都对**，
//! 但"一条动画 = 若干条按时间采样的轨道"这个说法只存在于文档里：想统一查询/工具化（编辑器时间轴、
//! 烘焙、跨类型事件）就得一处一处猜各自的规则。
//!
//! 本模块把那个说法变成类型：
//!
//! - [`crate::core::animation_track::PropertyTrack`]：`目标 + 关键帧 + 插值方式`。离散目标（Sprite region / 骨骼 pose）只允许
//!   `Step`——对 UV 区域做"插值"没有意义，静默允许会让美术看到中间的杂乱帧。连续目标
//!   （Transform）才允许 `Linear`。
//! - [`crate::core::animation_track::EventTrack`]：按时间点触发的事件。**事件不是"第几帧"**：帧只是 Sprite region 轨道上的一种
//!   量化，因此事件自带源数据序号（[`crate::core::animation_track::EventMarker::order`]）供"每轮只触发一次"的去重使用。
//! - [`crate::core::animation_track::AnimationTracks`]：一组轨道 + 采样查询。
//!
//! 与既有实现的关系是**视图而非替代**：`AnimationClip::tracks()` 把帧动画表达成轨道，
//! `AnimationStateMachine` 的 UV 采样与帧事件触发都改走这一份实现，于是"帧→UV"与"帧事件落在哪一步"
//! 这两条规则各自只有一处定义。3D 骨骼的 palette 采样器继续专用（§4.4 允许），但它的轨道形态
//! （`TrackValue::Pose`）已经能表达进同一套类型里。

use std::borrow::Cow;
use std::fmt;

use crate::core::{
    math3d::{Mat4, Vec3},
    transform::{Transform, Transform3D, Vec2},
};

/// 轨道作用的目标（§4.4 里"属性轨道"的四类）。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TrackTarget {
    /// 2D 帧动画的纹理区域（`SpriteRegion`）。
    SpriteRegion,
    /// 2D 变换属性。
    PlaneTransform,
    /// 3D 变换属性。
    SpatialTransform,
    /// 骨骼 palette（3D 骨骼动画）。
    SkeletonPose,
}

impl TrackTarget {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpriteRegion => "sprite_region",
            Self::PlaneTransform => "plane_transform",
            Self::SpatialTransform => "spatial_transform",
            Self::SkeletonPose => "skeleton_pose",
        }
    }

    /// 该目标是否只接受离散采样（插值对它没有意义）。
    #[must_use]
    pub const fn is_discrete(self) -> bool {
        matches!(self, Self::SpriteRegion | Self::SkeletonPose)
    }
}

/// 关键帧的值。刻意用一个枚举而不是泛型：轨道要能放进同一个列表里被统一查询（编辑器时间轴、
/// 烘焙、跨类型事件），泛型会把这个列表变成"每种类型一个列表"。
#[derive(Clone, Debug, PartialEq)]
pub enum TrackValue {
    /// 纹理区域（归一化 UV）。
    Region {
        uv_min: Vec2,
        uv_max: Vec2,
    },
    Plane(Transform),
    Spatial(Transform3D),
    /// 骨骼 palette（每个关节一个矩阵）。
    Pose(Vec<Mat4>),
}

impl TrackValue {
    /// 该值与目标是否匹配——不匹配的轨道是数据错误，必须报出来而不是渲染成空白。
    #[must_use]
    pub fn matches(&self, target: TrackTarget) -> bool {
        matches!(
            (self, target),
            (Self::Region { .. }, TrackTarget::SpriteRegion)
                | (Self::Plane(_), TrackTarget::PlaneTransform)
                | (Self::Spatial(_), TrackTarget::SpatialTransform)
                | (Self::Pose(_), TrackTarget::SkeletonPose)
        )
    }
}

/// 关键帧之间的插值方式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Interpolation {
    /// 取前一个关键帧的值（帧动画、骨骼 pose 用）。
    Step,
    /// 位置/缩放线性、旋转球面插值（Transform 轨道用）。
    Linear,
}

/// 一个关键帧。
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub value: TrackValue,
}

/// 属性轨道：一个目标随时间的取值。
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyTrack {
    pub target: TrackTarget,
    pub interpolation: Interpolation,
    pub keyframes: Vec<Keyframe>,
}

impl PropertyTrack {
    /// 在 `time` 采样。`time` 早于第一个关键帧 ⇒ 取第一个（不外推）；晚于最后一个 ⇒ 取最后一个
    /// （循环由调用方按 `duration` 折回，轨道本身不管循环策略）。
    ///
    /// 返回值用 `Cow`：`Step`（离散目标，包括帧动画这类热路径）**借用**原关键帧、不克隆；只有
    /// `Linear` 的中间帧才需要算出一个新值。
    #[must_use]
    pub fn sample(&self, time: f32) -> Option<Cow<'_, TrackValue>> {
        let first = self.keyframes.first()?;
        if time <= first.time {
            return Some(Cow::Borrowed(&first.value));
        }
        let last = self.keyframes.last()?;
        if time >= last.time {
            return Some(Cow::Borrowed(&last.value));
        }
        // 找到 `time` 落在哪两个关键帧之间（关键帧数量小，线性扫描足够；这里不引入二分以免
        // 采样与"顺序"两条规则分家）。
        let mut previous = first;
        for keyframe in &self.keyframes[1..] {
            if time < keyframe.time {
                return match self.interpolation {
                    Interpolation::Step => Some(Cow::Borrowed(&previous.value)),
                    Interpolation::Linear => {
                        let span = keyframe.time - previous.time;
                        // 两个关键帧同一时刻（`span == 0`）时用 0 比例：取前一个值，不外推。
                        let alpha = if span > 0.0 {
                            ((time - previous.time) / span).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        Some(Cow::Owned(previous.value.lerp(&keyframe.value, alpha)))
                    }
                };
            }
            previous = keyframe;
        }
        Some(Cow::Borrowed(&previous.value))
    }
}

impl TrackValue {
    /// 两个值之间的 `Linear` 插值（`alpha` 已由调用方按时间归一化）。
    ///
    /// 离散目标不可能走到这里（[`AnimationTracks::validate`] 会拒绝 Step 之外的插值方式），
    /// 因此这里只需处理 Transform；`Pose` 真被走到说明校验被绕过，退回前一个值而不是猜。
    fn lerp(&self, next: &Self, alpha: f32) -> Self {
        match (self, next) {
            (Self::Plane(from), Self::Plane(to)) => Self::Plane(lerp_plane(*from, *to, alpha)),
            (Self::Spatial(from), Self::Spatial(to)) => {
                Self::Spatial(lerp_spatial(*from, *to, alpha))
            }
            (value, _) => value.clone(),
        }
    }
}

fn lerp_plane(from: Transform, to: Transform, alpha: f32) -> Transform {
    Transform {
        position: lerp_vec2(from.position, to.position, alpha),
        rotation: from.rotation + (to.rotation - from.rotation) * alpha,
        scale: lerp_vec2(from.scale, to.scale, alpha),
    }
}

fn lerp_spatial(from: Transform3D, to: Transform3D, alpha: f32) -> Transform3D {
    Transform3D {
        position: lerp_vec3(from.position, to.position, alpha),
        rotation: lerp_quat(from.rotation, to.rotation, alpha),
        scale: lerp_vec3(from.scale, to.scale, alpha),
    }
}

fn lerp_vec2(from: Vec2, to: Vec2, alpha: f32) -> Vec2 {
    Vec2::new(
        from.x + (to.x - from.x) * alpha,
        from.y + (to.y - from.y) * alpha,
    )
}

fn lerp_vec3(from: Vec3, to: Vec3, alpha: f32) -> Vec3 {
    from + (to - from) * alpha
}

/// 四元数插值：先做最短弧校正（点积为负时取反）再归一化线性插值。
///
/// 刻意不用"球面插值"的近似说法：工程里没有 `slerp`，而 nlerp 在动画关键帧这种小角度场景下与
/// 它视觉等价，且**不静默产生非单位四元数**（归一化失败时退回起点）。
fn lerp_quat(
    from: crate::core::math3d::Quat,
    to: crate::core::math3d::Quat,
    alpha: f32,
) -> crate::core::math3d::Quat {
    let dot = from.x * to.x + from.y * to.y + from.z * to.z + from.w * to.w;
    let to = if dot < 0.0 {
        crate::core::math3d::Quat {
            x: -to.x,
            y: -to.y,
            z: -to.z,
            w: -to.w,
        }
    } else {
        to
    };
    // `Quat` 没有 `lerp`：按分量算（`Vec3::lerp` 是同一形式的四分量版本）。
    let blended = crate::core::math3d::Quat {
        x: from.x + (to.x - from.x) * alpha,
        y: from.y + (to.y - from.y) * alpha,
        z: from.z + (to.z - from.z) * alpha,
        w: from.w + (to.w - from.w) * alpha,
    };
    blended.normalized().unwrap_or(from)
}

/// 一条事件：在 `time` 触发 `name`；`order` 是它在**源数据**里的序号（去重与诊断用）。
///
/// 事件不写"第几帧"：帧只是 Sprite region 轨道上的量化，写在事件上会让别的轨道类型无法复用。
#[derive(Clone, Debug, PartialEq)]
pub struct EventMarker {
    pub time: f32,
    pub name: String,
    pub order: usize,
}

/// 事件轨道：按时间点触发的事件集合。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventTrack {
    pub events: Vec<EventMarker>,
}

impl EventTrack {
    /// `[from, to]` 区间内的事件，按时间升序。
    ///
    /// `looped && to >= duration` 表示这一步跨过了循环回绕：`from..duration` 与 `0..to-duration`
    /// 两段都算到达过（与"跨多帧时中间的帧事件全部按顺序触发"同一语义）。区间是**闭**的，且
    /// `from` 由调用方按自己的量化给出（帧动画传"起始帧对应的时刻"）——因此调用方不需要重写
    /// 一遍区间比较，只需说明自己的量化粒度。
    #[must_use]
    pub fn events_in_interval(
        &self,
        from: f32,
        to: f32,
        looped: bool,
        duration: f32,
    ) -> Vec<EventMarker> {
        let wrapped = looped && duration > 0.0 && to >= duration;
        let wrapped_to = if wrapped { to - duration } else { to };
        let mut due: Vec<EventMarker> = self
            .events
            .iter()
            .filter(|event| {
                if wrapped {
                    event.time >= from || event.time <= wrapped_to
                } else {
                    event.time >= from && event.time <= to
                }
            })
            .cloned()
            .collect();
        due.sort_by(|left, right| {
            left.time
                .partial_cmp(&right.time)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.order.cmp(&right.order))
        });
        due
    }
}

/// 一条轨道。
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    Property(PropertyTrack),
    Event(EventTrack),
}

/// 一组轨道（一段可查询的动画）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationTracks {
    pub tracks: Vec<Track>,
}

impl AnimationTracks {
    #[must_use]
    pub fn new(tracks: Vec<Track>) -> Self {
        Self { tracks }
    }

    /// 某个目标的属性轨道。
    #[must_use]
    pub fn property(&self, target: TrackTarget) -> Option<&PropertyTrack> {
        self.tracks.iter().find_map(|track| match track {
            Track::Property(property) if property.target == target => Some(property),
            _ => None,
        })
    }

    /// 采样某个目标（没有该目标的轨道 ⇒ `None`）。
    #[must_use]
    pub fn sample(&self, target: TrackTarget, time: f32) -> Option<Cow<'_, TrackValue>> {
        self.property(target)?.sample(time)
    }

    /// 事件轨道的并集（多条事件轨道按时间合并）。
    #[must_use]
    pub fn events_in_interval(
        &self,
        from: f32,
        to: f32,
        looped: bool,
        duration: f32,
    ) -> Vec<EventMarker> {
        let mut due: Vec<EventMarker> = Vec::new();
        for track in &self.tracks {
            if let Track::Event(events) = track {
                due.extend(events.events_in_interval(from, to, looped, duration));
            }
        }
        due.sort_by(|left, right| {
            left.time
                .partial_cmp(&right.time)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.order.cmp(&right.order))
        });
        due
    }

    /// 校验：时间有限且升序、值与目标匹配、离散目标只允许 Step、轨道不空。
    pub fn validate(&self) -> Result<(), TrackError> {
        for track in &self.tracks {
            match track {
                Track::Property(property) => {
                    if property.keyframes.is_empty() {
                        return Err(TrackError::EmptyTrack(property.target));
                    }
                    if property.target.is_discrete()
                        && property.interpolation != Interpolation::Step
                    {
                        return Err(TrackError::InterpolationNotAllowed(property.target));
                    }
                    let mut previous: Option<f32> = None;
                    for keyframe in &property.keyframes {
                        if !keyframe.time.is_finite() {
                            return Err(TrackError::NonFiniteTime(keyframe.time));
                        }
                        if let Some(previous) = previous
                            && keyframe.time < previous
                        {
                            return Err(TrackError::OutOfOrder {
                                target: property.target,
                                previous,
                                next: keyframe.time,
                            });
                        }
                        if !keyframe.value.matches(property.target) {
                            return Err(TrackError::ValueMismatch(property.target));
                        }
                        previous = Some(keyframe.time);
                    }
                }
                Track::Event(events) => {
                    let mut previous: Option<f32> = None;
                    for event in &events.events {
                        if !event.time.is_finite() {
                            return Err(TrackError::NonFiniteTime(event.time));
                        }
                        if let Some(previous) = previous
                            && event.time < previous
                        {
                            return Err(TrackError::OutOfOrder {
                                target: TrackTarget::SpriteRegion,
                                previous,
                                next: event.time,
                            });
                        }
                        previous = Some(event.time);
                    }
                }
            }
        }
        Ok(())
    }
}

/// 轨道校验错误。
#[derive(Clone, Debug, PartialEq)]
pub enum TrackError {
    EmptyTrack(TrackTarget),
    InterpolationNotAllowed(TrackTarget),
    NonFiniteTime(f32),
    OutOfOrder {
        target: TrackTarget,
        previous: f32,
        next: f32,
    },
    ValueMismatch(TrackTarget),
}

impl fmt::Display for TrackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTrack(target) => {
                write!(formatter, "{} track has no keyframes", target.as_str())
            }
            Self::InterpolationNotAllowed(target) => write!(
                formatter,
                "{} 是离散目标，只允许 Step 插值（插值 UV 区域没有意义）",
                target.as_str()
            ),
            Self::NonFiniteTime(time) => write!(formatter, "track time is not finite: {time}"),
            Self::OutOfOrder {
                target,
                previous,
                next,
            } => write!(
                formatter,
                "{} track keyframes are out of order: {previous} -> {next}",
                target.as_str()
            ),
            Self::ValueMismatch(target) => {
                write!(
                    formatter,
                    "keyframe value does not match {}",
                    target.as_str()
                )
            }
        }
    }
}

impl std::error::Error for TrackError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math3d::Quat;

    fn region(min: f32, max: f32) -> TrackValue {
        TrackValue::Region {
            uv_min: Vec2::new(min, 0.0),
            uv_max: Vec2::new(max, 1.0),
        }
    }

    fn step_regions(times: &[f32]) -> PropertyTrack {
        PropertyTrack {
            target: TrackTarget::SpriteRegion,
            interpolation: Interpolation::Step,
            keyframes: times
                .iter()
                .enumerate()
                .map(|(index, time)| Keyframe {
                    time: *time,
                    value: region(index as f32 / 4.0, (index + 1) as f32 / 4.0),
                })
                .collect(),
        }
    }

    /// 离散目标：落在哪一帧就取那一帧的区域，边界前后都不外推。
    #[test]
    fn step_track_holds_the_previous_keyframe_and_clamps_outside() {
        let track = step_regions(&[0.0, 0.25, 0.5, 0.75]);
        let at = |time: f32| match track.sample(time).expect("sample").as_ref() {
            TrackValue::Region { uv_min, .. } => uv_min.x,
            other => panic!("expected region, got {other:?}"),
        };
        assert_eq!(at(-1.0), 0.0, "早于第一个关键帧取第一个");
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(0.2), 0.0, "帧内保持不变");
        assert_eq!(at(0.25), 0.25, "边界属于后一帧");
        assert_eq!(at(0.9), 0.75);
        assert_eq!(at(10.0), 0.75, "晚于最后一个取最后一个");
    }

    /// 连续目标：线性插值给出中间值；旋转按最短弧。
    #[test]
    fn linear_track_interpolates_transforms() {
        let track = PropertyTrack {
            target: TrackTarget::PlaneTransform,
            interpolation: Interpolation::Linear,
            keyframes: vec![
                Keyframe {
                    time: 0.0,
                    value: TrackValue::Plane(Transform {
                        position: Vec2::new(0.0, 0.0),
                        rotation: 0.0,
                        scale: Vec2::new(1.0, 1.0),
                    }),
                },
                Keyframe {
                    time: 1.0,
                    value: TrackValue::Plane(Transform {
                        position: Vec2::new(10.0, 20.0),
                        rotation: std::f32::consts::PI,
                        scale: Vec2::new(3.0, 3.0),
                    }),
                },
            ],
        };
        match track.sample(0.5).expect("sample").as_ref() {
            TrackValue::Plane(value) => {
                assert!((value.position.x - 5.0).abs() < 1e-4);
                assert!((value.position.y - 10.0).abs() < 1e-4);
                assert!((value.scale.x - 2.0).abs() < 1e-4);
                assert!((value.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
            }
            other => panic!("expected plane transform, got {other:?}"),
        }
    }

    /// 3D 轨道：旋转走最短弧（点积为负时取反），结果仍是单位四元数。
    #[test]
    fn linear_track_interpolates_rotations_along_the_short_arc() {
        let from = Transform3D {
            position: Vec3::ZERO,
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let to = Transform3D {
            position: Vec3::new(0.0, 0.0, 0.0),
            // 与 identity 点积为负的等价旋转（-identity）：必须走最短弧而不是转一整圈。
            rotation: Quat {
                x: -0.0,
                y: -0.0,
                z: -0.0,
                w: -1.0,
            },
            scale: Vec3::new(1.0, 1.0, 1.0),
        };
        let track = PropertyTrack {
            target: TrackTarget::SpatialTransform,
            interpolation: Interpolation::Linear,
            keyframes: vec![
                Keyframe {
                    time: 0.0,
                    value: TrackValue::Spatial(from),
                },
                Keyframe {
                    time: 1.0,
                    value: TrackValue::Spatial(to),
                },
            ],
        };
        match track.sample(0.5).expect("sample").as_ref() {
            TrackValue::Spatial(value) => {
                assert!(value.rotation.is_unit(), "插值结果必须是单位四元数");
                assert!(
                    value.rotation.w.abs() > 0.9,
                    "最短弧上不应该出现 90° 的中间姿态：{:?}",
                    value.rotation
                );
            }
            other => panic!("expected spatial transform, got {other:?}"),
        }
    }

    /// 事件区间：闭区间、回绕两段、按时间升序。
    #[test]
    fn event_track_reports_due_events_in_time_order() {
        let track = EventTrack {
            events: vec![
                EventMarker {
                    time: 0.1,
                    name: "step".into(),
                    order: 0,
                },
                EventMarker {
                    time: 0.3,
                    name: "hit".into(),
                    order: 1,
                },
                EventMarker {
                    time: 0.9,
                    name: "dust".into(),
                    order: 2,
                },
            ],
        };
        let names = |events: Vec<EventMarker>| -> Vec<String> {
            events.into_iter().map(|event| event.name).collect()
        };
        assert_eq!(
            names(track.events_in_interval(0.1, 0.3, false, 1.0)),
            vec!["step".to_owned(), "hit".to_owned()],
            "闭区间两端都要触发"
        );
        assert_eq!(
            names(track.events_in_interval(0.2, 0.4, false, 1.0)),
            vec!["hit".to_owned()]
        );
        // 回绕：0.9 → 1.1 覆盖了 0.9（尾）与 0.1（头）。
        assert_eq!(
            names(track.events_in_interval(0.9, 1.1, true, 1.0)),
            vec!["step".to_owned(), "dust".to_owned()],
            "回绕时两段都算到达过，且按时间升序（头在前）"
        );
        // 不循环时超出时长不会"回到开头"：0.95 → 1.1 什么都不到（循环时同一区间会带上 0.1）。
        assert!(track.events_in_interval(0.95, 1.1, false, 1.0).is_empty());
        assert_eq!(
            names(track.events_in_interval(0.95, 1.1, true, 1.0)),
            vec!["step".to_owned()],
            "循环时同一区间应带上回绕后的 0.1"
        );
    }

    /// 校验：离散目标拒绝插值、乱序/非有限时间、值与目标不匹配、空轨道各自报错。
    #[test]
    fn validation_rejects_ambiguous_or_broken_tracks() {
        let mut tracks = AnimationTracks::new(vec![Track::Property(PropertyTrack {
            target: TrackTarget::SpriteRegion,
            interpolation: Interpolation::Linear,
            keyframes: vec![Keyframe {
                time: 0.0,
                value: region(0.0, 0.25),
            }],
        })]);
        assert_eq!(
            tracks.validate(),
            Err(TrackError::InterpolationNotAllowed(
                TrackTarget::SpriteRegion
            )),
            "对 UV 区域插值必须被拒绝"
        );

        tracks.tracks = vec![Track::Property(PropertyTrack {
            target: TrackTarget::PlaneTransform,
            interpolation: Interpolation::Step,
            keyframes: vec![Keyframe {
                time: 0.0,
                value: TrackValue::Plane(Transform {
                    position: Vec2::new(0.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::new(1.0, 1.0),
                }),
            }],
        })];
        assert!(tracks.validate().is_ok());

        // 值与目标不匹配（区域塞进 Transform 轨道）。
        tracks.tracks = vec![Track::Property(PropertyTrack {
            target: TrackTarget::PlaneTransform,
            interpolation: Interpolation::Step,
            keyframes: vec![Keyframe {
                time: 0.0,
                value: region(0.0, 1.0),
            }],
        })];
        assert_eq!(
            tracks.validate(),
            Err(TrackError::ValueMismatch(TrackTarget::PlaneTransform))
        );

        // 乱序时间。
        tracks.tracks = vec![Track::Property(PropertyTrack {
            target: TrackTarget::SpriteRegion,
            interpolation: Interpolation::Step,
            keyframes: vec![
                Keyframe {
                    time: 0.5,
                    value: region(0.0, 0.5),
                },
                Keyframe {
                    time: 0.25,
                    value: region(0.5, 1.0),
                },
            ],
        })];
        assert!(matches!(
            tracks.validate(),
            Err(TrackError::OutOfOrder { .. })
        ));

        // 空轨道与非有限时间。
        tracks.tracks = vec![Track::Property(PropertyTrack {
            target: TrackTarget::SpriteRegion,
            interpolation: Interpolation::Step,
            keyframes: Vec::new(),
        })];
        assert_eq!(
            tracks.validate(),
            Err(TrackError::EmptyTrack(TrackTarget::SpriteRegion))
        );
        tracks.tracks = vec![Track::Event(EventTrack {
            events: vec![EventMarker {
                time: f32::NAN,
                name: "x".into(),
                order: 0,
            }],
        })];
        assert!(matches!(
            tracks.validate(),
            Err(TrackError::NonFiniteTime(_))
        ));
    }

    /// `sample` 对离散目标是**借用**（帧动画热路径不该因为引入轨道视图而多一次克隆）。
    #[test]
    fn step_sampling_borrows_instead_of_cloning() {
        let track = step_regions(&[0.0, 0.25]);
        let sampled = track.sample(0.1).expect("sample");
        assert!(
            matches!(sampled, Cow::Borrowed(_)),
            "Step 采样必须借用原关键帧"
        );
    }
}
