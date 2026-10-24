//! E23: minimal one-way ViewModel property bindings.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BindingFlow {
    #[default]
    OneWay,
    /// Target edits are pushed back to the source through [`BindingRuntime::push`].
    TwoWay,
}

impl BindingFlow {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "one_way" => Some(Self::OneWay),
            "two_way" => Some(Self::TwoWay),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneWay => "one_way",
            Self::TwoWay => "two_way",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyBinding {
    pub property: Option<BoundProperty>,
    pub target: String,
    pub source: BindingPath,
    pub default: Option<String>,
    pub flow: BindingFlow,
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
            let mut flow = BindingFlow::OneWay;
            for field in item.split(';') {
                let Some((key, value)) = field.split_once('=') else {
                    return Err(BindingError::Malformed(field.to_owned()));
                };
                match key {
                    "property" => property = BoundProperty::parse(value),
                    "target" => target = value.to_owned(),
                    "source" => source = Some(BindingPath::parse(value)?),
                    "default" => default = Some(value.to_owned()),
                    "mode" => {
                        flow = BindingFlow::parse(value)
                            .ok_or_else(|| BindingError::UnknownFlow(value.to_owned()))?
                    }
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
                flow,
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
                if binding.flow == BindingFlow::TwoWay {
                    fields.push(format!("mode={}", binding.flow.as_str()));
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

/// 运行期组件形态：实体上挂的 `UiBindings` 串解析结果。
///
/// 与 `UiEvents` 一样，组件串是**多条记录**；这里只做包装，解析/编码仍由
/// [`BindingSet`] 一处定义，编辑器与运行时因此不会各写一套格式。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiBindings(pub BindingSet);

impl UiBindings {
    pub fn parse(raw: &str) -> Result<Self, BindingError> {
        BindingSet::parse(raw).map(Self)
    }

    pub fn from_component_str(raw: &str) -> Result<Self, BindingError> {
        Self::parse(raw)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        self.0.to_component_str()
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

/// Per-frame binding driver. `resolve` is pure; this wrapper only emits the
/// targets that actually changed and pushes two-way edits back to the source.
#[derive(Clone, Debug, Default)]
pub struct BindingRuntime {
    last: BTreeMap<(String, BoundProperty), BindingValue>,
}

impl BindingRuntime {
    /// Resolves every binding, returning only changed outcomes. Missing sources
    /// are skipped (the previous target value stays on screen).
    pub fn update(
        &mut self,
        bindings: &BindingSet,
        view_model: &BTreeMap<String, BindingValue>,
    ) -> Vec<BindingOutcome> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (binding, result) in bindings.bindings.iter().zip(bindings.resolve(view_model)) {
            let Ok(outcome) = result else {
                continue;
            };
            let property = binding.property.unwrap_or(BoundProperty::TextText);
            let key = (outcome.target.clone(), property);
            seen.insert(key.clone());
            if self.last.get(&key) != Some(&outcome.value) {
                self.last.insert(key, outcome.value.clone());
                out.push(outcome);
            }
        }
        self.last.retain(|key, _| seen.contains(key));
        out
    }

    /// Two-way edit from the target: writes the value back to the source path.
    /// One-way bindings reject the write instead of silently doing nothing.
    pub fn push(
        &mut self,
        bindings: &BindingSet,
        target: &str,
        property: BoundProperty,
        value: BindingValue,
        view_model: &mut BTreeMap<String, BindingValue>,
    ) -> Result<(), BindingError> {
        let binding = bindings
            .bindings
            .iter()
            .find(|binding| {
                binding.target == target
                    && binding.property.unwrap_or(BoundProperty::TextText) == property
            })
            .ok_or_else(|| BindingError::BindingNotFound(target.to_owned()))?;
        if binding.flow != BindingFlow::TwoWay {
            return Err(BindingError::NotTwoWay(target.to_owned()));
        }
        view_model.insert(binding.source.0.join("."), value);
        self.last.remove(&(target.to_owned(), property));
        Ok(())
    }

    #[must_use]
    pub fn last_value(&self, target: &str, property: BoundProperty) -> Option<&BindingValue> {
        self.last.get(&(target.to_owned(), property))
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
    #[error("unknown binding mode: {0}")]
    UnknownFlow(String),
    #[error("binding for target {0} was not found")]
    BindingNotFound(String),
    #[error("binding for target {0} is one-way and cannot write back")]
    NotTwoWay(String),
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
    fn runtime_emits_only_changed_targets_and_tracks_removal() {
        let bindings = BindingSet::parse(
            "property=text.text;target=Score;source=data.score|property=progress.value;target=Bar;source=data.progress",
        )
        .unwrap();
        let mut view_model = BTreeMap::new();
        view_model.insert("data.score".to_owned(), BindingValue::Number(1.0));
        view_model.insert("data.progress".to_owned(), BindingValue::Number(0.25));
        let mut runtime = BindingRuntime::default();
        assert_eq!(runtime.update(&bindings, &view_model).len(), 2);
        // Same view model next frame: nothing is written to the target.
        assert!(runtime.update(&bindings, &view_model).is_empty());
        view_model.insert("data.score".to_owned(), BindingValue::Number(2.0));
        let changed = runtime.update(&bindings, &view_model);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].target, "Score");
        assert_eq!(
            runtime.last_value("Score", BoundProperty::TextText),
            Some(&BindingValue::Text("2".to_owned()))
        );

        let only_bar =
            BindingSet::parse("property=progress.value;target=Bar;source=data.progress").unwrap();
        runtime.update(&only_bar, &view_model);
        assert!(
            runtime
                .last_value("Score", BoundProperty::TextText)
                .is_none()
        );
    }

    #[test]
    fn two_way_binding_writes_back_and_one_way_rejects() {
        let bindings = BindingSet::parse(
            "property=text.text;target=Input;source=data.name;mode=two_way|property=text.text;target=Label;source=data.name",
        )
        .unwrap();
        assert_eq!(bindings.bindings[0].flow, BindingFlow::TwoWay);
        assert_eq!(bindings.bindings[1].flow, BindingFlow::OneWay);
        assert!(bindings.to_component_str().contains("mode=two_way"));
        assert_eq!(
            BindingSet::parse(&bindings.to_component_str()).unwrap(),
            bindings
        );

        let mut view_model = BTreeMap::new();
        view_model.insert("data.name".to_owned(), BindingValue::Text("Ann".to_owned()));
        let mut runtime = BindingRuntime::default();
        runtime.update(&bindings, &view_model);
        runtime
            .push(
                &bindings,
                "Input",
                BoundProperty::TextText,
                BindingValue::Text("Bob".to_owned()),
                &mut view_model,
            )
            .unwrap();
        assert_eq!(
            view_model["data.name"],
            BindingValue::Text("Bob".to_owned())
        );
        assert_eq!(runtime.update(&bindings, &view_model).len(), 2);
        assert!(matches!(
            runtime.push(
                &bindings,
                "Label",
                BoundProperty::TextText,
                BindingValue::Text("x".to_owned()),
                &mut view_model,
            ),
            Err(BindingError::NotTwoWay(_))
        ));
        assert!(matches!(
            BindingSet::parse("property=text.text;target=X;source=a;mode=sideways"),
            Err(BindingError::UnknownFlow(_))
        ));
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
