//! Per-entity animation state machines: named states, each with a sprite-sheet
//! clip (1xN frame strip), frame-rate, and loop policy. Lua reads and switches
//! states; the renderer samples the current frame's UV region.

use crate::core::transform::Vec2;

/// One state's animation: a `1 x cols` frame strip sampled at `frame_rate` FPS.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub sheet: String,
    pub cols: u32,
    pub frame_rate: f32,
    pub looped: bool,
    /// 帧事件（WP4 ⑦）：命中判定、脚步、尘土都挂在具体帧上。帧号必须是 `0..cols`，
    /// 越界在解析时就报错——静默丢弃会让"攻击判定帧"神秘消失。
    pub events: Vec<AnimationEvent>,
}

/// 一条帧事件：播放到 `frame` 这一帧时触发一次。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationEvent {
    pub frame: u32,
    pub name: String,
}

/// 一帧里真正触发的事件（宿主把它交给 Lua）。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationFrameEvent {
    pub frame: u32,
    pub name: String,
}

impl AnimationClip {
    /// docs/54 §4.4：把帧动画表达成**统一的轨道视图**——一条 `SpriteRegion` 属性轨道（Step）
    /// 加一条事件轨道。
    ///
    /// 这样"帧 → UV 区域"与"事件落在哪个时刻"各自只有一处定义：`AnimationStateMachine` 的采样与
    /// 触发都走它（见 `current_frame_uv` / `advance`）。离散目标的插值方式必须是 Step——对 UV 区域
    /// 插值没有意义，校验会拒绝。
    #[must_use]
    pub fn tracks(&self) -> crate::core::animation_track::AnimationTracks {
        use crate::core::animation_track::{
            AnimationTracks, EventMarker, EventTrack, Interpolation, Keyframe, PropertyTrack,
            Track, TrackTarget, TrackValue,
        };
        let cols = self.cols.max(1);
        let frame_rate = if self.frame_rate.is_finite() && self.frame_rate > 0.0 {
            self.frame_rate
        } else {
            // 解析期已经拒绝非正帧率；真走到这里按 1 FPS 处理而不是除以 0。
            1.0
        };
        let keyframes: Vec<Keyframe> = (0..cols)
            .map(|frame| {
                let start = frame as f32 / cols as f32;
                let end = (frame + 1) as f32 / cols as f32;
                Keyframe {
                    time: frame as f32 / frame_rate,
                    value: TrackValue::Region {
                        uv_min: Vec2::new(start, 0.0),
                        uv_max: Vec2::new(end, 1.0),
                    },
                }
            })
            .collect();
        let events: Vec<EventMarker> = self
            .events
            .iter()
            .enumerate()
            .map(|(order, event)| EventMarker {
                time: event.frame as f32 / frame_rate,
                name: event.name.clone(),
                order,
            })
            .collect();
        AnimationTracks::new(vec![
            Track::Property(PropertyTrack {
                target: TrackTarget::SpriteRegion,
                interpolation: Interpolation::Step,
                keyframes,
            }),
            Track::Event(EventTrack { events }),
        ])
    }

    /// 一段片段的总时长（秒）。
    #[must_use]
    pub fn duration(&self) -> f32 {
        let cols = self.cols.max(1) as f32;
        let frame_rate = if self.frame_rate.is_finite() && self.frame_rate > 0.0 {
            self.frame_rate
        } else {
            1.0
        };
        cols / frame_rate
    }

    /// docs/54 §4.4：这段片段在**运行时资源身份**里的键。
    ///
    /// 片段本身是场景数据（没有文件路径），它的身份由"内容签名"给出：贴图 + 帧数 + 帧率 + 是否循环 +
    /// 帧事件。同一段片段在进程内因此恒同键——"动画片段也有资源身份"由此成为可判定的说法
    /// （`RuntimeScene` 在推进动画时把它 intern 进与贴图/网格同一张身份表）。
    #[must_use]
    pub fn resource_key(&self) -> String {
        let events: Vec<String> = self
            .events
            .iter()
            .map(|event| format!("{}:{}", event.frame, event.name))
            .collect();
        format!(
            "animation:{}:{}:{}:{}:{}",
            self.sheet,
            self.cols,
            // 帧率用**位模式**：`1.0` 与 `1.00` 在十进制下同串，但只有逐位相同的片段才算同一身份。
            self.frame_rate.to_bits(),
            self.looped,
            events.join(",")
        )
    }
}

/// A set of named clips and the currently playing state.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationStateMachine {
    pub default: String,
    pub clips: Vec<AnimationClip>,
    current: String,
    time: f32,
    /// Set once when the current non-looping clip crosses its end boundary.
    ended: bool,
    /// Whether the current clip has already been marked complete, so the end
    /// event fires once per completion rather than on every frame.
    completed: bool,
    /// 已经触发过的帧事件在**当前这一轮**播放中的下标，避免同一帧每步重复触发。
    fired: Vec<usize>,
    /// 本步触发、等待宿主取走的帧事件。
    pending: Vec<AnimationFrameEvent>,
    /// docs/54 §4.4：当前片段的**轨道视图缓存**。轨道是 `(clip)` 的纯函数，而 `current_frame_uv`
    /// 与 `advance` 每次调用都要用它——每次现算就是每帧每实体一次 Vec 分配（批 H 引入过这个回归，
    /// 批 N 修掉）。缓存只在**当前片段变化**时重建。
    tracks: crate::core::animation_track::AnimationTracks,
    /// 当前片段在运行时资源身份表里的键（同样是 `(clip)` 的纯函数）。
    clip_key: String,
}

/// Errors produced while building or driving an animation state machine.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AnimationError {
    #[error("animation state machine has no clips")]
    NoClips,
    #[error("default state is not one of the clips")]
    MissingDefault,
    #[error("unknown animation state: {0}")]
    UnknownState(String),
    #[error("duplicate animation clip: {0}")]
    DuplicateClip(String),
    #[error("invalid animation component string: {0}")]
    Malformed(String),
}

impl AnimationStateMachine {
    /// Creates a state machine; `default` must reference one of `clips`.
    pub fn new(
        default: impl Into<String>,
        clips: Vec<AnimationClip>,
    ) -> Result<Self, AnimationError> {
        let default = default.into();
        if clips.is_empty() {
            return Err(AnimationError::NoClips);
        }
        let mut seen = std::collections::HashSet::new();
        for clip in &clips {
            if !seen.insert(clip.name.clone()) {
                return Err(AnimationError::DuplicateClip(clip.name.clone()));
            }
            if clip.cols == 0 || !clip.frame_rate.is_finite() || clip.frame_rate <= 0.0 {
                return Err(AnimationError::Malformed(format!(
                    "clip {:?} needs a positive column count and frame rate",
                    clip.name
                )));
            }
            for event in &clip.events {
                if event.frame >= clip.cols {
                    return Err(AnimationError::Malformed(format!(
                        "clip {:?} has an event on frame {} but only has {} frames",
                        clip.name, event.frame, clip.cols
                    )));
                }
                if event.name.is_empty() {
                    return Err(AnimationError::Malformed(format!(
                        "clip {:?} has a frame event with an empty name",
                        clip.name
                    )));
                }
            }
        }
        if !clips.iter().any(|clip| clip.name == default) {
            return Err(AnimationError::MissingDefault);
        }
        let current = default.clone();
        let clip = clips
            .iter()
            .find(|clip| clip.name == current)
            .expect("default state must reference a known clip");
        let tracks = clip.tracks();
        let clip_key = clip.resource_key();
        Ok(Self {
            default,
            clips,
            current,
            time: 0.0,
            ended: false,
            completed: false,
            fired: Vec::new(),
            pending: Vec::new(),
            tracks,
            clip_key,
        })
    }

    /// docs/54 §4.4：当前片段的轨道视图（**借用**缓存，不重新构建）。
    #[must_use]
    pub fn tracks(&self) -> &crate::core::animation_track::AnimationTracks {
        &self.tracks
    }

    /// docs/54 §4.4：当前片段在运行时资源身份表里的键（`sheet` + 帧数 + 帧率 + 循环 + 事件）。
    #[must_use]
    pub fn clip_resource_key(&self) -> &str {
        &self.clip_key
    }

    /// 重建轨道缓存与身份键。**任何**改动当前片段的地方都要调它（状态切换、重播）。
    fn refresh_clip_caches(&mut self) {
        let (tracks, clip_key) = {
            let clip = self.current_clip();
            (clip.tracks(), clip.resource_key())
        };
        self.tracks = tracks;
        self.clip_key = clip_key;
    }

    /// Returns the name of the currently playing state.
    #[must_use]
    pub fn current_state(&self) -> &str {
        &self.current
    }

    /// Returns the clip of the currently playing state.
    #[must_use]
    pub fn current_clip(&self) -> &AnimationClip {
        self.clips
            .iter()
            .find(|clip| clip.name == self.current)
            .expect("current state must reference a known clip")
    }

    /// Switches to `name`, restarting its clip. Returns `true` when a
    /// transition actually happened (the requested state differs).
    pub fn set_state(&mut self, name: &str) -> Result<bool, AnimationError> {
        if name == self.current {
            return Ok(false);
        }
        if !self.clips.iter().any(|clip| clip.name == name) {
            return Err(AnimationError::UnknownState(name.to_owned()));
        }
        self.current = name.to_owned();
        // docs/54 §4.4：轨道视图与身份键都是 `(当前片段)` 的纯函数，换片段必须重建缓存。
        self.refresh_clip_caches();
        self.time = 0.0;
        self.ended = false;
        self.completed = false;
        // 换状态 = 重新播一轮：已触发记录清空，因此新状态在第 0 帧的事件也能触发。
        self.fired.clear();
        Ok(true)
    }

    /// Advances the current clip by `dt` seconds. Non-looping clips clamp at
    /// their last frame and raise the once-only end event.
    ///
    /// 同时触发落在本步覆盖帧范围内的**帧事件**（WP4 ⑦）。判定方式是比较"本步开始时的帧"
    /// 与"本步结束时的帧"，因此与固定步长无关：
    /// - 单步跨过多帧时，中间的帧事件**全部**按顺序触发（不会因一次 dt 较大而漏掉命中帧）；
    /// - 循环片段回绕时，`fired` 记录清空，下一圈同帧事件再次触发；
    /// - 同一步内重复 `advance` 到同一帧不会重复触发（`fired` 记的是"本轮已触发过"）。
    pub fn advance(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        let (looped, duration, cols, frame_rate) = {
            let clip = self.current_clip();
            (clip.looped, clip.duration(), clip.cols, clip.frame_rate)
        };
        let from_frame = self.current_frame();
        let from_cycle = (self.time * frame_rate).floor() as u64 / u64::from(cols.max(1));
        self.time += delta_seconds;
        if !looped {
            if !self.completed && self.time >= duration {
                self.completed = true;
                self.ended = true;
            }
            self.time = self.time.min(duration);
        }
        let to_cycle = (self.time * frame_rate).floor() as u64 / u64::from(cols.max(1));
        if to_cycle != from_cycle {
            // 回绕（可能一次跨多圈）：新一轮从头算，本圈尚未触发过任何事件。
            self.fired.clear();
        }
        // docs/54 §4.4：帧事件的区间判定也走统一轨道——本条实现只负责"量化粒度"（起始帧对应的
        // 时刻）与"本圈是否已触发过"（`fired` 按源数据序号去重），区间语义在 `EventTrack` 里。
        //
        // 起始下界刻意用**起始帧的时刻**（`from_frame / frame_rate`）而不是 `self.time` 之前的
        // 值：帧事件挂在帧上，一步跨多帧时中间的帧事件必须全部触发（滑步/漏掉命中帧都属于真实
        // 缺陷）。循环回绕时按"两段都到达过"处理。
        let from_time = from_frame as f32 / frame_rate.max(f32::EPSILON);
        // 没有帧事件时整段跳过：绝大多数片段都没有事件，省掉每帧一次 Vec 分配。
        let due = if self.current_clip().events.is_empty() {
            Vec::new()
        } else {
            self.tracks
                .events_in_interval(from_time, self.time, looped, duration)
        };
        for marker in due {
            if self.fired.contains(&marker.order) {
                continue;
            }
            self.fired.push(marker.order);
            // 帧号由事件时刻反推（事件就挂在帧上），因此 Lua/宿主看到的仍是帧号。
            let frame = (marker.time * frame_rate).round().max(0.0) as u32;
            self.pending.push(AnimationFrameEvent {
                frame: frame.min(cols.saturating_sub(1)),
                name: marker.name,
            });
        }
    }

    /// 取走本步触发的帧事件（取走即清空；宿主每帧调用一次交给 Lua）。
    pub fn take_frame_events(&mut self) -> Vec<AnimationFrameEvent> {
        std::mem::take(&mut self.pending)
    }

    /// Returns the current 0-based frame index for the playing clip.
    #[must_use]
    pub fn current_frame(&self) -> u32 {
        let clip = self.current_clip();
        let raw = (self.time * clip.frame_rate).floor() as u32;
        if clip.looped {
            raw % clip.cols
        } else {
            raw.min(clip.cols - 1)
        }
    }

    /// 当前片段的播放时刻（秒）。与 `Animation3D::time` 同名同义，供宿主/测试对拍轨道采样。
    #[must_use]
    pub const fn time(&self) -> f32 {
        self.time
    }

    /// Returns the normalized UV rectangle of the current frame.
    ///
    /// docs/54 §4.4：帧 → UV 这条规则现在只有一处定义——`AnimationClip::tracks()` 的
    /// `SpriteRegion` 轨道。这里采样它，因此"轨道视图"与"渲染真正用的 UV"不可能分家。
    #[must_use]
    pub fn current_frame_uv(&self) -> (Vec2, Vec2) {
        let time = self.time;
        // 采样**缓存**的轨道（不再每次现算）：帧 → UV 这条规则仍然只有一处定义。
        if let Some(crate::core::animation_track::TrackValue::Region { uv_min, uv_max }) = self
            .tracks
            .sample(
                crate::core::animation_track::TrackTarget::SpriteRegion,
                time,
            )
            .as_deref()
        {
            return (*uv_min, *uv_max);
        }
        // 轨道为空（`cols == 0` 之类的坏数据）：退回整张贴图，与"没有区域信息"一致。
        (Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0))
    }

    /// Returns whether the current non-looping clip completed, firing once per
    /// completion. Reads and clears the flag.
    #[must_use]
    pub fn take_ended(&mut self) -> bool {
        let ended = self.ended;
        self.ended = false;
        ended
    }

    /// Serializes the machine into a scene `Animation` component string:
    /// `default=<name>;clip:<name>:<sheet>:<cols>:<fps>:<looped>` per state。
    ///
    /// 有帧事件时追加第六段 `events=<帧>:<名>[,<帧>:<名>]`（WP4 ⑦）。**没有事件就不写这一
    /// 段**，因此既有场景的规范字符串一字不变。
    pub fn to_component_str(&self) -> String {
        let mut out = format!("default={}", self.default);
        for clip in &self.clips {
            out.push_str(&format!(
                ";clip:{}:{}:{}:{}:{}",
                clip.name, clip.sheet, clip.cols, clip.frame_rate, clip.looped
            ));
            if !clip.events.is_empty() {
                let events = clip
                    .events
                    .iter()
                    .map(|event| format!("{}:{}", event.frame, event.name))
                    .collect::<Vec<_>>()
                    .join(",");
                out.push_str(&format!(":events={events}"));
            }
        }
        out
    }

    /// Parses a scene `Animation` component string back into a state machine.
    pub fn from_component_str(value: &str) -> Result<Self, AnimationError> {
        let mut default = None;
        let mut clips = Vec::new();
        for field in value.split(';') {
            if let Some(name) = field.strip_prefix("default=") {
                default = Some(name.to_owned());
                continue;
            }
            let Some(rest) = field.strip_prefix("clip:") else {
                return Err(AnimationError::Malformed(format!("unknown field: {field}")));
            };
            // `events=` 段内部还有冒号（`2:step`），因此只切前 5 段，第 6 段留整段。
            let parts: Vec<&str> = rest.splitn(6, ':').collect();
            if parts.len() != 5 && parts.len() != 6 {
                return Err(AnimationError::Malformed(format!(
                    "clip needs name:sheet:cols:fps:looped[:events=..], got {rest}"
                )));
            }
            let cols: u32 = parts[2]
                .parse()
                .map_err(|_| AnimationError::Malformed("clip cols not an integer".to_owned()))?;
            let frame_rate: f32 = parts[3]
                .parse()
                .map_err(|_| AnimationError::Malformed("clip fps not a number".to_owned()))?;
            let looped: bool = match parts[4] {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(AnimationError::Malformed(
                        "clip looped must be true/false".to_owned(),
                    ));
                }
            };
            let events = match parts.get(5) {
                None => Vec::new(),
                Some(raw) => parse_frame_events(raw)?,
            };
            clips.push(AnimationClip {
                name: parts[0].to_owned(),
                sheet: parts[1].to_owned(),
                cols,
                frame_rate,
                looped,
                events,
            });
        }
        let default =
            default.ok_or_else(|| AnimationError::Malformed("missing default".to_owned()))?;
        Self::new(default, clips)
    }
}

/// 解析 `events=<帧>:<名>[,<帧>:<名>]`。格式错误一律报错：帧号写错会让"命中判定帧"
/// 静默消失，比直接报错难查得多。
fn parse_frame_events(raw: &str) -> Result<Vec<AnimationEvent>, AnimationError> {
    let Some(list) = raw.strip_prefix("events=") else {
        return Err(AnimationError::Malformed(format!(
            "expected events=.., got {raw}"
        )));
    };
    if list.is_empty() {
        return Err(AnimationError::Malformed(
            "events= needs at least one entry".to_owned(),
        ));
    }
    let mut events = Vec::new();
    for entry in list.split(',') {
        let (frame, name) = entry.split_once(':').ok_or_else(|| {
            AnimationError::Malformed(format!("event needs frame:name, got {entry}"))
        })?;
        let frame: u32 = frame.parse().map_err(|_| {
            AnimationError::Malformed(format!("event frame not an integer: {frame}"))
        })?;
        if name.is_empty() {
            return Err(AnimationError::Malformed(format!(
                "event on frame {frame} has an empty name"
            )));
        }
        events.push(AnimationEvent {
            frame,
            name: name.to_owned(),
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/54 §4.4：**轨道视图与真正渲染用的值必须一致**。
    ///
    /// `current_frame_uv` 现在采样 `SpriteRegion` 轨道，因此这条测试是对"单一规则"的回归保护：
    /// 时间扫描（含帧边界、循环回绕）逐点比对轨道采样与机器自己算出的帧，两者任何一处改口径都会
    /// 在这里失败。
    #[test]
    fn clip_tracks_agree_with_the_state_machine() {
        let mut machine = machine();
        let clip = machine.current_clip().clone();
        let tracks = clip.tracks();
        let track = tracks
            .property(crate::core::animation_track::TrackTarget::SpriteRegion)
            .expect("region track");
        assert_eq!(track.keyframes.len(), clip.cols as usize);
        assert_eq!(
            track.interpolation,
            crate::core::animation_track::Interpolation::Step,
            "帧动画是离散目标"
        );
        assert!(
            tracks.validate().is_ok(),
            "由片段生成的轨道必须自洽：{:?}",
            tracks.validate()
        );

        // 时间扫描：按固定步推进（步长刻意取 1/32 秒，比帧长短，因此帧边界两侧都被覆盖到，
        // 也会跨过两圈回绕），每步比对轨道采样与渲染真正用的 UV。
        let mut steps = 0;
        while machine.time() < 2.5 {
            machine.advance(1.0 / 32.0);
            steps += 1;
            assert!(steps < 1000, "advance 没有推进时间（测试自身失效）");
            let time = machine.time();
            let (uv_min, uv_max) = machine.current_frame_uv();
            match tracks
                .sample(
                    crate::core::animation_track::TrackTarget::SpriteRegion,
                    time,
                )
                .expect("sample")
                .as_ref()
            {
                crate::core::animation_track::TrackValue::Region {
                    uv_min: track_min,
                    uv_max: track_max,
                } => {
                    assert_eq!(
                        (uv_min.x, uv_min.y, uv_max.x, uv_max.y),
                        (track_min.x, track_min.y, track_max.x, track_max.y),
                        "t={time} 时轨道采样与渲染用的 UV 必须一致"
                    );
                }
                other => panic!("expected region, got {other:?}"),
            }
        }
        // 片段是 4 帧 @4fps = 1 秒一圈；1/32 秒步长 ⇒ 每圈 32 步，2.5 秒因此跨过两圈以上。
        assert!(steps >= 64, "扫描至少要跨两圈循环（实际 {steps} 步）");
    }

    /// docs/54 §4.4：轨道视图**按片段缓存**——`advance` 与 `current_frame_uv` 都不再每帧重建它。
    ///
    /// 回归背景：批 H 把"帧 → UV"与帧事件都改成走轨道视图，但每次调用都现算一次
    /// （`cols` 个关键帧 + 事件的 Vec）。对一群动画实体就是每帧每实体一次分配，而 WP0 的
    /// 基准场景恰好都是静态精灵，因此性能门没抓到。这条用例用**数据指针**证明缓存真的在复用。
    #[test]
    fn clip_track_cache_is_reused_across_frames() {
        let mut machine = machine();
        let keyframes_ptr = |machine: &AnimationStateMachine| {
            machine
                .tracks()
                .property(crate::core::animation_track::TrackTarget::SpriteRegion)
                .expect("region track")
                .keyframes
                .as_ptr()
        };
        let first = keyframes_ptr(&machine);
        for _ in 0..10 {
            machine.advance(1.0 / 60.0);
        }
        assert_eq!(
            keyframes_ptr(&machine),
            first,
            "advance 不该重建轨道（缓存必须复用）"
        );
        // 换状态 ⇒ 片段变了 ⇒ 缓存必须重建，且身份键跟着变。
        let before_key = machine.clip_resource_key().to_owned();
        machine.set_state("attack").expect("switch to attack");
        assert_ne!(
            keyframes_ptr(&machine),
            first,
            "换片段后轨道缓存必须重建（否则采样的是上一段的帧）"
        );
        assert_ne!(machine.clip_resource_key(), before_key);
        // 同一状态再切回去：键回到原值（内容寻址，不含时间/瞬时状态）。
        machine.set_state("idle").expect("switch back");
        assert_eq!(machine.clip_resource_key(), before_key);
    }

    /// 片段身份是**内容寻址**的：内容相同 ⇒ 同键；帧数/帧率/循环/事件任一不同 ⇒ 不同键。
    #[test]
    fn clip_resource_key_is_content_addressed() {
        let base = AnimationClip {
            name: "idle".into(),
            sheet: "assets/idle.png".into(),
            cols: 4,
            frame_rate: 8.0,
            looped: true,
            events: Vec::new(),
        };
        let mut same = base.clone();
        same.name = "renamed".into();
        assert_eq!(
            base.resource_key(),
            same.resource_key(),
            "名字是本地标签，不参与身份"
        );

        for mutate in [
            |clip: &mut AnimationClip| clip.cols = 5,
            |clip: &mut AnimationClip| clip.frame_rate = 12.0,
            |clip: &mut AnimationClip| clip.looped = false,
            |clip: &mut AnimationClip| clip.sheet = "assets/other.png".into(),
            |clip: &mut AnimationClip| {
                clip.events.push(AnimationEvent {
                    frame: 1,
                    name: "hit".into(),
                })
            },
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(
                base.resource_key(),
                changed.resource_key(),
                "内容不同的片段不能共用身份"
            );
        }
    }
    /// 事件轨道承载的就是片段里的帧事件（顺序与名称一致），因此"第几帧触发"只有一处定义。
    #[test]
    fn clip_tracks_carry_the_frame_events_in_order() {
        let machine = machine_with_events();
        let clip = machine.current_clip();
        let tracks = clip.tracks();
        let sampled: Vec<(f32, String, usize)> = tracks
            .events_in_interval(0.0, clip.duration(), clip.looped, clip.duration())
            .into_iter()
            .map(|event| (event.time, event.name, event.order))
            .collect();
        let expected: Vec<(f32, String, usize)> = clip
            .events
            .iter()
            .enumerate()
            .map(|(order, event)| {
                (
                    event.frame as f32 / clip.frame_rate,
                    event.name.clone(),
                    order,
                )
            })
            .collect();
        assert_eq!(sampled, expected);
    }

    fn machine() -> AnimationStateMachine {
        let clips = vec![
            AnimationClip {
                name: "idle".into(),
                sheet: "assets/idle-sheet.png".into(),
                cols: 4,
                frame_rate: 4.0,
                looped: true,
                events: Vec::new(),
            },
            AnimationClip {
                name: "attack".into(),
                sheet: "assets/attack-sheet.png".into(),
                cols: 2,
                frame_rate: 2.0,
                looped: false,
                events: Vec::new(),
            },
        ];
        AnimationStateMachine::new("idle", clips).unwrap()
    }

    /// 带帧事件的机器：循环的 `idle` 在第 2 帧、非循环的 `attack` 在第 1 帧各有一个事件。
    fn machine_with_events() -> AnimationStateMachine {
        let event = |frame: u32, name: &str| AnimationEvent {
            frame,
            name: name.to_owned(),
        };
        let clips = vec![
            AnimationClip {
                name: "idle".into(),
                sheet: "assets/idle-sheet.png".into(),
                cols: 4,
                frame_rate: 4.0,
                looped: true,
                events: vec![event(2, "step")],
            },
            AnimationClip {
                name: "attack".into(),
                sheet: "assets/attack-sheet.png".into(),
                cols: 2,
                frame_rate: 2.0,
                looped: false,
                events: vec![event(0, "swing"), event(1, "hit")],
            },
        ];
        AnimationStateMachine::new("idle", clips).unwrap()
    }

    fn names(events: Vec<AnimationFrameEvent>) -> Vec<String> {
        events.into_iter().map(|event| event.name).collect()
    }

    #[test]
    fn frame_events_fire_once_when_the_frame_is_reached() {
        let mut m = machine_with_events();
        assert!(names(m.take_frame_events()).is_empty(), "初始不算到达");
        m.advance(0.25); // idle -> frame 1
        assert!(names(m.take_frame_events()).is_empty());
        m.advance(0.25); // idle -> frame 2（事件帧）
        assert_eq!(names(m.take_frame_events()), vec!["step"]);
        // 同一帧上继续推进不再重复触发。
        m.advance(0.0);
        assert!(names(m.take_frame_events()).is_empty());
        // 循环片段推进整整一圈会**再次经过**帧 2（本步的终点正好回到帧 2），因此这里
        // 必须再触发一次，而且只触发一次（不是"起点一次、终点一次"）。
        m.advance(1.0);
        assert_eq!(names(m.take_frame_events()), vec!["step"]);
    }

    #[test]
    fn a_single_large_step_fires_every_frame_it_crosses() {
        // 固定步长可调时（掉帧追帧）一次 dt 可能跨过整个片段：中间的命中帧不能漏。
        let mut m = machine_with_events();
        assert!(m.set_state("attack").unwrap());
        m.advance(5.0); // 直接跨完 2 帧的片段
        assert_eq!(names(m.take_frame_events()), vec!["swing", "hit"]);
        assert!(names(m.take_frame_events()).is_empty(), "取走即清空");
    }

    #[test]
    fn looped_clips_refire_events_on_each_wrap() {
        let mut m = machine_with_events();
        m.advance(0.5); // 跨过帧 2 -> 第一次
        assert_eq!(names(m.take_frame_events()), vec!["step"]);
        m.advance(0.25); // 帧 3
        assert!(names(m.take_frame_events()).is_empty());
        m.advance(0.25); // 回绕到帧 0（本轮未经过帧 2）
        assert!(
            names(m.take_frame_events()).is_empty(),
            "回绕跳过的帧不算到达"
        );
        m.advance(0.25);
        m.advance(0.25); // 帧 1 -> 帧 2：新的一圈，再次触发
        assert_eq!(names(m.take_frame_events()), vec!["step"]);
    }

    #[test]
    fn switching_state_restarts_event_tracking() {
        let mut m = machine_with_events();
        assert!(m.set_state("attack").unwrap());
        // 第 0 帧的事件在**切换后**的第一次 advance 就要触发（不是从旧状态的已触发集合里
        // 被继承掉）。
        m.advance(0.0);
        assert_eq!(names(m.take_frame_events()), vec!["swing"]);
        m.advance(0.5);
        assert_eq!(names(m.take_frame_events()), vec!["hit"]);
    }

    #[test]
    fn frame_events_round_trip_and_reject_bad_input() {
        let m = machine_with_events();
        let encoded = m.to_component_str();
        assert!(encoded.contains(":events=2:step"), "{encoded}");
        assert!(encoded.contains(":events=0:swing,1:hit"), "{encoded}");
        let parsed = AnimationStateMachine::from_component_str(&encoded).unwrap();
        assert_eq!(parsed.clips, m.clips, "往返后事件必须逐条相同");
        assert_eq!(parsed.to_component_str(), encoded);

        // 既有无事件的片段规范字符串一字不变（不写空的 events 段）。
        assert_eq!(
            machine().to_component_str(),
            "default=idle;clip:idle:assets/idle-sheet.png:4:4:true;\
             clip:attack:assets/attack-sheet.png:2:2:false"
        );
        // 越界帧、空名、坏格式都必须报错，不能静默丢弃。
        let over = "default=a;clip:a:s.png:2:2:true:events=5:hit";
        assert!(AnimationStateMachine::from_component_str(over).is_err());
        let empty_name = "default=a;clip:a:s.png:2:2:true:events=1:";
        assert!(AnimationStateMachine::from_component_str(empty_name).is_err());
        let bad = "default=a;clip:a:s.png:2:2:true:events=1";
        assert!(AnimationStateMachine::from_component_str(bad).is_err());
        let bad_frame = "default=a;clip:a:s.png:2:2:true:events=x:hit";
        assert!(AnimationStateMachine::from_component_str(bad_frame).is_err());
    }

    #[test]
    fn advances_looped_clip_frames_and_wraps() {
        let mut m = machine();
        assert_eq!(m.current_state(), "idle");
        assert_eq!(m.current_frame(), 0);
        m.advance(0.25); // 4 fps -> one frame every 0.25s
        assert_eq!(m.current_frame(), 1);
        m.advance(0.25);
        assert_eq!(m.current_frame(), 2);
        m.advance(2.0); // time = 2.5s, 10 frames wrapped by 4 -> frame 2
        assert_eq!(m.current_frame(), 2);
    }

    #[test]
    fn non_looping_clip_clamps_and_fires_end_event_once() {
        let mut m = machine();
        assert!(m.set_state("attack").unwrap());
        m.advance(0.5); // 2 fps -> frame 1 at 0.5s, attack not done yet (duration 1.0s)
        assert_eq!(m.current_frame(), 1);
        assert!(!m.take_ended());
        m.advance(0.6); // past end -> frame 1 clamped, ended
        assert_eq!(m.current_frame(), 1);
        assert!(m.take_ended());
        assert!(!m.take_ended());
        // the end event fires once per completion; further advance does not re-fire
        m.advance(1.0);
        assert!(!m.take_ended());
    }

    #[test]
    fn frame_uv_selects_sheet_columns() {
        let mut m = machine();
        m.advance(0.25);
        let (min, max) = m.current_frame_uv();
        assert_eq!(min.x, 0.25);
        assert_eq!(max.x, 0.5);
        assert_eq!(min.y, 0.0);
        assert_eq!(max.y, 1.0);
    }

    #[test]
    fn rejects_unknown_state_and_bad_construction() {
        let mut m = machine();
        assert!(matches!(
            m.set_state("missing"),
            Err(AnimationError::UnknownState(_))
        ));
        assert!(AnimationStateMachine::new("missing", vec![]).is_err());
        assert!(AnimationStateMachine::new("missing", m.clips.clone()).is_err());
    }

    #[test]
    fn round_trips_through_component_string() {
        let m = machine();
        let text = m.to_component_str();
        assert_eq!(AnimationStateMachine::from_component_str(&text).unwrap(), m);
    }

    #[test]
    fn state_transition_restarts_clip() {
        let mut m = machine();
        m.advance(1.0); // idle in frame 1
        assert!(!m.set_state("idle").unwrap());
        assert!(m.set_state("attack").unwrap());
        assert_eq!(m.current_state(), "attack");
        assert_eq!(m.current_frame(), 0); // restarted
    }
}
