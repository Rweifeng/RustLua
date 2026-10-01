//! Lightweight JSON-schema validation for data tables loaded through the Lua
//! `data` API (M16-11).
//!
//! Kept intentionally small — the farm content tables (items / crops / shop /
//! farm) only need map / array / object shapes and the JSON primitive types.
//! Schemas are authored from Lua (game-specific, not hardcoded in Rust) and
//! serialized to `serde_json::Value`; [`crate::core::data_schema::validate`] reports the first violation
//! with a dotted field path so bad data is located at the file/field level.
//!
//! A schema value is either a primitive kind name (`"string"`, `"number"`,
//! `"integer"`, `"boolean"`, `"any"`), a composite object discriminated by
//! `"$kind"`, or a bare object which is shorthand for an implicit object schema:
//! - `{ "$kind": "array",  "items": <schema> }`
//! - `{ "$kind": "object", "fields": { name: <schema> }, "required": [...] }`
//! - `{ "$kind": "map",    "values": <schema> }` (string-keyed dictionary,
//!   each value validated against `values`; e.g. `items.json`)
//! - `{ name: <schema>, ... }` — implicit object schema (fields = itself)
//!
//! The discriminator is `"$kind"` (not `"kind"`) so game tables that use a
//! field literally named `kind` (e.g. item definitions) do not collide.

use serde_json::Value;

/// Validates `value` against `schema`. Returns a description of the first
/// violation (with a `path` like `data/items.json.seed_tomato.stack`) or
/// `None` when `value` conforms.
pub fn validate(value: &Value, schema: &Value, path: &str) -> Option<String> {
    if let Some(kind) = schema.as_str() {
        return validate_primitive(value, kind, path);
    }
    let Value::Object(map) = schema else {
        return Some(format!("{path}: schema must be a kind name or an object"));
    };
    if let Some(kind) = map.get("$kind").and_then(Value::as_str) {
        return match kind {
            "array" => validate_array(value, schema, path),
            "object" => validate_object(value, schema, path),
            "map" => validate_map(value, schema, path),
            other => Some(format!("{path}: unknown schema kind {other:?}")),
        };
    }
    // Shorthand: a bare object is an implicit object schema (fields = itself).
    validate_object(value, schema, path)
}

fn validate_primitive(value: &Value, kind: &str, path: &str) -> Option<String> {
    let ok = match kind {
        "any" => true,
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "number" => value.is_number(),
        other => return Some(format!("{path}: unknown schema kind {other:?}")),
    };
    if ok {
        None
    } else {
        Some(format!("{path}: expected {kind}, got {}", type_name(value)))
    }
}

fn validate_array(value: &Value, schema: &Value, path: &str) -> Option<String> {
    let Value::Array(list) = value else {
        return Some(format!("{path}: expected array, got {}", type_name(value)));
    };
    let Some(items) = schema.get("items") else {
        return Some(format!("{path}: array schema missing `items`"));
    };
    for (index, item) in list.iter().enumerate() {
        if let Some(error) = validate(item, items, &format!("{path}[{index}]")) {
            return Some(error);
        }
    }
    None
}

fn validate_object(value: &Value, schema: &Value, path: &str) -> Option<String> {
    let Value::Object(map) = value else {
        return Some(format!("{path}: expected object, got {}", type_name(value)));
    };
    let fields = if let Some(fields) = schema.get("fields").and_then(Value::as_object) {
        fields
    } else if let Value::Object(schema_map) = schema {
        schema_map
    } else {
        return Some(format!("{path}: object schema missing `fields`"));
    };
    for (name, field_schema) in fields {
        match map.get(name) {
            Some(field_value) => {
                if let Some(error) = validate(field_value, field_schema, &format!("{path}.{name}"))
                {
                    return Some(error);
                }
            }
            None => {
                if is_required(schema, name) {
                    return Some(format!("{path}: missing required field {name:?}"));
                }
            }
        }
    }
    None
}

fn validate_map(value: &Value, schema: &Value, path: &str) -> Option<String> {
    let Value::Object(map) = value else {
        return Some(format!(
            "{path}: expected map/object, got {}",
            type_name(value)
        ));
    };
    let Some(values) = schema.get("values") else {
        return Some(format!("{path}: map schema missing `values`"));
    };
    for (key, item) in map {
        if let Some(error) = validate(item, values, &format!("{path}.{key}")) {
            return Some(error);
        }
    }
    None
}

fn is_required(schema: &Value, name: &str) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().any(|value| value.as_str() == Some(name)))
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.as_i64().is_some() || number.as_u64().is_some() => {
            "integer"
        }
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use serde_json::json;

    #[test]
    fn passes_matching_primitives_and_shapes() {
        // A map whose values are implicit object schemas; the item field named
        // `kind` must not collide with the `$kind` discriminator.
        let schema = json!({
            "$kind": "map",
            "values": {
                "name": "string",
                "stack": "integer",
                "buy": "number",
                "kind": "string",
            }
        });
        let value = json!({
            "seed_tomato": { "name": "种子", "stack": 99, "buy": 10, "kind": "seed" },
            "tomato": { "name": "番茄", "stack": 16, "buy": 0, "kind": "crop" },
        });
        assert_eq!(validate(&value, &schema, "data/items.json"), None);
    }

    #[test]
    fn reports_field_type_mismatch_with_path() {
        let schema = json!({ "$kind": "map", "values": { "stack": "integer" } });
        let value = json!({ "tomato": { "stack": "big" } });
        let error = validate(&value, &schema, "data/items.json").unwrap();
        assert!(error.contains("data/items.json.tomato.stack"), "{error}");
        assert!(error.contains("integer"), "{error}");
    }

    #[test]
    fn requires_missing_fields_and_rejects_bad_array_items() {
        let schema = json!({
            "$kind": "object",
            "fields": { "x": "number", "y": "number" },
            "required": ["y"],
        });
        assert_eq!(
            validate(&json!({"x": 1.0}), &schema, "plot"),
            Some("plot: missing required field \"y\"".to_owned())
        );
        let array_schema = json!({
            "$kind": "array",
            "items": { "$kind": "object", "fields": { "x": "number", "y": "number" } }
        });
        let bad = json!([{ "x": 1.0, "y": 2.0 }, { "x": "a", "y": 3.0 }]);
        let error = validate(&bad, &array_schema, "farm.plots").unwrap();
        assert!(error.contains("farm.plots[1].x"), "{error}");
    }

    #[test]
    fn floats_fail_integer_and_pass_number() {
        let integer = json!("integer");
        let number = json!("number");
        assert_eq!(validate(&json!(3.0), &integer, "v"), None);
        assert_eq!(
            validate(&json!(3.5), &integer, "v"),
            Some("v: expected integer, got number".to_owned())
        );
        assert_eq!(validate(&json!(3.5), &number, "v"), None);
        assert_eq!(
            validate(&json!("x"), &number, "v"),
            Some("v: expected number, got string".to_owned())
        );
    }
}
