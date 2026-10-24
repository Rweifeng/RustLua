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

    /// `UiAnimation` 组件串 → 轨道表：**这份格式的唯一事实源**（第 40 轮从编辑器搬进引擎）。
    ///
    /// 串形如 `track.<属性>=<时间>:<值>:<缓动>[,<时间>:<值>:<缓动>…]`，
    /// 多条轨道用 `|` 连接（与 `UiEvents`/`UiBindings` 的多记录分隔符一致）。
    /// **名字不在串里**：`UiAnimationClip::name` 是编辑器预览的概念，不属于组件契约。
    ///
    /// 为什么把它搬进引擎：迁进之前这份编码/解码只有编辑器实现
    /// （`ui_editor::parse_ui_animation_component` / `apply_ui_animation_editor`），
    /// 于是引擎与审计都无法校验 `UiAnimation`——注册 schema 就等于凭空造一份事实源。
    /// 现在引擎有了解析器，schema 的 `ListModel`（动态键 `track.`）才有对拍对象。
    pub fn tracks_from_component_str(
        raw: &str,
    ) -> Result<BTreeMap<String, UiAnimationTrack>, String> {
        let mut tracks = BTreeMap::new();
        for field in raw.split('|') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("malformed animation field: {field}"));
            };
            let Some(property) = key.strip_prefix("track.") else {
                return Err(format!("unknown animation field: {key}"));
            };
            let mut keyframes = Vec::new();
            for item in value.split(',') {
                let parts = item.split(':').collect::<Vec<_>>();
                if parts.len() != 3 {
                    return Err(format!("malformed keyframe: {item}"));
                }
                keyframes.push(UiKeyframe {
                    time: parts[0]
                        .trim()
                        .parse()
                        .map_err(|_| format!("bad time: {item}"))?,
                    value: parts[1]
                        .trim()
                        .parse()
                        .map_err(|_| format!("bad value: {item}"))?,
                    easing: UiEasing::parse(parts[2])
                        .ok_or_else(|| format!("bad easing: {item}"))?,
                });
            }
            let track = UiAnimationTrack::new(property, keyframes)?;
            tracks.insert(track.property.clone(), track);
        }
        Ok(tracks)
    }

    /// 轨道表 → 组件串（[`Self::tracks_from_component_str`] 的逆运算）。
    ///
    /// 数字按编辑器一直以来的写法定长 3 位小数（`{:.3}`）：这是**磁盘上既有数据的形态**
    /// （`examples/` 与编辑器预览都写这个格式），改精度会让所有旧串在"读→写"后变形。
    #[must_use]
    pub fn tracks_to_component_str(tracks: &BTreeMap<String, UiAnimationTrack>) -> String {
        tracks
            .values()
            .map(|track| {
                let keyframes = track
                    .keyframes
                    .iter()
                    .map(|keyframe| {
                        format!(
                            "{:.3}:{:.3}:{}",
                            keyframe.time,
                            keyframe.value,
                            keyframe.easing.as_str()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("track.{}={keyframes}", track.property)
            })
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// Runtime driver for a UI clip. Uses the same sampling/looping rules as
/// [`UiAnimationPreview`], but reports changed track values and can be reset
/// when a state transition interrupts an entry/exit animation.
#[derive(Clone, Debug, PartialEq)]
pub struct UiAnimationDriver {
    clip: UiAnimationClip,
    time: f32,
    playing: bool,
    looping: bool,
    speed: f32,
    values: BTreeMap<String, f32>,
}

impl UiAnimationDriver {
    #[must_use]
    pub fn new(clip: UiAnimationClip, looping: bool) -> Self {
        Self {
            clip,
            time: 0.0,
            playing: true,
            looping,
            speed: 1.0,
            values: BTreeMap::new(),
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        if speed.is_finite() {
            self.speed = speed.max(0.0);
        }
    }

    pub fn restart(&mut self) {
        self.time = 0.0;
        self.playing = true;
        self.values.clear();
    }

    pub fn stop(&mut self) {
        self.playing = false;
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    #[must_use]
    pub fn time(&self) -> f32 {
        self.time
    }

    #[must_use]
    pub fn clip(&self) -> &UiAnimationClip {
        &self.clip
    }

    /// Advances the clock and returns tracks whose sampled value changed.
    pub fn advance(&mut self, delta: f32) -> Vec<(String, f32)> {
        if !self.playing || !delta.is_finite() {
            return Vec::new();
        }
        let duration = self.clip.duration();
        self.time += delta.max(0.0) * self.speed;
        if duration > 0.0 && self.time >= duration {
            if self.looping {
                self.time %= duration;
            } else {
                self.time = duration;
                self.playing = false;
            }
        }
        let mut changed = Vec::new();
        for (property, track) in &self.clip.tracks {
            let value = track.sample(self.time);
            if self.values.get(property).is_none_or(|last| *last != value) {
                self.values.insert(property.clone(), value);
                changed.push((property.clone(), value));
            }
        }
        changed
    }

    #[must_use]
    pub fn values(&self) -> &BTreeMap<String, f32> {
        &self.values
    }
}

/// Named clip set for entry/state/hint animations. Later clips override earlier
/// ones for the same property, matching "state overrides entry" in the scheme.
#[derive(Clone, Debug, Default)]
pub struct UiAnimationRuntime {
    drivers: BTreeMap<String, UiAnimationDriver>,
}

impl UiAnimationRuntime {
    pub fn play(&mut self, name: impl Into<String>, clip: UiAnimationClip, looping: bool) {
        self.drivers
            .insert(name.into(), UiAnimationDriver::new(clip, looping));
    }

    pub fn stop(&mut self, name: &str) {
        if let Some(driver) = self.drivers.get_mut(name) {
            driver.stop();
        }
    }

    pub fn advance(&mut self, delta: f32) -> BTreeMap<String, f32> {
        let mut merged = BTreeMap::new();
        for driver in self.drivers.values_mut() {
            for (property, value) in driver.advance(delta) {
                merged.insert(property, value);
            }
        }
        merged
    }

    #[must_use]
    pub fn driver(&self, name: &str) -> Option<&UiAnimationDriver> {
        self.drivers.get(name)
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

    /// 组件串语法：**唯一事实源**的门（第 40 轮）。
    ///
    /// 逐字节要求在这里成立的理由：数字格式（`{:.3}`）与轨道顺序（`BTreeMap` 字典序，
    /// 与 `UiEvents`/`UiBindings` 的多记录格式一致）都是**磁盘上既有数据的形态**，
    /// 编辑器一直在写它。这条门同时守住"编辑器不再自己编码"这件事——
    /// 编辑器现在调用的是 [`UiAnimationClip::tracks_to_component_str`]。
    #[test]
    fn component_str_round_trips_and_rejects_malformed_input() {
        let sample = "track.opacity=0.000:0.000:linear|track.scale=0.000:1.000:smoothstep";
        let tracks =
            UiAnimationClip::tracks_from_component_str(sample).expect("编辑器一直写这个格式");
        assert_eq!(tracks.len(), 2);
        assert_eq!(
            tracks["opacity"].keyframes,
            vec![UiKeyframe {
                time: 0.0,
                value: 0.0,
                easing: UiEasing::Linear
            }]
        );
        assert_eq!(
            UiAnimationClip::tracks_to_component_str(&tracks),
            sample,
            "引擎写回必须与编辑器写过的串逐字节一致"
        );
        // 多关键帧 + 三种缓动都能往返。
        let multi = "track.opacity=0.000:0.000:linear,0.250:0.500:step,0.500:1.000:smoothstep";
        let tracks = UiAnimationClip::tracks_from_component_str(multi).expect("多关键帧");
        assert_eq!(tracks["opacity"].keyframes.len(), 3);
        assert_eq!(UiAnimationClip::tracks_to_component_str(&tracks), multi);
        // 空串 = 没有轨道（编辑器删除组件时写的形态），解析成功、写回为空。
        assert!(
            UiAnimationClip::tracks_from_component_str("")
                .expect("空串合法")
                .is_empty()
        );

        for bad in [
            // 缺 `=`。
            "track.opacity",
            // 键名不是 `track.`。
            "opacity=0.000:0.000:linear",
            // 键帧不是三元组。
            "track.opacity=0.000:0.000",
            // 缓动名不认识。
            "track.opacity=0.000:0.000:bounce",
            // 时间不是数。
            "track.opacity=x:0.000:linear",
            // 时间不严格递增（`UiAnimationTrack::new` 的校验）。
            "track.opacity=0.500:0.000:linear,0.500:1.000:linear",
        ] {
            assert!(
                UiAnimationClip::tracks_from_component_str(bad).is_err(),
                "{bad:?} 必须被拒"
            );
        }
    }

    #[test]
    fn driver_reports_changed_tracks_and_honours_speed_and_stop() {
        let mut clip = UiAnimationClip::new("fade");
        clip.add_track(track()).unwrap();
        clip.add_track(
            UiAnimationTrack::new(
                "scale",
                vec![
                    UiKeyframe {
                        time: 0.0,
                        value: 1.0,
                        easing: UiEasing::Linear,
                    },
                    UiKeyframe {
                        time: 2.0,
                        value: 2.0,
                        easing: UiEasing::Linear,
                    },
                ],
            )
            .unwrap(),
        )
        .unwrap();
        let mut driver = UiAnimationDriver::new(clip.clone(), false);
        let first = driver.advance(0.25);
        assert_eq!(first.len(), 2);
        assert_eq!(driver.values()["scale"], 1.125);
        // Same clock: no changed values.
        assert!(driver.advance(0.0).is_empty());
        driver.set_speed(2.0);
        let second = driver.advance(0.25);
        assert_eq!(second.len(), 2);
        // 0.25s at 2× speed → t=0.75 of a 2s linear track → 1.375.
        assert!((driver.values()["scale"] - 1.375).abs() < 1e-6);
        driver.advance(10.0);
        assert!(!driver.is_playing());
        assert_eq!(driver.values()["scale"], 2.0);
        driver.restart();
        assert!(driver.is_playing());
        assert_eq!(driver.time(), 0.0);

        let mut runtime = UiAnimationRuntime::default();
        runtime.play("enter", clip.clone(), false);
        runtime.play("hint", clip, true);
        assert_eq!(runtime.advance(0.5).len(), 2);
        runtime.stop("hint");
        assert!(!runtime.driver("hint").unwrap().is_playing());
        assert!(runtime.driver("enter").unwrap().is_playing());
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
