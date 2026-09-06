//! E21: typed schemas for high-value scene components encoded as
//! `BTreeMap<String, String>`.

use std::{collections::BTreeMap, fmt};

/// A validated component instance decoded from a serialized component string.
pub type ComponentData = BTreeMap<String, FieldValue>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldKind {
    Bool,
    Int,
    Number,
    Text,
    Vec2,
    Vec3,
    Vec4,
    Enum,
}

impl FieldKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Number => "number",
            Self::Text => "string",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Vec4 => "vec4",
            Self::Enum => "enum",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    Number(f32),
    Text(String),
    Tuple(Vec<f32>),
    Choice(String),
}

impl FieldValue {
    #[must_use]
    pub fn to_component_value(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Number(value) => format!("{value:.6}"),
            Self::Text(value) => value.clone(),
            Self::Tuple(values) => values
                .iter()
                .map(|value| format!("{value:.6}"))
                .collect::<Vec<_>>()
                .join(","),
            Self::Choice(value) => value.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldSchema {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
    pub choices: &'static [&'static str],
}

impl FieldSchema {
    #[must_use]
    pub const fn new(name: &'static str, kind: FieldKind, required: bool) -> Self {
        Self {
            name,
            kind,
            required,
            choices: &[],
        }
    }

    #[must_use]
    pub const fn with_choices(mut self, choices: &'static [&'static str]) -> Self {
        self.choices = choices;
        self
    }

    fn validate_raw(&self, raw: &str, component: &str) -> Result<FieldValue, SchemaError> {
        let fail = |reason: String| SchemaError {
            component: component.to_owned(),
            field: self.name.to_owned(),
            reason,
        };
        let parse_number = |value: &str| -> Result<f32, SchemaError> {
            value
                .trim()
                .parse::<f32>()
                .map_err(|_| fail(format!("invalid {value:?}; expected number")))
                .and_then(|parsed| {
                    parsed
                        .is_finite()
                        .then_some(parsed)
                        .ok_or_else(|| fail(format!("invalid {value:?}; number must be finite")))
                })
        };
        match self.kind {
            FieldKind::Bool => match raw.trim() {
                "true" | "1" => Ok(FieldValue::Bool(true)),
                "false" | "0" => Ok(FieldValue::Bool(false)),
                value => Err(fail(format!("invalid {value:?}; expected bool"))),
            },
            FieldKind::Int => raw
                .trim()
                .parse::<i64>()
                .map(FieldValue::Int)
                .map_err(|_| fail(format!("invalid {raw:?}; expected integer"))),
            FieldKind::Number => parse_number(raw).map(FieldValue::Number),
            FieldKind::Text => Ok(FieldValue::Text(raw.to_owned())),
            FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 => {
                let expected = match self.kind {
                    FieldKind::Vec2 => 2,
                    FieldKind::Vec3 => 3,
                    _ => 4,
                };
                let parts: Vec<&str> = raw.split(',').collect();
                if parts.len() != expected {
                    return Err(fail(format!(
                        "expected {expected} comma-separated numbers, found {}",
                        parts.len()
                    )));
                }
                let mut values = Vec::with_capacity(expected);
                for part in parts {
                    values.push(parse_number(part)?);
                }
                Ok(FieldValue::Tuple(values))
            }
            FieldKind::Enum => {
                let normalized = raw.trim();
                if self.choices.contains(&normalized) {
                    Ok(FieldValue::Choice(normalized.to_owned()))
                } else {
                    Err(fail(format!(
                        "invalid {normalized:?}; expected one of {:?}",
                        self.choices
                    )))
                }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentSchema {
    pub name: &'static str,
    pub version: u32,
    pub fields: &'static [FieldSchema],
}

impl ComponentSchema {
    #[must_use]
    pub const fn new(name: &'static str, version: u32, fields: &'static [FieldSchema]) -> Self {
        Self {
            name,
            version,
            fields,
        }
    }

    /// Parses and validates a component payload. Missing optional fields remain
    /// absent, so round-trips do not add synthetic defaults.
    pub fn parse(&self, raw: &str) -> Result<ComponentData, SchemaError> {
        let mut data = ComponentData::new();
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(SchemaError {
                    component: self.name.to_owned(),
                    field: field.to_owned(),
                    reason: "component field must be key=value".to_owned(),
                });
            };
            let schema = self
                .fields
                .iter()
                .find(|schema| schema.name == key)
                .ok_or_else(|| SchemaError {
                    component: self.name.to_owned(),
                    field: key.to_owned(),
                    reason: format!("unknown field {key:?}"),
                })?;
            data.insert(key.to_owned(), schema.validate_raw(value, self.name)?);
        }
        for schema in self.fields {
            if schema.required && !data.contains_key(schema.name) {
                return Err(SchemaError {
                    component: self.name.to_owned(),
                    field: schema.name.to_owned(),
                    reason: "required field is missing".to_owned(),
                });
            }
        }
        Ok(data)
    }

    #[must_use]
    pub fn serialize(data: &ComponentData) -> String {
        data.iter()
            .map(|(key, value)| format!("{key}={}", value.to_component_value()))
            .collect::<Vec<_>>()
            .join(";")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaError {
    pub component: String,
    pub field: String,
    pub reason: String,
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "component {} field {}: {}",
            self.component, self.field, self.reason
        )
    }
}

impl std::error::Error for SchemaError {}

const TRANSFORM_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("x", FieldKind::Number, true),
    FieldSchema::new("y", FieldKind::Number, true),
    FieldSchema::new("rotation", FieldKind::Number, true),
    FieldSchema::new("scale_x", FieldKind::Number, true),
    FieldSchema::new("scale_y", FieldKind::Number, true),
];

const TEXT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("text", FieldKind::Text, true),
    FieldSchema::new("font_size", FieldKind::Number, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
];

const IMAGE_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("texture", FieldKind::Text, true),
    FieldSchema::new("color", FieldKind::Vec3, false),
    FieldSchema::new("nine_slice", FieldKind::Vec4, false),
];

const UI_LAYOUT_FIELDS: &[FieldSchema] = &[
    FieldSchema::new("anchors", FieldKind::Vec4, false),
    FieldSchema::new("offsets", FieldKind::Vec4, false),
    FieldSchema::new("min_size", FieldKind::Vec2, false),
    FieldSchema::new("max_size", FieldKind::Vec2, false),
    FieldSchema::new("pivot", FieldKind::Vec2, false),
];

/// First-phase built-in schemas. Future structured components should be added
/// here instead of growing ad hoc parsing paths.
#[must_use]
pub fn builtin_registry() -> BTreeMap<&'static str, ComponentSchema> {
    let schemas = [
        ComponentSchema::new("Transform", 1, TRANSFORM_FIELDS),
        ComponentSchema::new("Text", 2, TEXT_FIELDS),
        ComponentSchema::new("Image", 2, IMAGE_FIELDS),
        ComponentSchema::new("UiLayout", 1, UI_LAYOUT_FIELDS),
    ];
    schemas
        .into_iter()
        .map(|schema| (schema.name, schema))
        .collect()
}

/// Validates a component without materializing its values.
pub fn validate_component(
    registry: &BTreeMap<&'static str, ComponentSchema>,
    name: &str,
    raw: &str,
) -> Result<(), SchemaError> {
    registry
        .get(name)
        .ok_or_else(|| SchemaError {
            component: name.to_owned(),
            field: "<component>".to_owned(),
            reason: "no schema is registered".to_owned(),
        })
        .and_then(|schema| schema.parse(raw).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transform_and_round_trips() {
        let registry = builtin_registry();
        let data = registry["Transform"]
            .parse("x=1;y=-2;rotation=0;scale_x=2;scale_y=3")
            .unwrap();
        assert_eq!(data["x"], FieldValue::Number(1.0));
        assert_eq!(data["scale_y"], FieldValue::Number(3.0));
        assert!(!ComponentSchema::serialize(&data).is_empty());
    }

    #[test]
    fn errors_include_component_and_field_path() {
        let registry = builtin_registry();
        let error = registry["Transform"]
            .parse("x=bad;y=0;rotation=0;scale_x=1;scale_y=1")
            .unwrap_err();
        assert_eq!(error.component, "Transform");
        assert_eq!(error.field, "x");
        assert!(error.to_string().contains("component Transform field x"));
    }

    #[test]
    fn validates_builtin_components() {
        let registry = builtin_registry();
        assert!(validate_component(&registry, "Unknown", "x=1").is_err());
        assert!(
            registry["UiLayout"]
                .parse("anchors=0,0,1,1;offsets=0,0,0,0")
                .is_ok()
        );
    }

    /// 回归：Text/Image schema 必须与新组件解析器（ui.rs）字段一致——
    /// 视觉属性 + color/nine_slice；旧内联几何（x/y/scale/width/height）
    /// 已随 UiLayout 迁移删除，schema 白名单不得再接受。
    #[test]
    fn text_and_image_schemas_match_current_visual_only_format() {
        let registry = builtin_registry();
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;color=1,0.5,0")
                .is_ok()
        );
        assert!(registry["Text"].parse("text=Hello;font_size=24").is_ok());
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;x=1;y=2;scale=1")
                .is_err()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;color=1,1,1;nine_slice=8,8,8,8")
                .is_ok()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;width=64;height=32")
                .is_err()
        );
        // 字段类型：color 必须 3 个数，nine_slice 必须 4 个数。
        assert!(
            registry["Text"]
                .parse("text=Hello;font_size=24;color=1,1")
                .is_err()
        );
        assert!(
            registry["Image"]
                .parse("texture=ui/button.png;nine_slice=8,8,8")
                .is_err()
        );
    }
}
