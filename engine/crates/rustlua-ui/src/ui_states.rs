//! E20: structured visual states for reusable buttons.
//!
//! 除了贴图覆盖，状态还带底色覆盖（`tint.<state>`）与过渡时长/曲线（`transition`）：
//! M80-07 的样式状态色走的就是这一条，插值复用动画内核的 [`UiEasing`]，不新造曲线。

use std::collections::BTreeMap;

use crate::ui_animation::UiEasing;

/// The five visual states supported by reusable button widgets.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ButtonVisualState {
    Normal,
    Hover,
    Pressed,
    Disabled,
    Focused,
}

impl ButtonVisualState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Hover => "hover",
            Self::Pressed => "pressed",
            Self::Disabled => "disabled",
            Self::Focused => "focused",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "normal" => Some(Self::Normal),
            "hover" => Some(Self::Hover),
            "pressed" => Some(Self::Pressed),
            "disabled" => Some(Self::Disabled),
            "focused" => Some(Self::Focused),
            _ => None,
        }
    }
}

/// A `ButtonStates` component value. Texture fields are project-relative
/// references; a missing state falls back to `normal`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ButtonStateSet {
    pub normal: Option<String>,
    pub hover: Option<String>,
    pub pressed: Option<String>,
    pub disabled: Option<String>,
    pub focused: Option<String>,
    pub disabled_state: bool,
    /// 状态底色覆盖（0..=1 的 RGBA）。缺失的状态回退到作者底色本身。
    pub tints: BTreeMap<ButtonVisualState, [f32; 4]>,
    /// 状态之间的底色过渡；`duration` 为 0 时立刻切换（与没有过渡等价）。
    pub transition: UiStateTransition,
}

/// 底色过渡的时长与曲线（M80-07）。曲线直接复用动画内核的 [`UiEasing`]。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UiStateTransition {
    pub duration: f32,
    pub easing: UiEasing,
}

impl UiStateTransition {
    #[must_use]
    pub fn is_blendable(self) -> bool {
        self.duration.is_finite() && self.duration > 0.0
    }
}

/// 状态底色的仿射表示：显示色 = `作者底色 * scale + add`。
///
/// `Normal` 是 `(1, [0,0,0,0])`，绝对色是 `(0, color)`。写成仿射而不是绝对值，
/// 是因为过渡被打断时要从**当前显示色**继续——那一刻宿主并不需要知道作者底色是多少。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiStateTint {
    pub scale: f32,
    pub add: [f32; 4],
}

impl UiStateTint {
    pub const NORMAL: Self = Self {
        scale: 1.0,
        add: [0.0, 0.0, 0.0, 0.0],
    };

    #[must_use]
    pub const fn absolute(color: [f32; 4]) -> Self {
        Self {
            scale: 0.0,
            add: color,
        }
    }

    #[must_use]
    pub fn resolve(self, base: [f32; 4]) -> [f32; 4] {
        [
            base[0] * self.scale + self.add[0],
            base[1] * self.scale + self.add[1],
            base[2] * self.scale + self.add[2],
            base[3] * self.scale + self.add[3],
        ]
    }

    /// 两端点都是仿射，插值后仍是仿射：`weight` 取已打过 easing 的值。
    #[must_use]
    pub fn lerp(self, to: Self, weight: f32) -> Self {
        let mix = |from: f32, to: f32| from + (to - from) * weight;
        Self {
            scale: mix(self.scale, to.scale),
            add: [
                mix(self.add[0], to.add[0]),
                mix(self.add[1], to.add[1]),
                mix(self.add[2], to.add[2]),
                mix(self.add[3], to.add[3]),
            ],
        }
    }
}

/// 一次进行中的底色过渡。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiStateBlend {
    pub from: UiStateTint,
    pub to: UiStateTint,
    pub elapsed: f32,
    pub duration: f32,
    pub easing: UiEasing,
}

impl UiStateBlend {
    /// 0..=1 的时间进度打过曲线之后的值。
    #[must_use]
    pub fn weight(&self) -> f32 {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return 1.0;
        }
        self.easing
            .apply((self.elapsed / self.duration).clamp(0.0, 1.0))
    }

    #[must_use]
    pub fn tint(&self) -> UiStateTint {
        self.from.lerp(self.to, self.weight())
    }

    pub fn advance(&mut self, delta: f32) {
        self.elapsed = (self.elapsed + delta.max(0.0)).min(self.duration);
    }

    #[must_use]
    pub fn settled(&self) -> bool {
        self.elapsed >= self.duration
    }
}

/// 节点名 → 进行中的底色过渡。
#[derive(Clone, Debug, Default)]
pub struct UiStateTransitions {
    blends: BTreeMap<String, UiStateBlend>,
}

impl UiStateTransitions {
    /// 状态从 `previous` 变到 `next` 时重定目标。没有配过渡（时长 0）时不留条目，
    /// 读取端因此总是"没有进行中的过渡 ⇒ 直接取当前状态的色"。
    pub fn on_state_changed(
        &mut self,
        key: &str,
        states: &ButtonStateSet,
        previous: Option<ButtonVisualState>,
        next: ButtonVisualState,
    ) {
        if !states.transition.is_blendable() {
            self.blends.remove(key);
            return;
        }
        let to = states.tint(next);
        let from = self
            .blends
            .get(key)
            .map(|blend| blend.tint())
            .unwrap_or_else(|| states.tint(previous.unwrap_or(ButtonVisualState::Normal)));
        if from == to {
            self.blends.remove(key);
            return;
        }
        self.blends.insert(
            key.to_owned(),
            UiStateBlend {
                from,
                to,
                elapsed: 0.0,
                duration: states.transition.duration,
                easing: states.transition.easing,
            },
        );
    }

    pub fn advance(&mut self, delta: f32) {
        for blend in self.blends.values_mut() {
            blend.advance(delta);
        }
    }

    #[must_use]
    pub fn tint_of(&self, key: &str) -> Option<UiStateTint> {
        self.blends.get(key).map(UiStateBlend::tint)
    }

    /// 交互树换键（重名时是 `名字#槽位`）之外，实体消失后条目必须跟着清，
    /// 否则名字复用会把上一个按钮的过渡尾巴画到新按钮上。
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.blends.retain(|key, _| keep(key));
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blends.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.blends.len()
    }
}

impl ButtonStateSet {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut set = Self::default();
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("malformed ButtonStates field: {field}"));
            };
            match key {
                "normal" => set.normal = non_empty(value)?,
                "hover" => set.hover = non_empty(value)?,
                "pressed" => set.pressed = non_empty(value)?,
                "disabled" => set.disabled = non_empty(value)?,
                "focused" => set.focused = non_empty(value)?,
                "disabled_state" => {
                    set.disabled_state = match value {
                        "true" | "1" => true,
                        "false" | "0" => false,
                        other => return Err(format!("invalid disabled_state: {other:?}")),
                    }
                }
                // `tint.<state>=r,g,b,a`：状态底色覆盖，各通道 0..=1。
                other => {
                    if let Some(state) = other.strip_prefix("tint.") {
                        let parsed = ButtonVisualState::parse(state)
                            .ok_or_else(|| format!("unknown tint state: {state}"))?;
                        set.tints.insert(parsed, parse_rgba(value)?);
                    } else if other == "transition" {
                        set.transition = parse_transition(value)?;
                    } else {
                        return Err(format!("unknown ButtonStates field: {key}"));
                    }
                }
            }
        }
        Ok(set)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut fields = Vec::new();
        for (state, value) in [
            (ButtonVisualState::Normal, &self.normal),
            (ButtonVisualState::Hover, &self.hover),
            (ButtonVisualState::Pressed, &self.pressed),
            (ButtonVisualState::Disabled, &self.disabled),
            (ButtonVisualState::Focused, &self.focused),
        ] {
            if let Some(value) = value {
                fields.push(format!("{}={}", state.as_str(), value));
            }
        }
        if self.disabled_state {
            fields.push("disabled_state=true".to_owned());
        }
        for (state, color) in &self.tints {
            fields.push(format!(
                "tint.{}={},{},{},{}",
                state.as_str(),
                color[0],
                color[1],
                color[2],
                color[3]
            ));
        }
        if self.transition.duration > 0.0 {
            fields.push(format!(
                "transition={}:{}",
                self.transition.duration,
                self.transition.easing.as_str()
            ));
        }
        fields.join(";")
    }

    /// 某状态的底色覆盖；没写过就回退到 `tint.normal`，再没有就是"用作者底色"，
    /// 与 [`Self::texture`] 的回退口径一致。
    #[must_use]
    pub fn tint(&self, state: ButtonVisualState) -> UiStateTint {
        self.tints
            .get(&state)
            .or_else(|| {
                (state != ButtonVisualState::Normal)
                    .then(|| self.tints.get(&ButtonVisualState::Normal))
                    .flatten()
            })
            .map_or(UiStateTint::NORMAL, |color| UiStateTint::absolute(*color))
    }

    #[must_use]
    pub fn texture(&self, state: ButtonVisualState) -> Option<&str> {
        let requested = match state {
            ButtonVisualState::Normal => self.normal.as_deref(),
            ButtonVisualState::Hover => self.hover.as_deref(),
            ButtonVisualState::Pressed => self.pressed.as_deref(),
            ButtonVisualState::Disabled => self.disabled.as_deref(),
            ButtonVisualState::Focused => self.focused.as_deref(),
        };
        requested.or(self.normal.as_deref())
    }

    #[must_use]
    pub fn has_overrides(&self) -> bool {
        self.hover.is_some()
            || self.pressed.is_some()
            || self.disabled.is_some()
            || self.focused.is_some()
            || self
                .tints
                .iter()
                .any(|(state, _)| *state != ButtonVisualState::Normal)
    }
}

/// `r,g,b,a`（0..=1，四个都要能解析成有限数）。
fn parse_rgba(raw: &str) -> Result<[f32; 4], String> {
    let parts = raw.split(',').collect::<Vec<_>>();
    let [r, g, b, a] = parts.as_slice() else {
        return Err(format!("tint 需要 4 个通道: {raw}"));
    };
    let channel = |value: &str| -> Result<f32, String> {
        let parsed = value
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("invalid tint channel: {value:?}"))?;
        if parsed.is_finite() && (0.0..=1.0).contains(&parsed) {
            Ok(parsed)
        } else {
            Err(format!("tint 通道要在 0..=1 之间: {value:?}"))
        }
    };
    Ok([channel(r)?, channel(g)?, channel(b)?, channel(a)?])
}

/// `transition=<秒>:<曲线>`，曲线省略时是 `linear`。
fn parse_transition(raw: &str) -> Result<UiStateTransition, String> {
    let (duration, easing) = match raw.split_once(':') {
        Some((duration, easing)) => (
            duration,
            UiEasing::parse(easing).ok_or_else(|| format!("invalid easing: {easing:?}"))?,
        ),
        None => (raw, UiEasing::Linear),
    };
    let duration = duration
        .trim()
        .parse::<f32>()
        .map_err(|_| format!("invalid transition duration: {duration:?}"))?;
    if !duration.is_finite() || duration < 0.0 {
        return Err(format!("transition 时长要是非负有限数: {raw}"));
    }
    Ok(UiStateTransition { duration, easing })
}

fn non_empty(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_five_states() {
        let raw = "normal=a.png;hover=b.png;pressed=c.png;disabled=d.png;focused=e.png;disabled_state=true";
        let states = ButtonStateSet::from_component_str(raw).unwrap();
        assert_eq!(states.texture(ButtonVisualState::Pressed), Some("c.png"));
        assert_eq!(
            ButtonStateSet::from_component_str(&states.to_component_str()).unwrap(),
            states
        );
        assert!(states.has_overrides());
    }

    #[test]
    fn unspecified_states_fall_back_to_normal() {
        let states = ButtonStateSet::from_component_str("normal=base.png").unwrap();
        assert_eq!(states.texture(ButtonVisualState::Hover), Some("base.png"));
        assert!(!states.has_overrides());
    }

    #[test]
    fn rejects_unknown_fields_and_values() {
        assert!(ButtonStateSet::from_component_str("bad=1").is_err());
        assert!(ButtonStateSet::from_component_str("disabled_state=maybe").is_err());
        assert!(ButtonStateSet::from_component_str("tint.hover=0.1,0.2,0.3").is_err());
        assert!(ButtonStateSet::from_component_str("tint.hover=0,0,0,2").is_err());
        assert!(ButtonStateSet::from_component_str("tint.bounce=0,0,0,1").is_err());
        assert!(ButtonStateSet::from_component_str("transition=0.2:wobble").is_err());
    }

    /// M80-07：状态底色与过渡时长/曲线走同一条组件文法，能原样写回。
    #[test]
    fn parses_and_round_trips_tints_and_transition() {
        let raw = "hover=0.9,0.8,0.7,1;transition=0.2:smoothstep;tint.hover=0.25,0.5,0.75,0.5";
        let states = ButtonStateSet::from_component_str(raw).unwrap();
        assert_eq!(
            states.texture(ButtonVisualState::Hover),
            Some("0.9,0.8,0.7,1")
        );
        assert_eq!(
            states.tint(ButtonVisualState::Hover),
            UiStateTint::absolute([0.25, 0.5, 0.75, 0.5])
        );
        assert_eq!(states.transition.duration, 0.2);
        assert_eq!(states.transition.easing, UiEasing::SmoothStep);
        let encoded = ButtonStateSet::from_component_str(&states.to_component_str()).unwrap();
        assert_eq!(encoded, states);
        assert!(states.has_overrides());
    }

    /// 没写 `tint.<state>` 的状态回退到 `tint.normal`，再没有就是作者底色本身。
    #[test]
    fn tint_falls_back_to_normal_then_base() {
        let base = ButtonStateSet::default();
        assert_eq!(base.tint(ButtonVisualState::Hover), UiStateTint::NORMAL);
        assert_eq!(
            base.tint(ButtonVisualState::Hover)
                .resolve([1.0, 0.5, 0.0, 1.0]),
            [1.0, 0.5, 0.0, 1.0]
        );
        let mut with_normal = ButtonStateSet::default();
        with_normal
            .tints
            .insert(ButtonVisualState::Normal, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(
            with_normal.tint(ButtonVisualState::Focused),
            UiStateTint::absolute([0.1, 0.2, 0.3, 1.0])
        );
        assert!(!with_normal.has_overrides(), "只写 normal 不算状态覆盖");
    }

    /// 过渡的数学：时长到点即停，`Step` 曲线在终点才跳，被打断时从当前显示色续走。
    #[test]
    fn blend_advances_with_easing_and_survives_interruption() {
        let mut states = ButtonStateSet::default();
        states
            .tints
            .insert(ButtonVisualState::Hover, [0.0, 0.0, 1.0, 1.0]);
        states.transition = UiStateTransition {
            duration: 1.0,
            easing: UiEasing::Linear,
        };
        let mut transitions = UiStateTransitions::default();
        let base = [1.0, 1.0, 1.0, 1.0];

        transitions.on_state_changed(
            "btn",
            &states,
            Some(ButtonVisualState::Normal),
            ButtonVisualState::Hover,
        );
        transitions.advance(0.25);
        assert_eq!(
            transitions.tint_of("btn").unwrap().resolve(base),
            [0.75, 0.75, 1.0, 1.0]
        );
        // 半途改回 Normal：起点必须是此刻的显示色，而不是跳回纯底色。
        transitions.on_state_changed(
            "btn",
            &states,
            Some(ButtonVisualState::Hover),
            ButtonVisualState::Normal,
        );
        let from = transitions.tint_of("btn").unwrap();
        assert_eq!(from.resolve(base), [0.75, 0.75, 1.0, 1.0]);
        transitions.advance(1.0);
        assert_eq!(transitions.tint_of("btn").unwrap().resolve(base), base);

        // 时长 0 ⇒ 不建过渡条目，读取端直接取当前状态的色。
        states.transition = UiStateTransition::default();
        transitions.on_state_changed(
            "btn",
            &states,
            Some(ButtonVisualState::Normal),
            ButtonVisualState::Hover,
        );
        assert!(transitions.is_empty());
    }

    #[test]
    fn smooth_step_eases_the_blend_endpoint_velocities() {
        let blend = UiStateBlend {
            from: UiStateTint::NORMAL,
            to: UiStateTint::absolute([0.0, 0.0, 0.0, 1.0]),
            elapsed: 0.5,
            duration: 1.0,
            easing: UiEasing::SmoothStep,
        };
        assert_eq!(blend.weight(), 0.5);
        assert_eq!(blend.tint().scale, 0.5);
        let step = UiStateBlend {
            easing: UiEasing::Step,
            ..blend
        };
        assert_eq!(step.weight(), 0.0);
        assert!(!step.settled());
    }

    /// 实体消失后要能摘掉过渡条目：名字复用不能把上一个按钮的尾巴画过来。
    #[test]
    fn retain_prunes_finished_entries() {
        let mut states = ButtonStateSet::default();
        states
            .tints
            .insert(ButtonVisualState::Pressed, [1.0, 0.0, 0.0, 1.0]);
        states.transition = UiStateTransition {
            duration: 0.2,
            easing: UiEasing::Linear,
        };
        let mut transitions = UiStateTransitions::default();
        transitions.on_state_changed(
            "a",
            &states,
            Some(ButtonVisualState::Normal),
            ButtonVisualState::Pressed,
        );
        transitions.on_state_changed(
            "b",
            &states,
            Some(ButtonVisualState::Normal),
            ButtonVisualState::Pressed,
        );
        assert_eq!(transitions.len(), 2);
        transitions.retain(|key| key != "b");
        assert_eq!(transitions.len(), 1);
        assert!(transitions.tint_of("b").is_none());
    }
}
