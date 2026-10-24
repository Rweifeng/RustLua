//! P3-E01-2：`.anim` 的运行时播放器组件。
//!
//! [`AnimationPlayer`] 把 [`crate::anim_asset::AnimClip`] 接到实体上：每帧按 `speed` 推进时间、
//! 采样属性轨道，并由宿主系统把值写回 `Transform` / `Transform3D` / `Light`；`Sprite.region`
//! 不走写回（实体上的 `Sprite` 不存 UV），而是在提取阶段按 `frames` 换算成 UV 区域——与旧
//! `AnimationStateMachine` 的 `current_frame_uv()` 同一位置、同一"不脏文档"的口径。
//!
//! 组件串（场景文件里写的那一行）：
//!
//! ```text
//! clip=assets/anim/hero.anim;autoplay=true;speed=1;looped=inherit;frames=4
//! ```
//!
//! - `clip` 必填：工程根内的 `.anim` 路径。
//! - `autoplay` 缺省 `true`，`speed` 缺省 `1`（非负；`0` 表示时间不前进）。
//! - `looped=inherit`（缺省）跟随片段自己的 `looped`；`true`/`false` 覆盖它。
//! - `frames` 是 `Sprite.region` 的 1×N 横排帧数。片段里有 `Sprite.region` 轨道时必须提供，
//!   否则加载期就报错——没有它，"第 2 帧对应哪块 UV"没有答案，静默不动比报错更难查。
//!
//! 运行时状态（当前时间、是否播放、已加载片段）**不写回组件串**：`to_component_str` 只输出
//! 上面那行配置，因此保存场景不会把播放进度写进文档。
//!
//! `UiAnimation.<property>` 不写 ECS 组件：UI 域的运行期存储是视图模型，宿主把采样值
//! 发布成 `anim.<实体名>.<属性>`（M76-02，见 `RuntimeScene::publish_ui_animation_samples`）。

use std::collections::BTreeSet;
use std::path::{Component as PathComponent, Path};

use rustlua_math::math2d::Vec2;

use crate::anim_asset::{AnimClip, AnimPath, AnimValue};

/// 一帧里触发的 `.anim` 事件（与旧 `AnimationFrameEvent` 分开：事件有时间，不是帧号）。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationPlayerEvent {
    pub time: f32,
    pub name: String,
}

/// 组件串里 `looped` 的三种取值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayerLoop {
    /// 跟随片段。
    Inherit,
    Always,
    Never,
}

impl PlayerLoop {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "inherit" => Some(Self::Inherit),
            "true" => Some(Self::Always),
            "false" => Some(Self::Never),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Always => "true",
            Self::Never => "false",
        }
    }
}

/// 一个实体上的 `.anim` 播放器。
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationPlayer {
    clip_path: String,
    autoplay: bool,
    speed: f32,
    looped: PlayerLoop,
    frames: Option<u32>,
    clip: Option<AnimClip>,
    time: f32,
    playing: bool,
    ended: bool,
}

impl AnimationPlayer {
    /// 解析场景组件串；只解析配置，不读文件（片段由宿主调用 [`Self::attach_clip`] 装上）。
    pub fn from_component_str(value: &str) -> Result<Self, AnimationPlayerError> {
        let mut clip_path: Option<String> = None;
        let mut autoplay: Option<bool> = None;
        let mut speed: Option<f32> = None;
        let mut looped: Option<PlayerLoop> = None;
        let mut frames: Option<u32> = None;
        for field in value.split(';') {
            let field = field.trim();
            if field.is_empty() {
                return Err(AnimationPlayerError::Component(
                    "empty field (stray `;`)".to_owned(),
                ));
            }
            let (key, raw) = field.split_once('=').ok_or_else(|| {
                AnimationPlayerError::Component(format!("expected key=value, got `{field}`"))
            })?;
            let raw = raw.trim();
            match key {
                "clip" => {
                    if clip_path.is_some() {
                        return Err(AnimationPlayerError::Component(
                            "duplicate `clip=`".to_owned(),
                        ));
                    }
                    validate_clip_path(raw)?;
                    clip_path = Some(raw.to_owned());
                }
                "autoplay" => {
                    if autoplay.is_some() {
                        return Err(AnimationPlayerError::Component(
                            "duplicate `autoplay=`".to_owned(),
                        ));
                    }
                    autoplay = Some(match raw {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(AnimationPlayerError::Component(format!(
                                "`autoplay` must be true or false, got `{raw}`"
                            )));
                        }
                    });
                }
                "speed" => {
                    if speed.is_some() {
                        return Err(AnimationPlayerError::Component(
                            "duplicate `speed=`".to_owned(),
                        ));
                    }
                    let parsed: f32 = raw.parse().map_err(|_| {
                        AnimationPlayerError::Component(format!(
                            "`speed` must be a number, got `{raw}`"
                        ))
                    })?;
                    if !parsed.is_finite() || parsed < 0.0 {
                        return Err(AnimationPlayerError::Component(format!(
                            "`speed` must be finite and non-negative, got `{raw}`"
                        )));
                    }
                    speed = Some(parsed);
                }
                "looped" => {
                    if looped.is_some() {
                        return Err(AnimationPlayerError::Component(
                            "duplicate `looped=`".to_owned(),
                        ));
                    }
                    looped = Some(PlayerLoop::parse(raw).ok_or_else(|| {
                        AnimationPlayerError::Component(format!(
                            "`looped` must be inherit, true or false, got `{raw}`"
                        ))
                    })?);
                }
                "frames" => {
                    if frames.is_some() {
                        return Err(AnimationPlayerError::Component(
                            "duplicate `frames=`".to_owned(),
                        ));
                    }
                    let parsed: u32 = raw.parse().map_err(|_| {
                        AnimationPlayerError::Component(format!(
                            "`frames` must be a positive integer, got `{raw}`"
                        ))
                    })?;
                    if parsed == 0 {
                        return Err(AnimationPlayerError::Component(
                            "`frames` must be at least 1".to_owned(),
                        ));
                    }
                    frames = Some(parsed);
                }
                _ => {
                    return Err(AnimationPlayerError::Component(format!(
                        "unknown field `{key}`"
                    )));
                }
            }
        }
        let clip_path = clip_path
            .ok_or_else(|| AnimationPlayerError::Component("missing `clip=`".to_owned()))?;
        let autoplay = autoplay.unwrap_or(true);
        Ok(Self {
            clip_path,
            autoplay,
            speed: speed.unwrap_or(1.0),
            looped: looped.unwrap_or(PlayerLoop::Inherit),
            frames,
            clip: None,
            time: 0.0,
            playing: autoplay,
            ended: false,
        })
    }

    /// 配置的规范编码（运行时不写回）：只含 `clip/autoplay/speed` 与显式给出的
    /// `looped/frames`，因此保存场景不会把播放进度写进文档。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut out = format!(
            "clip={};autoplay={};speed={}",
            self.clip_path, self.autoplay, self.speed
        );
        if self.looped != PlayerLoop::Inherit {
            out.push_str(";looped=");
            out.push_str(self.looped.as_str());
        }
        if let Some(frames) = self.frames {
            out.push_str(&format!(";frames={frames}"));
        }
        out
    }

    /// 装上一段已解析的片段，并校验它与组件配置是否自洽（region 需要 `frames`）。
    ///
    /// 宿主负责读文件：`rustlua-animation` 不依赖文件系统/VFS，预览与测试可以直接把
    /// 内存里的片段装进来。
    pub fn attach_clip(&mut self, clip: AnimClip) -> Result<(), AnimationPlayerError> {
        if let Some(track) = clip.track(&AnimPath::SpriteRegion) {
            let frames = self.frames.ok_or_else(|| {
                AnimationPlayerError::Component(
                    "clip has a `Sprite.region` track but the player has no `frames=`".to_owned(),
                )
            })?;
            for keyframe in &track.keyframes {
                let AnimValue::Region(index) = keyframe.value else {
                    return Err(AnimationPlayerError::Component(format!(
                        "clip `{}` has a non-region value on Sprite.region",
                        clip.name
                    )));
                };
                if index >= frames {
                    return Err(AnimationPlayerError::Component(format!(
                        "clip `{}` addresses region {index} but `frames={frames}`",
                        clip.name
                    )));
                }
            }
        }
        self.time = 0.0;
        self.playing = self.autoplay;
        self.ended = false;
        self.clip = Some(clip);
        Ok(())
    }

    #[must_use]
    pub fn clip_path(&self) -> &str {
        &self.clip_path
    }

    #[must_use]
    pub fn clip(&self) -> Option<&AnimClip> {
        self.clip.as_ref()
    }

    #[must_use]
    pub fn frames(&self) -> Option<u32> {
        self.frames
    }

    #[must_use]
    pub fn autoplay(&self) -> bool {
        self.autoplay
    }

    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    #[must_use]
    pub fn time(&self) -> f32 {
        self.time
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    #[must_use]
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// 实际生效的循环策略：组件显式值优先，否则跟随片段。
    #[must_use]
    pub fn effective_looped(&self) -> bool {
        match self.looped {
            PlayerLoop::Inherit => self.clip.as_ref().is_some_and(|clip| clip.looped),
            PlayerLoop::Always => true,
            PlayerLoop::Never => false,
        }
    }

    #[must_use]
    pub fn duration(&self) -> f32 {
        self.clip.as_ref().map_or(0.0, |clip| clip.duration)
    }

    pub fn set_playing(&mut self, playing: bool) {
        if playing {
            self.ended = false;
        }
        self.playing = playing;
    }

    /// 停止：暂停并把时间归零（下一次 `play` 从片头开始）。
    pub fn stop(&mut self) {
        self.playing = false;
        self.time = 0.0;
        self.ended = false;
    }

    /// 定位到 `seconds`（循环片段折回 `[0, duration)`，非循环夹到 `[0, duration]`）。
    pub fn seek(&mut self, seconds: f32) -> Result<(), AnimationPlayerError> {
        let Some(clip) = &self.clip else {
            return Err(AnimationPlayerError::Component(
                "player has no loaded clip".to_owned(),
            ));
        };
        if !seconds.is_finite() {
            return Err(AnimationPlayerError::Component(
                "seek time must be finite".to_owned(),
            ));
        }
        let duration = clip.duration;
        self.time = if self.effective_looped() && duration > 0.0 {
            seconds.rem_euclid(duration)
        } else {
            seconds.clamp(0.0, duration)
        };
        self.ended = false;
        Ok(())
    }

    /// 推进 `delta` 秒，返回本步触发的事件（`(from, to]`，片头 `0` 时刻的事件在从 0
    /// 出发的那一步触发；跨圈只返回每个事件一次）。
    pub fn advance(&mut self, delta: f32) -> Vec<AnimationPlayerEvent> {
        let Some(clip) = &self.clip else {
            return Vec::new();
        };
        if !self.playing || !delta.is_finite() || delta <= 0.0 || self.speed <= 0.0 {
            return Vec::new();
        }
        let duration = clip.duration;
        if duration <= 0.0 {
            return Vec::new();
        }
        let from = self.time;
        let to = from + delta * self.speed;
        let looped = self.effective_looped();
        let mut due: Vec<AnimationPlayerEvent> = clip
            .events
            .iter()
            .filter(|event| {
                let reached = if looped && to >= duration {
                    let wrapped_to = to.rem_euclid(duration);
                    // 新一圈从 0 重新开始，片头（0 时刻）事件随回绕再触发。
                    (event.time > from && event.time <= duration) || event.time <= wrapped_to
                } else {
                    event.time > from && event.time <= to
                };
                reached || (from == 0.0 && event.time == 0.0)
            })
            .map(|event| AnimationPlayerEvent {
                time: event.time,
                name: event.name.clone(),
            })
            .collect();
        due.sort_by(|left, right| {
            left.time
                .partial_cmp(&right.time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if looped {
            self.time = to.rem_euclid(duration);
        } else if to >= duration {
            self.time = duration;
            self.playing = false;
            self.ended = true;
        } else {
            self.time = to;
        }
        due
    }

    /// 当前时间下全部轨道的采样值（时间已按循环策略折回）。供宿主写回组件。
    #[must_use]
    pub fn sample_all(&self) -> Vec<(AnimPath, AnimValue)> {
        let Some(clip) = &self.clip else {
            return Vec::new();
        };
        let duration = clip.duration;
        let time = if self.effective_looped() && duration > 0.0 {
            self.time.rem_euclid(duration)
        } else {
            self.time.min(duration)
        };
        clip.tracks
            .iter()
            .filter_map(|track| track.sample(time).map(|value| (track.path.clone(), value)))
            .collect()
    }

    /// `Sprite.region` 当前帧的 UV 区域（1×N 横排）。没有 region 轨道或没有 `frames` ⇒ `None`。
    #[must_use]
    pub fn current_region_uv(&self) -> Option<(Vec2, Vec2)> {
        let frames = self.frames?;
        let value = self.sample_all().into_iter().find_map(|(path, value)| {
            if path == AnimPath::SpriteRegion {
                Some(value)
            } else {
                None
            }
        })?;
        let AnimValue::Region(index) = value else {
            return None;
        };
        if frames == 0 || index >= frames {
            return None;
        }
        let start = index as f32 / frames as f32;
        let end = (index + 1) as f32 / frames as f32;
        Some((Vec2::new(start, 0.0), Vec2::new(end, 1.0)))
    }

    /// 片段里**不写回 ECS 组件**的目标（目前只有 `UiAnimation.*`：宿主改发视图模型）。
    #[must_use]
    pub fn unsupported_paths(&self) -> BTreeSet<String> {
        let Some(clip) = &self.clip else {
            return BTreeSet::new();
        };
        clip.tracks
            .iter()
            .filter(|track| matches!(track.path, AnimPath::UiAnimation(_)))
            .map(|track| track.path.to_string())
            .collect()
    }
}

fn validate_clip_path(raw: &str) -> Result<(), AnimationPlayerError> {
    if raw.is_empty() {
        return Err(AnimationPlayerError::Component(
            "`clip` is empty".to_owned(),
        ));
    }
    if raw.chars().any(char::is_whitespace) {
        return Err(AnimationPlayerError::Component(format!(
            "`clip` must not contain whitespace, got `{raw}`"
        )));
    }
    let path = Path::new(raw);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                PathComponent::ParentDir | PathComponent::RootDir | PathComponent::Prefix(_)
            )
        })
    {
        return Err(AnimationPlayerError::Component(format!(
            "`clip` must be a project-relative path without `..`, got `{raw}`"
        )));
    }
    if path.extension().and_then(|extension| extension.to_str()) != Some("anim") {
        return Err(AnimationPlayerError::Component(format!(
            "`clip` must point at a `.anim` file, got `{raw}`"
        )));
    }
    Ok(())
}

/// 播放器组件/片段装配错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AnimationPlayerError {
    #[error("{0}")]
    Component(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIP: &str = "\
version=1
name=hero
duration=1
looped=true
track=Transform.position;interp=linear;keys=0:0,0;1:10,20
track=Sprite.region;interp=step;keys=0:0;0.5:1
event=0:start
event=0.5:step
";

    fn player(component: &str) -> AnimationPlayer {
        let mut player = AnimationPlayer::from_component_str(component).expect("valid component");
        player
            .attach_clip(AnimClip::from_text("hero.anim", CLIP).expect("valid clip"))
            .expect("clip fits the player");
        player
    }

    /// 组件串：默认值、规范往返与坏字段。
    #[test]
    fn component_string_round_trips_and_rejects_bad_fields() {
        let parsed = AnimationPlayer::from_component_str(
            "clip=assets/anim/hero.anim;autoplay=false;speed=2;looped=true;frames=4",
        )
        .expect("valid");
        assert_eq!(parsed.clip_path(), "assets/anim/hero.anim");
        assert!(!parsed.autoplay());
        assert_eq!(parsed.speed(), 2.0);
        assert_eq!(parsed.frames(), Some(4));
        assert_eq!(
            parsed.to_component_str(),
            "clip=assets/anim/hero.anim;autoplay=false;speed=2;looped=true;frames=4"
        );

        let defaults = AnimationPlayer::from_component_str("clip=hero.anim").expect("valid");
        assert!(defaults.autoplay());
        assert_eq!(defaults.speed(), 1.0);
        assert_eq!(defaults.frames(), None);
        assert_eq!(
            defaults.to_component_str(),
            "clip=hero.anim;autoplay=true;speed=1",
            "缺省 looped 不写出来（跟随片段）"
        );

        for (raw, needle) in [
            ("autoplay=true", "missing `clip=`"),
            ("clip=", "`clip` is empty"),
            ("clip=../hero.anim", "project-relative"),
            ("clip=/hero.anim", "project-relative"),
            ("clip=hero.txt", "`.anim`"),
            ("clip=hero.anim;nope=1", "unknown field"),
            ("clip=hero.anim;clip=other.anim", "duplicate `clip=`"),
            ("clip=hero.anim;autoplay=yes", "true or false"),
            ("clip=hero.anim;speed=-1", "non-negative"),
            ("clip=hero.anim;speed=inf", "finite"),
            ("clip=hero.anim;looped=yes", "inherit, true or false"),
            ("clip=hero.anim;frames=0", "at least 1"),
            ("clip=hero.anim;;frames=2", "empty field"),
        ] {
            let error = AnimationPlayer::from_component_str(raw).expect_err(raw);
            assert!(error.to_string().contains(needle), "{raw}: {error}");
        }
    }

    /// 采样、UV 区域、事件与跨圈只触发一次。
    #[test]
    fn advances_samples_regions_and_events_once_per_crossing() {
        let mut player = player("clip=hero.anim;frames=2");
        assert!(player.is_playing());
        assert!(player.effective_looped(), "缺省跟随片段的 looped=true");
        assert_eq!(player.duration(), 1.0);

        let names = |events: Vec<AnimationPlayerEvent>| -> Vec<String> {
            events.into_iter().map(|event| event.name).collect()
        };
        assert_eq!(
            names(player.advance(0.25)),
            vec!["start".to_owned()],
            "0 时刻的事件在第一次推进时触发"
        );
        let sampled = player.sample_all();
        let position = sampled
            .iter()
            .find(|(path, _)| {
                *path == AnimPath::Transform(crate::anim_asset::TransformChannel::Position)
            })
            .map(|(_, value)| *value);
        assert_eq!(position, Some(AnimValue::Vec2(Vec2::new(2.5, 5.0))));
        assert_eq!(
            player.current_region_uv(),
            Some((Vec2::new(0.0, 0.0), Vec2::new(0.5, 1.0)))
        );

        assert_eq!(
            names(player.advance(0.25)),
            vec!["step".to_owned()],
            "0.5 的事件恰好落在边界上，触发一次"
        );
        assert_eq!(
            player.current_region_uv(),
            Some((Vec2::new(0.5, 0.0), Vec2::new(1.0, 1.0)))
        );
        assert!(
            player.advance(0.1).is_empty(),
            "同一个事件不能因为闭区间重复触发"
        );

        // 0.6 -> 1.1：跨过一圈，回到 0.1，0 时刻的事件在新一圈再次触发。
        assert_eq!(
            names(player.advance(0.5)),
            vec!["start".to_owned()],
            "跨圈后片头事件再次触发"
        );
        assert!((player.time() - 0.1).abs() < 1e-6);
        assert_eq!(
            player.current_region_uv(),
            Some((Vec2::new(0.0, 0.0), Vec2::new(0.5, 1.0)))
        );
    }

    /// 非循环片段停在时长处、`ended` 置位；pause/stop/seek 的语义。
    #[test]
    fn non_looping_stops_and_controls_seek_pause_stop() {
        let clip = "\
version=1
name=once
duration=1
looped=false
track=Light.intensity;interp=linear;keys=0:1;1:3
";
        let mut player = AnimationPlayer::from_component_str("clip=once.anim;looped=false")
            .expect("valid component");
        player
            .attach_clip(AnimClip::from_text("once.anim", clip).expect("valid clip"))
            .expect("attach");
        player.advance(1.5);
        assert_eq!(player.time(), 1.0);
        assert!(!player.is_playing());
        assert!(player.ended());
        assert_eq!(player.sample_all()[0].1, AnimValue::Scalar(3.0), "到达末键");

        player.set_playing(true);
        assert!(!player.ended(), "play 清掉 ended");
        player.seek(0.5).expect("seek");
        assert!((player.time() - 0.5).abs() < 1e-6);
        player.set_playing(false);
        assert!(player.advance(0.5).is_empty(), "暂停时不推进");
        assert!((player.time() - 0.5).abs() < 1e-6);
        player.stop();
        assert_eq!(player.time(), 0.0);
        assert!(!player.is_playing());
        assert!(player.seek(f32::NAN).is_err());

        // 组件覆盖 looped=true 时，同一片段折回。
        let mut looping = AnimationPlayer::from_component_str("clip=once.anim;looped=true")
            .expect("valid component");
        looping
            .attach_clip(AnimClip::from_text("once.anim", clip).expect("valid clip"))
            .expect("attach");
        looping.advance(1.25);
        assert!((looping.time() - 0.25).abs() < 1e-6);
    }

    /// region 轨道必须有 frames，且帧号必须落在范围内。
    #[test]
    fn sprite_regions_require_frames_and_stay_in_range() {
        let clip = AnimClip::from_text("hero.anim", CLIP).expect("valid clip");
        let mut missing = AnimationPlayer::from_component_str("clip=hero.anim").expect("valid");
        let error = missing.attach_clip(clip.clone()).expect_err("needs frames");
        assert!(error.to_string().contains("no `frames=`"), "{error}");

        let mut too_small =
            AnimationPlayer::from_component_str("clip=hero.anim;frames=1").expect("valid");
        let error = too_small
            .attach_clip(clip)
            .expect_err("region 1 needs 2 frames");
        assert!(error.to_string().contains("frames=1"), "{error}");
    }

    /// UI 轨道被保留但明确列为"运行时未写回"，不是静默忽略。
    #[test]
    fn ui_animation_tracks_are_reported_as_unsupported() {
        let clip = "\
version=1
name=ui
duration=1
looped=false
track=UiAnimation.opacity;interp=linear;keys=0:0;1:1
";
        let mut player = AnimationPlayer::from_component_str("clip=ui.anim").expect("valid");
        player
            .attach_clip(AnimClip::from_text("ui.anim", clip).expect("valid clip"))
            .expect("attach");
        assert_eq!(
            player.unsupported_paths().into_iter().collect::<Vec<_>>(),
            vec!["UiAnimation.opacity".to_owned()]
        );
    }

    /// 装配前的空播放器不会 panic，也没有采样。
    #[test]
    fn unloaded_player_is_inert() {
        let mut player = AnimationPlayer::from_component_str("clip=missing.anim").expect("valid");
        assert!(player.advance(1.0).is_empty());
        assert!(player.sample_all().is_empty());
        assert!(player.current_region_uv().is_none());
        assert!(player.seek(0.5).is_err());
        assert_eq!(player.duration(), 0.0);
    }
}
