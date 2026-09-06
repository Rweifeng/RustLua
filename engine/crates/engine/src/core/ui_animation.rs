//! E26: pure UI animation tracks with an isolated preview clock.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiEasing {
    Linear,
    Step,
    SmoothStep,
}

impl UiEasing {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "linear" => Some(Self::Linear),
            "step" => Some(Self::Step),
            "smoothstep" => Some(Self::SmoothStep),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Step => "step",
            Self::SmoothStep => "smoothstep",
        }
    }

    #[must_use]
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Step => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::SmoothStep => t * t * (3.0 - 2.0 * t),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiKeyframe {
    pub time: f32,
    pub value: f32,
    pub easing: UiEasing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiAnimationTrack {
    pub property: String,
    pub keyframes: Vec<UiKeyframe>,
}

impl UiAnimationTrack {
    pub fn new(property: impl Into<String>, keyframes: Vec<UiKeyframe>) -> Result<Self, String> {
        let property = property.into();
        if property.trim().is_empty() {
            return Err("track property cannot be empty".to_owned());
        }
        if keyframes.is_empty() {
            return Err("track needs at least one keyframe".to_owned());
        }
        if keyframes.windows(2).any(|pair| {
            !pair[0].time.is_finite()
                || !pair[1].time.is_finite()
                || pair[1].time <= pair[0].time
                || !pair[0].value.is_finite()
                || !pair[1].value.is_finite()
        }) {
            return Err("keyframe times must be finite and strictly increasing".to_owned());
        }
        Ok(Self {
            property,
            keyframes,
        })
    }

    #[must_use]
    pub fn sample(&self, time: f32) -> f32 {
        let first = self.keyframes[0];
        if time <= first.time {
            return first.value;
        }
        let last = *self.keyframes.last().unwrap();
        if time >= last.time {
            return last.value;
        }
        let next_index = self
            .keyframes
            .iter()
            .position(|keyframe| keyframe.time > time)
            .unwrap_or(self.keyframes.len() - 1);
        let from = self.keyframes[next_index - 1];
        let to = self.keyframes[next_index];
        let t = (time - from.time) / (to.time - from.time);
        let eased = to.easing.apply(t);
        from.value + (to.value - from.value) * eased
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiAnimationClip {
    pub name: String,
    pub tracks: BTreeMap<String, UiAnimationTrack>,
}

impl UiAnimationClip {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tracks: BTreeMap::new(),
        }
    }

    pub fn add_track(&mut self, track: UiAnimationTrack) -> Result<(), String> {
        self.tracks.insert(track.property.clone(), track);
        Ok(())
    }

    #[must_use]
    pub fn duration(&self) -> f32 {
        self.tracks
            .values()
            .filter_map(|track| track.keyframes.last().map(|keyframe| keyframe.time))
            .fold(0.0_f32, f32::max)
    }
}

/// Preview-only playback state. It never writes to a `SceneDocument`.
#[derive(Clone, Debug, PartialEq)]
pub struct UiAnimationPreview {
    pub clip: UiAnimationClip,
    pub time: f32,
    pub playing: bool,
    pub looping: bool,
}

impl UiAnimationPreview {
    #[must_use]
    pub fn new(clip: UiAnimationClip, looping: bool) -> Self {
        Self {
            clip,
            time: 0.0,
            playing: true,
            looping,
        }
    }

    pub fn advance(&mut self, delta: f32) {
        if !self.playing || !delta.is_finite() {
            return;
        }
        self.time += delta.max(0.0);
        let duration = self.clip.duration();
        if duration > 0.0 && self.time >= duration {
            if self.looping {
                self.time %= duration;
            } else {
                self.time = duration;
                self.playing = false;
            }
        }
    }

    #[must_use]
    pub fn sample(&self, property: &str) -> Option<f32> {
        self.clip
            .tracks
            .get(property)
            .map(|track| track.sample(self.time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> UiAnimationTrack {
        UiAnimationTrack::new(
            "opacity",
            vec![
                UiKeyframe {
                    time: 0.0,
                    value: 0.0,
                    easing: UiEasing::SmoothStep,
                },
                UiKeyframe {
                    time: 1.0,
                    value: 1.0,
                    easing: UiEasing::SmoothStep,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn samples_track_and_rejects_unordered_keyframes() {
        assert_eq!(track().sample(0.0), 0.0);
        assert_eq!(track().sample(1.0), 1.0);
        assert!(track().sample(0.5) > 0.0 && track().sample(0.5) < 1.0);
        assert!(
            UiAnimationTrack::new(
                "bad",
                vec![
                    UiKeyframe {
                        time: 1.0,
                        value: 0.0,
                        easing: UiEasing::Linear
                    },
                    UiKeyframe {
                        time: 1.0,
                        value: 1.0,
                        easing: UiEasing::Linear
                    },
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn preview_loop_and_stop_are_isolated_from_scene_data() {
        let mut clip = UiAnimationClip::new("fade");
        clip.add_track(track()).unwrap();
        let mut preview = UiAnimationPreview::new(clip.clone(), true);
        preview.advance(1.25);
        assert!((preview.time - 0.25).abs() < 0.0001);
        assert!(preview.playing);
        preview.looping = false;
        preview.time = 0.9;
        preview.advance(0.2);
        assert!(!preview.playing);
        assert_eq!(preview.sample("opacity"), Some(1.0));
        assert_eq!(preview.clip, clip);
    }
}
