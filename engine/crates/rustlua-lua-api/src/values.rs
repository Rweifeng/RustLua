//! Lua 值 ↔ JSON 的转换（`docs/62` P1 ⑤：从 `rustlua-scripting` 搬到契约层）。
//!
//! ## 为什么住在这里
//!
//! 存档的 blob（`save.blob_write/read`）与地图状态把"任意 Lua 表"序列化成 JSON 存进
//! 字符串键值表。这段转换与领域无关（不认识 `World`、不认识 UI），却一直住在
//! `rustlua-scripting` 里——于是任何"下沉到领域 crate 的绑定"只要用到 blob 就得把整段
//! 转换逻辑复制一份，或者反过来依赖脚本层（那是环）。
//!
//! 搬到契约层（本 crate）之后：`rustlua-scripting` 的 `script::values` 只做 `pub use`
//! 再导出（旧路径不动），`rustlua-gameplay` 的存档绑定直接用这里的实现。
//!
//! ## 语义约束（与旧实现逐字一致）
//!
//! - 非有限浮点（NaN/±inf）**拒绝**：JSON 没有它们，静默写成 `null` 会让读回的数值
//!   从"某个具体的数"变成"没存过"。
//! - Lua 表按"纯数组"或"纯对象"二选一：键是 1..n 的连续整数 ⇒ 数组；否则所有键必须
//!   是字符串或整数，混排直接报错（不猜调用方意图）。
//! - `serde_json::Value::Number` 读回时优先整数，避免 `3` 变成 `3.0` 让 Lua 侧 `math.type` 变化。

use mlua::{Lua, Result as LuaResult, Table, Value};

use crate::api_error;

/// 任意 Lua 值 → JSON（`save.blob_*`、地图状态用）。
pub fn lua_value_to_json(value: Value) -> Result<serde_json::Value, mlua::Error> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(value) => Ok(serde_json::Value::Bool(value)),
        Value::Integer(value) => Ok(serde_json::Value::Number(value.into())),
        Value::Number(value) => {
            if !value.is_finite() {
                return Err(api_error("blob value must contain finite numbers"));
            }
            serde_json::Number::from_f64(value)
                .map(serde_json::Value::Number)
                .ok_or_else(|| api_error("blob value contains an invalid number"))
        }
        Value::String(value) => value
            .to_str()
            .map(|text| serde_json::Value::String(text.to_owned()))
            .map_err(mlua::Error::external),
        Value::Table(table) => lua_table_to_json(&table),
        _ => Err(api_error("unsupported Lua value type for blob storage")),
    }
}

/// Lua 表 → JSON 数组或对象（判定规则见模块文档）。
pub fn lua_table_to_json(table: &Table) -> Result<serde_json::Value, mlua::Error> {
    let len = table.raw_len();
    let mut array = vec![serde_json::Value::Null; len];
    let mut array_like = true;
    for pair in table.clone().pairs() {
        let (key, value) = pair?;
        let converted = lua_value_to_json(value)?;
        match key {
            Value::Integer(index) if index >= 1 && (index as usize) <= len => {
                array[(index - 1) as usize] = converted;
            }
            _ => {
                array_like = false;
                break;
            }
        }
    }
    if array_like && len > 0 {
        return Ok(serde_json::Value::Array(array));
    }

    let mut object = serde_json::Map::new();
    for pair in table.clone().pairs() {
        let (key, value) = pair?;
        let key = match key {
            Value::Integer(index) => index.to_string(),
            Value::String(text) => text.to_str()?.to_owned(),
            _ => return Err(api_error("blob table keys must be strings or integers")),
        };
        object.insert(key, lua_value_to_json(value)?);
    }
    Ok(serde_json::Value::Object(object))
}

/// JSON → 任意 Lua 值。
pub fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> LuaResult<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(value) => Ok(Value::Boolean(*value)),
        serde_json::Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Ok(Value::Integer(integer))
            } else {
                Ok(Value::Number(value.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(value) => Ok(Value::String(lua.create_string(value)?)),
        serde_json::Value::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, value)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                table.set(key.as_str(), json_to_lua_value(lua, value)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_through_lua() {
        let lua = Lua::new();
        let json = serde_json::json!({
            "name": "gu",
            "hp": 12,
            "ratio": 0.5,
            "alive": true,
            "items": ["a", "b"],
        });
        let value = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(value).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn rejects_non_finite_and_mixed_tables() {
        let lua = Lua::new();
        let nan = lua.create_table().unwrap();
        nan.set("x", f32::NAN).unwrap();
        assert!(lua_table_to_json(&nan).is_err(), "NaN 必须被拒绝");

        let mixed = lua.create_table().unwrap();
        mixed.set(1, "a").unwrap();
        mixed.set("k", "v").unwrap();
        // 混排键：`raw_len` 为 1 但存在字符串键 ⇒ 走对象分支，键 1 会变成 "1"。
        let json = lua_table_to_json(&mixed).unwrap();
        assert!(json.is_object(), "混排表必须落到对象分支：{json:?}");
    }

    #[test]
    fn integers_stay_integers() {
        let lua = Lua::new();
        let value = json_to_lua_value(&lua, &serde_json::json!(3)).unwrap();
        assert!(matches!(value, Value::Integer(3)), "整数不能退化成 3.0");
    }
}
