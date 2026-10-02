//! E23: minimal one-way ViewModel property bindings.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundProperty {
    TextText,
    ImageVisible,
    ProgressValue,
}

impl BoundProperty {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "text.text" => Some(Self::TextText),
            "image.visible" => Some(Self::ImageVisible),
            "progress.value" => Some(Self::ProgressValue),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TextText => "text.text",
            Self::ImageVisible => "image.visible",
            Self::ProgressValue => "progress.value",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BindingPath(pub Vec<String>);

impl BindingPath {
    pub fn parse(raw: &str) -> Result<Self, BindingError> {
        let segments: Vec<String> = raw
            .split('.')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        if segments.is_empty() {
            return Err(BindingError::EmptyPath);
        }
        Ok(Self(segments))
    }

    fn get<'a>(&self, view_model: &'a BTreeMap<String, BindingValue>) -> Option<&'a BindingValue> {
        view_model.get(&self.0.join("."))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyBinding {
    pub property: Option<BoundProperty>,
    pub target: String,
    pub source: BindingPath,
    pub default: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindingSet {
    pub bindings: Vec<PropertyBinding>,
}

impl BindingSet {
    pub fn parse(raw: &str) -> Result<Self, BindingError> {
        let mut bindings = Vec::new();
        for item in raw.split('|') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let mut property = None;
            let mut target = String::new();
            let mut source = None;
            let mut default = None;
            for field in item.split(';') {
                let Some((key, value)) = field.split_once('=') else {
                    return Err(BindingError::Malformed(field.to_owned()));
                };
                match key {
                    "property" => property = BoundProperty::parse(value),
                    "target" => target = value.to_owned(),
                    "source" => source = Some(BindingPath::parse(value)?),
                    "default" => default = Some(value.to_owned()),
                    _ => return Err(BindingError::UnknownField(key.to_owned())),
                }
            }
            if target.is_empty() || property.is_none() {
                return Err(BindingError::MissingTargetOrProperty);
            }
            bindings.push(PropertyBinding {
                property,
                target,
                source: source.ok_or(BindingError::MissingSource)?,
                default,
            });
        }
        Ok(Self { bindings })
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        self.bindings
            .iter()
            .map(|binding| {
                let mut fields = vec![format!(
                    "property={}",
                    binding.property.map_or("text.text", BoundProperty::as_str)
                )];
                fields.push(format!("target={}", binding.target));
                fields.push(format!("source={}", binding.source.0.join(".")));
                if let Some(default) = &binding.default {
                    fields.push(format!("default={default}"));
                }
                fields.join(";")
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    /// Resolves one-way bindings and converts values by target property.
    /// Errors are diagnostics, not panics; unrelated bindings keep updating.
    #[must_use]
    pub fn resolve(
        &self,
        view_model: &BTreeMap<String, BindingValue>,
    ) -> Vec<Result<BindingOutcome, BindingError>> {
        self.bindings
            .iter()
            .map(|binding| {
                let property = binding.property.unwrap_or(BoundProperty::TextText);
                let value = binding.source.get(view_model).cloned().or_else(|| {
                    binding.default.as_ref().map(|text| match property {
                        BoundProperty::ProgressValue => {
                            BindingValue::Number(text.trim().parse().unwrap_or(0.0))
                        }
                        BoundProperty::ImageVisible => {
                            BindingValue::Bool(text.trim().parse().unwrap_or(true))
                        }
                        _ => BindingValue::Text(text.clone()),
                    })
                });
                let Some(value) = value else {
                    return Err(BindingError::MissingValue(binding.source.0.join(".")));
                };
                Ok(BindingOutcome {
                    property,
                    target: binding.target.clone(),
                    value: convert_value(property, &value),
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BindingValue {
    Text(String),
    Number(f32),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BindingOutcome {
    pub property: BoundProperty,
    pub target: String,
    pub value: BindingValue,
}

fn convert_value(property: BoundProperty, value: &BindingValue) -> BindingValue {
    match property {
        BoundProperty::TextText => match value {
            BindingValue::Text(text) => BindingValue::Text(text.clone()),
            BindingValue::Number(number) => BindingValue::Text(format!("{number}")),
            BindingValue::Bool(enabled) => BindingValue::Text(enabled.to_string()),
        },
        BoundProperty::ImageVisible => BindingValue::Bool(match value {
            BindingValue::Bool(enabled) => *enabled,
            BindingValue::Number(number) => *number != 0.0,
            BindingValue::Text(text) => text.trim().parse().unwrap_or(true),
        }),
        BoundProperty::ProgressValue => BindingValue::Number(match value {
            BindingValue::Number(number) => *number,
            BindingValue::Bool(enabled) => f32::from(*enabled),
            BindingValue::Text(text) => text.trim().parse().unwrap_or(0.0),
        }),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BindingError {
    #[error("binding path is empty")]
    EmptyPath,
    #[error("malformed binding: {0}")]
    Malformed(String),
    #[error("unknown binding field: {0}")]
    UnknownField(String),
    #[error("binding needs target and property")]
    MissingTargetOrProperty,
    #[error("binding source is required")]
    MissingSource,
    #[error("binding value {0} was not found")]
    MissingValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips_binding_component() {
        let bindings = BindingSet::parse(
            "property=text.text;target=ScoreLabel;source=data.score|property=progress.value;target=Bar;source=data.progress",
        )
        .unwrap();
        assert_eq!(bindings.bindings.len(), 2);
        assert_eq!(
            BindingSet::parse(&bindings.to_component_str()).unwrap(),
            bindings
        );
    }

    #[test]
    fn resolves_and_converts_values() {
        let mut view_model = BTreeMap::new();
        view_model.insert("data.score".to_owned(), BindingValue::Number(12.5));
        view_model.insert("data.progress".to_owned(), BindingValue::Bool(true));
        let bindings = BindingSet::parse(
            "property=text.text;target=Score;source=data.score|property=progress.value;target=Bar;source=data.progress",
        )
        .unwrap();
        let outcomes = bindings.resolve(&view_model);
        assert!(outcomes.iter().all(Result::is_ok));
        assert_eq!(
            outcomes[0].as_ref().unwrap().value,
            BindingValue::Text("12.5".to_owned())
        );
        assert_eq!(
            outcomes[1].as_ref().unwrap().value,
            BindingValue::Number(1.0)
        );
    }

    #[test]
    fn reports_missing_path_without_panicking() {
        let bindings =
            BindingSet::parse("property=text.text;target=Label;source=data.missing").unwrap();
        let result = bindings.resolve(&BTreeMap::new());
        assert_eq!(
            result[0].as_ref().unwrap_err(),
            &BindingError::MissingValue("data.missing".to_owned())
        );
    }
}
