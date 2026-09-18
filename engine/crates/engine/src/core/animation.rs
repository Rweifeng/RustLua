//! Per-entity animation state machines: named states, each with a sprite-sheet
//! clip (1xN frame strip), frame-rate, and loop policy. Lua reads and switches
//! states; the renderer samples the current frame's UV region.

use std::{error::Error, fmt};

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
}

/// Errors produced while building or driving an animation state machine.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationError {
    NoClips,
    MissingDefault,
    UnknownState(String),
    DuplicateClip(String),
    Malformed(String),
}

impl fmt::Display for AnimationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoClips => write!(f, "animation state machine has no clips"),
            Self::MissingDefault => write!(f, "default state is not one of the clips"),
            Self::UnknownState(name) => write!(f, "unknown animation state: {name}"),
            Self::DuplicateClip(name) => write!(f, "duplicate animation clip: {name}"),
            Self::Malformed(reason) => write!(f, "invalid animation component string: {reason}"),
        }
    }
}

impl Error for AnimationError {}

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
        Ok(Self {
            default,
            clips,
            current,
            time: 0.0,
            ended: false,
            completed: false,
            fired: Vec::new(),
            pending: Vec::new(),
        })
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
            (
                clip.looped,
                clip.cols as f32 / clip.frame_rate,
                clip.cols,
                clip.frame_rate,
            )
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
        let to_frame = self.current_frame();
        let to_cycle = (self.time * frame_rate).floor() as u64 / u64::from(cols.max(1));
        if to_cycle != from_cycle {
            // 回绕（可能一次跨多圈）：新一轮从头算，本圈尚未触发过任何事件。
            self.fired.clear();
        }
        // 本步覆盖的帧区间：不循环时夹在 [from, to]（含），循环时可能回绕。
        let events: Vec<(usize, AnimationEvent)> = self
            .current_clip()
            .events
            .iter()
            .cloned()
            .enumerate()
            .collect();
        for (index, event) in events {
            if self.fired.contains(&index) {
                continue;
            }
            let reached = if to_cycle != from_cycle {
                // 回绕：末尾到 cols-1 与开头到 to_frame 都算到达过。
                event.frame >= from_frame || event.frame <= to_frame
            } else {
                event.frame >= from_frame && event.frame <= to_frame
            };
            if reached {
                self.fired.push(index);
                self.pending.push(AnimationFrameEvent {
                    frame: event.frame,
                    name: event.name,
                });
            }
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

    /// Returns the normalized UV rectangle of the current frame.
    #[must_use]
    pub fn current_frame_uv(&self) -> (Vec2, Vec2) {
        let clip = self.current_clip();
        let cols = clip.cols.max(1) as f32;
        let frame = self.current_frame() as f32;
        (
            Vec2::new(frame / cols, 0.0),
            Vec2::new((frame + 1.0) / cols, 1.0),
        )
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
