//! Typed property paths over component strings: `Component.field`, `Component.tuple[index]`.
//!
//! docs/55 asks for "统一 Schema/PropertyPath" so that Inspector field edits, Prefab per-field
//! overrides and future diffs all address the same thing. This module provides that address
//! space plus **minimal-churn** writes:
//!
//! * values are validated through [`crate::component_schema`], so a typo cannot land in a
//!   scene file;
//! * only the addressed field's text is rewritten — every other field keeps its exact bytes.
//!   Writing a value that is already present therefore returns the payload unchanged, which is
//!   what keeps Prefab overrides and dirty-diffs from churning the whole component string.

use crate::component_schema::ComponentData;
use crate::component_schema::ComponentSchema;
use crate::component_schema::FieldKind;
use crate::component_schema::FieldValue;
use crate::component_schema::SchemaError;
use crate::component_schema::builtin_registry;
use std::{collections::BTreeMap, fmt};

/// `Component.field` or `Component.field[index]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyPath {
    pub component: String,
    pub field: String,
    /// Tuple component index (`Sprite.tint[2]`), when the path addresses one element.
    pub index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PropertyPathError {
    /// The text is not `Component.field` / `Component.field[index]`.
    #[error("不是合法的属性路径：{0:?}（应形如 Sprite.pivot）")]
    Malformed(String),
    /// The tuple index is missing, empty or not a number.
    #[error("非法的元组下标：{0:?}")]
    InvalidIndex(String),
}

impl PropertyPath {
    #[must_use]
    pub fn new(component: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            field: field.into(),
            index: None,
        }
    }

    #[must_use]
    pub fn with_index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    /// Parse `Sprite.pivot` / `Sprite.tint[2]` / `ui.root`（组件名可含点：最后一个点分段）。
    pub fn parse(text: &str) -> Result<Self, PropertyPathError> {
        let text = text.trim();
        let (head, index) = match text.rsplit_once('[') {
            Some((head, tail)) => {
                let digits = tail
                    .strip_suffix(']')
                    .ok_or_else(|| PropertyPathError::InvalidIndex(text.to_owned()))?;
                let index: usize = digits
                    .trim()
                    .parse()
                    .map_err(|_| PropertyPathError::InvalidIndex(text.to_owned()))?;
                (head, Some(index))
            }
            None => (text, None),
        };
        let (component, field) = head
            .rsplit_once('.')
            .ok_or_else(|| PropertyPathError::Malformed(text.to_owned()))?;
        if component.trim().is_empty() || field.trim().is_empty() {
            return Err(PropertyPathError::Malformed(text.to_owned()));
        }
        Ok(Self {
            component: component.trim().to_owned(),
            field: field.trim().to_owned(),
            index,
        })
    }
}

impl fmt::Display for PropertyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.index {
            Some(index) => write!(f, "{}.{}[{index}]", self.component, self.field),
            None => write!(f, "{}.{}", self.component, self.field),
        }
    }
}

/// What went wrong while reading/writing one property.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PropertyError {
    #[error("组件 {0} 没有注册 schema")]
    NoSchema(String),
    #[error("组件 {component} 没有字段 {field}")]
    UnknownField { component: String, field: String },
    #[error("字段 {field} 只有 {len} 个分量，下标 {index} 越界")]
    UnknownIndex {
        field: String,
        index: usize,
        len: usize,
    },
    #[error("字段 {field} 取值非法：{reason}")]
    InvalidValue { field: String, reason: String },
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

/// Split a component payload into `(key, raw value)` pairs, preserving order and exact text.
fn split_fields(payload: &str) -> Vec<(&str, &str)> {
    payload
        .split(';')
        .filter_map(|segment| {
            let segment = segment.trim();
            if segment.is_empty() {
                return None;
            }
            segment
                .split_once('=')
                .map(|(key, value)| (key.trim(), value))
        })
        .collect()
}

/// 与 [`split_fields`] 相同，但**尊重 `UiText` 的转义**（`text=a\;b` 里的 `\;` 不是分隔符）。
///
/// 只在组件**有自由文本字段**（`FieldSchema::escaped`）时使用：其它组件的引擎解析器是朴素
/// 按 `;` 切分的，跟着转义会让 schema 比运行时更松。
fn split_fields_escaped(payload: &str) -> Vec<(&str, &str)> {
    crate::component_schema::split_fields(payload)
        .into_iter()
        .filter_map(|segment| {
            let segment = segment.trim();
            if segment.is_empty() {
                return None;
            }
            segment
                .split_once('=')
                .map(|(key, value)| (key.trim(), value))
        })
        .collect()
}

/// 按组件是否含自由文本字段选择切分口径。
fn split_for_schema<'a>(schema: &ComponentSchema, payload: &'a str) -> Vec<(&'a str, &'a str)> {
    if schema.has_escaped_fields() {
        split_fields_escaped(payload)
    } else {
        split_fields(payload)
    }
}

/// Read one field (or one tuple element) as a typed value.
pub fn read_field(
    registry: &BTreeMap<&'static str, ComponentSchema>,
    payload: &str,
    path: &PropertyPath,
) -> Result<FieldValue, PropertyError> {
    let schema = registry
        .get(path.component.as_str())
        .ok_or_else(|| PropertyError::NoSchema(path.component.clone()))?;
    let data: ComponentData = schema.parse(payload)?;
    let value = data
        .get(&path.field)
        .ok_or_else(|| PropertyError::UnknownField {
            component: path.component.clone(),
            field: path.field.clone(),
        })?
        .clone();
    match (path.index, value) {
        (None, value) => Ok(value),
        (Some(index), FieldValue::Tuple(values)) => values
            .get(index)
            .map(|value| FieldValue::Number(*value))
            .ok_or_else(|| PropertyError::UnknownIndex {
                field: path.field.clone(),
                index,
                len: values.len(),
            }),
        (Some(_), other) => Err(PropertyError::InvalidValue {
            field: path.field.clone(),
            reason: format!("{} 不是元组，无法按下标取值", other.to_component_value()),
        }),
    }
}

/// Write one field (or one tuple element) with **minimal churn**.
///
/// Returns the payload unchanged when the addressed value already equals `raw_value`
/// (after typed normalization), so round-tripping an override is a no-op.
pub fn write_field(
    registry: &BTreeMap<&'static str, ComponentSchema>,
    payload: &str,
    path: &PropertyPath,
    raw_value: &str,
) -> Result<String, PropertyError> {
    let schema = registry
        .get(path.component.as_str())
        .ok_or_else(|| PropertyError::NoSchema(path.component.clone()))?;
    let field_schema = schema
        .fields
        .iter()
        .find(|field| field.name == path.field)
        .ok_or_else(|| PropertyError::UnknownField {
            component: path.component.clone(),
            field: path.field.clone(),
        })?;

    // 当前值（用于"无变化则原样返回"）。
    let current = schema.parse(payload)?;
    let current_value = current.get(&path.field).cloned();

    // 自由文本字段：编辑器的输入是**人看到的文本**，而组件串里是转义形式
    // （`read_field_display_text` 已反转义）。这里统一按"入参是转义文本"处理：
    // 先反转义再走校验/比较/写回，于是 `read_field_text`（保留原样）与编辑器输入两条路径
    // 都得到同一个结果，也不会二次转义。
    let incoming: String = if field_schema.escaped && path.index.is_none() {
        crate::component_schema::unescape_text(raw_value)
    } else {
        raw_value.to_owned()
    };

    let updated_raw = match path.index {
        None => {
            // 只校验目标字段本身：写一个字段时其它必填字段不该参与判定。
            let new_value = field_schema
                .validate(&incoming, &path.component)
                .map_err(|error| PropertyError::InvalidValue {
                    field: path.field.clone(),
                    reason: error.reason,
                })?;
            if current_value.as_ref() == Some(&new_value) {
                return Ok(payload.to_owned());
            }
            let raw = new_value.to_component_value();
            if field_schema.escaped {
                crate::component_schema::escape_text(&raw)
            } else {
                raw
            }
        }
        Some(index) => {
            let Some(FieldValue::Tuple(values)) = current_value.clone() else {
                return Err(PropertyError::InvalidValue {
                    field: path.field.clone(),
                    reason: "字段不是元组，无法按下标写入".to_owned(),
                });
            };
            if index >= values.len() {
                return Err(PropertyError::UnknownIndex {
                    field: path.field.clone(),
                    index,
                    len: values.len(),
                });
            }
            let parsed: f32 =
                raw_value
                    .trim()
                    .parse()
                    .map_err(|_| PropertyError::InvalidValue {
                        field: path.field.clone(),
                        reason: format!("{raw_value:?} 不是数字"),
                    })?;
            if !parsed.is_finite() {
                return Err(PropertyError::InvalidValue {
                    field: path.field.clone(),
                    reason: "数值必须有限".to_owned(),
                });
            }
            if values[index] == parsed {
                return Ok(payload.to_owned());
            }
            // 只替换该分量的文本，其余分量保持原样。
            let raw_payload = split_fields(payload)
                .into_iter()
                .find(|(key, _)| *key == path.field)
                .map(|(_, value)| value.to_owned())
                .unwrap_or_else(|| {
                    values
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                });
            let mut parts: Vec<String> = raw_payload
                .split(',')
                .map(|part| part.trim().to_owned())
                .collect();
            parts[index] = raw_value.trim().to_owned();
            parts.join(",")
        }
    };

    // 重建负载：目标字段就地替换，其余字段逐字保留；缺失则追加到末尾。
    let mut segments: Vec<String> = Vec::new();
    let mut replaced = false;
    for (key, value) in split_for_schema(schema, payload) {
        if key == path.field {
            segments.push(format!("{}={updated_raw}", path.field));
            replaced = true;
        } else {
            segments.push(format!("{key}={value}"));
        }
    }
    if !replaced {
        segments.push(format!("{}={updated_raw}", path.field));
    }
    let result = segments.join(";");
    // 写入后必须仍能被 schema 接受（防止拼接出非法负载）。
    schema.parse(&result)?;
    Ok(result)
}

/// Read the **raw text** of a field (or of one tuple element), preserving formatting.
///
/// Used when recording a Prefab override: "keep this value" must not rewrite `77` into
/// `77.000000`, otherwise re-applying a Prefab would churn unrelated characters.
///
/// 注意：**自由文本字段**在这里返回的是组件串里的**转义原文**（`a\;b`）。要显示给人看
/// 请用 [`read_field_display_text`]；`write_field` 会把入参当转义文本处理，因此
/// "`read_field_text` → `write_field`"这条 Prefab 回退路径仍然是原样往返。
pub fn read_field_text(payload: &str, path: &PropertyPath) -> Result<String, PropertyError> {
    let registry = builtin_registry();
    let split = splitter_for(registry.get(path.component.as_str()));
    read_field_text_with(split, payload, path)
}

/// 读取字段的**显示文本**：自由文本字段（`FieldSchema::escaped`）先反转义，其它字段与
/// [`read_field_text`] 相同。
///
/// 编辑器的输入框用它：人在框里看到的、以及编辑后提交的，都是**真实文本**（不含 `\;` 这类
/// 转义），转义只在组件串这一层存在（`ComponentSchema` 负责两个方向的换算）。
pub fn read_field_display_text(
    registry: &BTreeMap<&'static str, ComponentSchema>,
    payload: &str,
    path: &PropertyPath,
) -> Result<String, PropertyError> {
    let schema = registry.get(path.component.as_str());
    let raw = read_field_text_with(splitter_for(schema), payload, path)?;
    let escaped = schema
        .and_then(|schema| schema.fields.iter().find(|field| field.name == path.field))
        .is_some_and(|field| field.escaped);
    if escaped && path.index.is_none() {
        Ok(crate::component_schema::unescape_text(&raw))
    } else {
        Ok(raw)
    }
}

/// 该组件该用哪种切分：有自由文本字段就尊重转义，否则与它自己的引擎解析器口径一致。
fn splitter_for(
    schema: Option<&ComponentSchema>,
) -> for<'a> fn(&'a str) -> Vec<(&'a str, &'a str)> {
    if schema.is_some_and(ComponentSchema::has_escaped_fields) {
        split_fields_escaped
    } else {
        split_fields
    }
}

fn read_field_text_with(
    split: for<'a> fn(&'a str) -> Vec<(&'a str, &'a str)>,
    payload: &str,
    path: &PropertyPath,
) -> Result<String, PropertyError> {
    let raw = split(payload)
        .into_iter()
        .find(|(key, _)| *key == path.field)
        .map(|(_, value)| value.trim().to_owned())
        .ok_or_else(|| PropertyError::UnknownField {
            component: path.component.clone(),
            field: path.field.clone(),
        })?;
    match path.index {
        None => Ok(raw),
        Some(index) => {
            // `宽x高`（Size）与逗号元组两种编码都要支持。
            let separator = if raw.contains('x') && !raw.contains(',') {
                'x'
            } else {
                ','
            };
            let parts: Vec<&str> = raw.split(separator).map(str::trim).collect();
            parts
                .get(index)
                .map(|value| (*value).to_owned())
                .ok_or_else(|| PropertyError::UnknownIndex {
                    field: path.field.clone(),
                    index,
                    len: parts.len(),
                })
        }
    }
}

/// Read a property using the built-in registry.
pub fn read_builtin(payload: &str, path: &PropertyPath) -> Result<FieldValue, PropertyError> {
    read_field(&builtin_registry(), payload, path)
}

/// Write a property using the built-in registry.
pub fn write_builtin(
    payload: &str,
    path: &PropertyPath,
    raw_value: &str,
) -> Result<String, PropertyError> {
    write_field(&builtin_registry(), payload, path, raw_value)
}

/// Typed equality for two raw field texts: `1.0` equals `1.000000`, `true` equals `1`.
///
/// Without this, a value that differs only in formatting would show up as a Prefab override.
#[must_use]
pub fn field_text_equal(component: &str, field: &str, left: &str, right: &str) -> bool {
    let registry = builtin_registry();
    let Some(schema) = registry.get(component) else {
        return left == right;
    };
    let Some(field_schema) = schema.fields.iter().find(|schema| schema.name == field) else {
        return left == right;
    };
    // 元组字段按下标逐分量比较，避免 `1.0,2` 与 `1,2.0` 被当成不同。
    // `Color` 也在这一族里：漏了它 `Image.color` 会退化成字符串比较，作者把 `1,1,1` 写成
    // `1.0,1.0,1.0` 就会在撤销/脏标记里记成一次真实改动。
    if matches!(
        field_schema.kind,
        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 | FieldKind::Color | FieldKind::Size
    ) {
        let separator = if field_schema.kind == FieldKind::Size {
            'x'
        } else {
            ','
        };
        let left_parts: Vec<&str> = left.split(separator).collect();
        let right_parts: Vec<&str> = right.split(separator).collect();
        if left_parts.len() != right_parts.len() {
            return false;
        }
        return left_parts
            .iter()
            .zip(right_parts.iter())
            .all(|(left, right)| numbers_equal(left, right));
    }
    match field_schema.kind {
        FieldKind::Bool => parse_bool(left) == parse_bool(right),
        FieldKind::Int => parse_i64(left) == parse_i64(right),
        FieldKind::Number => numbers_equal(left, right),
        _ => left.trim() == right.trim(),
    }
}

fn numbers_equal(left: &str, right: &str) -> bool {
    match (left.trim().parse::<f32>(), right.trim().parse::<f32>()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left.trim() == right.trim(),
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_i64(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i64::from_str_radix(rest, 16).ok();
    }
    if let Some(rest) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        return i64::from_str_radix(rest, 2).ok();
    }
    text.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_paths() {
        let path = PropertyPath::parse("Sprite.pivot").unwrap();
        assert_eq!(path.component, "Sprite");
        assert_eq!(path.field, "pivot");
        assert_eq!(path.index, None);
        assert_eq!(path.to_string(), "Sprite.pivot");

        let indexed = PropertyPath::parse("Sprite.tint[2]").unwrap();
        assert_eq!(indexed.index, Some(2));
        assert_eq!(indexed.to_string(), "Sprite.tint[2]");

        assert!(matches!(
            PropertyPath::parse("Sprite"),
            Err(PropertyPathError::Malformed(_))
        ));
        assert!(matches!(
            PropertyPath::parse("Sprite.tint[x]"),
            Err(PropertyPathError::InvalidIndex(_))
        ));
        assert!(matches!(
            PropertyPath::parse("Sprite.tint[2"),
            Err(PropertyPathError::InvalidIndex(_))
        ));
    }

    #[test]
    fn reads_typed_values() {
        let payload =
            "texture=a.png;size=16x16;pivot=0.5,0.25;tint=1,1,1,1;sorting_layer=world;order=3";
        let pivot = read_builtin(payload, &PropertyPath::parse("Sprite.pivot").unwrap()).unwrap();
        assert_eq!(pivot, FieldValue::Tuple(vec![0.5, 0.25]));
        let order = read_builtin(payload, &PropertyPath::parse("Sprite.order").unwrap()).unwrap();
        assert_eq!(order, FieldValue::Int(3));
        let tint = read_builtin(payload, &PropertyPath::parse("Sprite.tint[2]").unwrap()).unwrap();
        assert_eq!(tint, FieldValue::Number(1.0));
        assert!(matches!(
            read_builtin(payload, &PropertyPath::parse("Sprite.tint[9]").unwrap()),
            Err(PropertyError::UnknownIndex { .. })
        ));
        assert!(matches!(
            read_builtin(payload, &PropertyPath::parse("NoSuch.field").unwrap()),
            Err(PropertyError::NoSchema(_))
        ));
    }

    #[test]
    fn writes_only_the_addressed_field() {
        let payload = "texture=a.png;size=16x16;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=world;order=3";
        let written = write_builtin(
            payload,
            &PropertyPath::parse("Sprite.pivot").unwrap(),
            "0.0,1.0",
        )
        .unwrap();
        assert_eq!(
            written,
            "texture=a.png;size=16x16;pivot=0.000000,1.000000;tint=1.000,1.000,1.000,1.000;sorting_layer=world;order=3",
            "只改目标字段，其余字段逐字保留"
        );
        // 写回同一个值：完全不变（无格式抖动）。
        assert_eq!(
            write_builtin(
                &written,
                &PropertyPath::parse("Sprite.pivot").unwrap(),
                "0.0,1.0"
            )
            .unwrap(),
            written
        );
        // 按下标写入：只替换该分量文本。
        let tinted = write_builtin(
            payload,
            &PropertyPath::parse("Sprite.tint[1]").unwrap(),
            "0.25",
        )
        .unwrap();
        assert!(tinted.contains("tint=1.000,0.25,1.000,1.000"), "{tinted}");
        assert_eq!(
            write_builtin(
                &tinted,
                &PropertyPath::parse("Sprite.tint[1]").unwrap(),
                "0.25"
            )
            .unwrap(),
            tinted
        );
    }

    #[test]
    fn rejects_values_the_schema_refuses() {
        let payload = "texture=a.png;size=16x16;pivot=0,0;tint=1,1,1,1;sorting_layer=world;order=0";
        assert!(matches!(
            write_builtin(
                payload,
                &PropertyPath::parse("Sprite.order").unwrap(),
                "abc"
            ),
            Err(PropertyError::InvalidValue { .. })
        ));
        assert!(matches!(
            write_builtin(
                payload,
                &PropertyPath::parse("Sprite.pivot").unwrap(),
                "1,2,3"
            ),
            Err(PropertyError::InvalidValue { .. })
        ));
        assert!(matches!(
            write_builtin(payload, &PropertyPath::parse("Sprite.nope").unwrap(), "1"),
            Err(PropertyError::UnknownField { .. })
        ));
    }

    #[test]
    fn appends_missing_optional_field() {
        let payload = "texture=a.png;size=16x16;tint=1,1,1,1;sorting_layer=world";
        let written =
            write_builtin(payload, &PropertyPath::parse("Sprite.order").unwrap(), "7").unwrap();
        assert!(written.ends_with(";order=7"), "{written}");
        assert!(written.starts_with("texture=a.png;size=16x16;tint=1,1,1,1;sorting_layer=world"));
    }

    #[test]
    fn typed_equality_ignores_formatting_only_differences() {
        assert!(field_text_equal(
            "Sprite",
            "pivot",
            "0.5,0.5",
            "0.500000,0.500000"
        ));
        assert!(field_text_equal(
            "Sprite",
            "size",
            "16x16",
            "16.000000x16.000000"
        ));
        assert!(field_text_equal("Sprite", "order", "1", "1"));
        assert!(!field_text_equal("Sprite", "pivot", "0.5,0.5", "0.5,0.6"));
        // M64a 的 `Image.color` 也走逐分量归一：漏掉 `FieldKind::Color` 就退回逐字比较，
        // `0.2` 与 `0.20` 这种纯格式差异会被编辑器记成一次真编辑（一条假 Undo）。
        assert!(field_text_equal(
            "Image",
            "color",
            "0.2,0.3,0.4,0.55",
            "0.20,0.30,0.40,0.550"
        ));
        // 段数不同就是两个值：三段是"省略 alpha"，四段半透明不是它的同义写法。
        assert!(!field_text_equal("Image", "color", "1,1,1", "1,1,1,1"));
        assert!(!field_text_equal("Image", "color", "1,1,1", "1,1,1,0.5"));
        // 未注册组件/字段退回逐字比较。
        assert!(!field_text_equal("Nope", "x", "1.0", "1.000"));
        assert!(field_text_equal("Nope", "x", "1.0", "1.0"));
    }
}
