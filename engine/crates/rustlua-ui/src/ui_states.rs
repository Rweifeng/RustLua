//! E20: structured visual states for reusable buttons.

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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonStateSet {
    pub normal: Option<String>,
    pub hover: Option<String>,
    pub pressed: Option<String>,
    pub disabled: Option<String>,
    pub focused: Option<String>,
    pub disabled_state: bool,
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
                _ => return Err(format!("unknown ButtonStates field: {key}")),
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
        fields.join(";")
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
    }
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
    }
}
