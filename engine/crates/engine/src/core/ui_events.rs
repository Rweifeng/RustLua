//! E22: structured UI event bindings with legacy `clickEvent` conversion.

use std::{collections::BTreeMap, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiEventType {
    PointerEnter,
    PointerExit,
    PointerDown,
    PointerUp,
    Click,
    Focus,
    Submit,
    ValueChanged,
    Drag,
}

impl UiEventType {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "pointer_enter" => Some(Self::PointerEnter),
            "pointer_exit" => Some(Self::PointerExit),
            "pointer_down" => Some(Self::PointerDown),
            "pointer_up" => Some(Self::PointerUp),
            "click" => Some(Self::Click),
            "focus" => Some(Self::Focus),
            "submit" => Some(Self::Submit),
            "value_changed" => Some(Self::ValueChanged),
            "drag" => Some(Self::Drag),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PointerEnter => "pointer_enter",
            Self::PointerExit => "pointer_exit",
            Self::PointerDown => "pointer_down",
            Self::PointerUp => "pointer_up",
            Self::Click => "click",
            Self::Focus => "focus",
            Self::Submit => "submit",
            Self::ValueChanged => "value_changed",
            Self::Drag => "drag",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiEventBinding {
    pub event: Option<UiEventType>,
    pub target: Option<String>,
    pub function: String,
    pub arguments: BTreeMap<String, String>,
}

impl UiEventBinding {
    fn parse(raw: &str) -> Result<Self, UiEventError> {
        let mut binding = Self::default();
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(UiEventError::Malformed(field.to_owned()));
            };
            match key {
                "event" => {
                    binding.event = Some(
                        UiEventType::parse(value)
                            .ok_or_else(|| UiEventError::UnknownEvent(value.to_owned()))?,
                    );
                }
                "target" => {
                    if !value.is_empty() {
                        binding.target = Some(value.to_owned());
                    }
                }
                "function" => binding.function = value.to_owned(),
                key if key.starts_with("arg.") => {
                    binding
                        .arguments
                        .insert(key[4..].to_owned(), value.to_owned());
                }
                _ => return Err(UiEventError::UnknownField(key.to_owned())),
            }
        }
        if binding.function.is_empty() {
            return Err(UiEventError::MissingFunction);
        }
        Ok(binding)
    }

    fn to_text(&self) -> String {
        let mut fields = vec![format!(
            "event={}",
            self.event.map_or("click", UiEventType::as_str)
        )];
        if let Some(target) = &self.target {
            fields.push(format!("target={target}"));
        }
        fields.push(format!("function={}", self.function));
        for (key, value) in &self.arguments {
            fields.push(format!("arg.{key}={value}"));
        }
        fields.join(";")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiEvents {
    pub bindings: Vec<UiEventBinding>,
}

impl UiEvents {
    #[must_use]
    pub fn click(function: impl Into<String>) -> Self {
        Self {
            bindings: vec![UiEventBinding {
                event: Some(UiEventType::Click),
                target: None,
                function: function.into(),
                arguments: BTreeMap::new(),
            }],
        }
    }

    /// Converts the existing empty/legacy `clickEvent` component. The runtime
    /// currently calls global `on_click(entity_name)`, so that is the default.
    #[must_use]
    pub fn from_legacy_click_event(raw: &str) -> Self {
        let mut events = Self::parse(raw).unwrap_or_default();
        if events.bindings.is_empty() {
            events = Self::click("on_click");
        }
        events
    }

    pub fn parse(raw: &str) -> Result<Self, UiEventError> {
        let mut bindings = Vec::new();
        for binding in raw.split('|') {
            let binding = binding.trim();
            if binding.is_empty() {
                continue;
            }
            bindings.push(UiEventBinding::parse(binding)?);
        }
        Ok(Self { bindings })
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        self.bindings
            .iter()
            .map(UiEventBinding::to_text)
            .collect::<Vec<_>>()
            .join("|")
    }

    /// Editor-facing validation. `entity_names` and `function_names` are the
    /// scene/project catalogs visible to the user; diagnostics are returned
    /// instead of silently dropping invalid bindings.
    #[must_use]
    pub fn validate(
        &self,
        entity_names: &std::collections::BTreeSet<String>,
        function_names: &std::collections::BTreeSet<String>,
    ) -> Vec<UiEventDiagnostic> {
        let mut diagnostics = Vec::new();
        for (index, binding) in self.bindings.iter().enumerate() {
            if binding.function.is_empty() {
                diagnostics.push(UiEventDiagnostic {
                    binding_index: index,
                    message: "function is empty".to_owned(),
                });
            } else if !function_names.is_empty() && !function_names.contains(&binding.function) {
                diagnostics.push(UiEventDiagnostic {
                    binding_index: index,
                    message: format!("function {} was not found", binding.function),
                });
            }
            if let Some(target) = &binding.target
                && !entity_names.contains(target)
            {
                diagnostics.push(UiEventDiagnostic {
                    binding_index: index,
                    message: format!("target entity {target} was not found"),
                });
            }
        }
        diagnostics
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiEventError {
    Malformed(String),
    UnknownEvent(String),
    UnknownField(String),
    MissingFunction,
}

impl fmt::Display for UiEventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(value) => write!(f, "malformed event binding: {value}"),
            Self::UnknownEvent(value) => write!(f, "unknown event type: {value}"),
            Self::UnknownField(value) => write!(f, "unknown event field: {value}"),
            Self::MissingFunction => write!(f, "event function is required"),
        }
    }
}

impl std::error::Error for UiEventError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiEventDiagnostic {
    pub binding_index: usize,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_legacy_click_event_to_structured_binding() {
        let events = UiEvents::from_legacy_click_event("");
        assert_eq!(events.bindings.len(), 1);
        assert_eq!(events.bindings[0].event, Some(UiEventType::Click));
        assert_eq!(events.bindings[0].function, "on_click");
    }

    #[test]
    fn parses_and_serializes_arguments() {
        let events = UiEvents::parse(
            "event=pointer_down;function=on_press;arg.speed=2|event=click;target=Menu;function=go_menu",
        )
        .unwrap();
        assert_eq!(events.bindings.len(), 2);
        assert_eq!(events.bindings[0].arguments["speed"], "2");
        assert_eq!(events.bindings[1].target.as_deref(), Some("Menu"));
        let text = events.to_component_str();
        assert_eq!(UiEvents::parse(&text).unwrap(), events);
    }

    #[test]
    fn validates_missing_targets_and_functions() {
        let mut entities = std::collections::BTreeSet::new();
        entities.insert("Button".to_owned());
        let mut functions = std::collections::BTreeSet::new();
        functions.insert("on_click".to_owned());
        let events = UiEvents {
            bindings: vec![UiEventBinding {
                event: Some(UiEventType::Click),
                target: Some("Missing".to_owned()),
                function: "missing_function".to_owned(),
                arguments: BTreeMap::new(),
            }],
        };
        let diagnostics = events.validate(&entities, &functions);
        assert_eq!(diagnostics.len(), 2);
    }
}
