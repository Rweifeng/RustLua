//! Per-entity animation state machines: named states, each with a sprite-sheet
//! clip (1xN frame strip), frame-rate, and loop policy. Lua reads and switches
//! states; the renderer samples the current frame's UV region.

use std::{error::Error, fmt};

use crate::transform::Vec2;

/// One state's animation: a `1 x cols` frame strip sampled at `frame_rate` FPS.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationClip {
    pub name: String,
    pub sheet: String,
    pub cols: u32,
    pub frame_rate: f32,
    pub looped: bool,
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
        Ok(true)
    }

    /// Advances the current clip by `dt` seconds. Non-looping clips clamp at
    /// their last frame and raise the once-only end event.
    pub fn advance(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds < 0.0 {
            return;
        }
        let (looped, duration) = {
            let clip = self.current_clip();
            (clip.looped, clip.cols as f32 / clip.frame_rate)
        };
        self.time += delta_seconds;
        if !looped {
            if !self.completed && self.time >= duration {
                self.completed = true;
                self.ended = true;
            }
            self.time = self.time.min(duration);
        }
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
    /// `default=<name>;clip:<name>:<sheet>:<cols>:<fps>:<looped>` per state.
    pub fn to_component_str(&self) -> String {
        let mut out = format!("default={}", self.default);
        for clip in &self.clips {
            out.push_str(&format!(
                ";clip:{}:{}:{}:{}:{}",
                clip.name, clip.sheet, clip.cols, clip.frame_rate, clip.looped
            ));
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
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() != 5 {
                return Err(AnimationError::Malformed(format!(
                    "clip needs name:sheet:cols:fps:looped, got {rest}"
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
            clips.push(AnimationClip {
                name: parts[0].to_owned(),
                sheet: parts[1].to_owned(),
                cols,
                frame_rate,
                looped,
            });
        }
        let default =
            default.ok_or_else(|| AnimationError::Malformed("missing default".to_owned()))?;
        Self::new(default, clips)
    }
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
            },
            AnimationClip {
                name: "attack".into(),
                sheet: "assets/attack-sheet.png".into(),
                cols: 2,
                frame_rate: 2.0,
                looped: false,
            },
        ];
        AnimationStateMachine::new("idle", clips).unwrap()
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
