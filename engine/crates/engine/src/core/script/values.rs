//! Lua ↔ Rust 的值桥：实体句柄编解码、常用返回表构造、Lua ↔ JSON 互转、参数有限性校验。
//!
//! 这里不放任何**业务**逻辑，只放被 275 个绑定函数反复复用的表示层小工具。
//! 两条刻意的设计：
//! - 实体在 Lua 侧是 **i64**（`index << 32 | generation`）而不是 userdata：可打印、可当表键、
//!   可进存档、跨 wasm/JS 边界无需序列化；代价是 Lua 侧没有类型保护，因此每个入口都要过
//!   [`entity_from_lua`] + 需要活体时用 [`require_alive`]。
//! - 数值校验（[`finite_ui_rect`] / [`finite_color4`]）宁可在绑定层**拒绝** NaN/Inf，
//!   也不静默当 0——NaN 会污染整帧布局或深度测试，事后无从定位。

use mlua::{Lua, Result as LuaResult, Table, Value};

use super::{ScriptState, error::api_error};
use crate::core::{entity::EntityId, math3d::Vec3};

pub(super) fn format_lua_value(value: Value) -> String {
    match value {
        Value::Nil => "nil".to_owned(),
        Value::Boolean(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.to_string_lossy(),
        Value::Table(_) => "<table>".to_owned(),
        Value::Function(_) => "<function>".to_owned(),
        Value::Thread(_) => "<thread>".to_owned(),
        Value::UserData(_) => "<userdata>".to_owned(),
        Value::LightUserData(_) => "<lightuserdata>".to_owned(),
        Value::Error(error) => error.to_string(),
        Value::Other(_) => "<other>".to_owned(),
    }
}

/// 解析查询的可选 `{ layer = N, mask = N }` 参数（docs/54 §8-⑥：查询也吃 `layer/mask`）。
///
/// - 缺省/空表 = `None`（全收，老行为）。
/// - 只给一侧时另一侧取 `0xFF`（"我在这一层，谁都看得见"是该侧最常用的意图）。
/// - 越界或非整数**报错**：位域是 u8，静默截断会让 `layer=256` 变成"全层可见"这种
///   只在真机上才发现的错。
pub(super) fn query_groups(opts: Option<mlua::Table>) -> Result<Option<(u8, u8)>, mlua::Error> {
    let Some(opts) = opts else {
        return Ok(None);
    };
    let read = |key: &str| -> Result<Option<u8>, mlua::Error> {
        let Some(raw) = opts.get::<Option<i64>>(key)? else {
            return Ok(None);
        };
        u8::try_from(raw).map(Some).map_err(|_| {
            api_error(format!(
                "query {key} must be an integer in 0..=255 (bit field), got {raw}"
            ))
        })
    };
    let layer = read("layer")?;
    let mask = read("mask")?;
    if layer.is_none() && mask.is_none() {
        return Ok(None);
    }
    Ok(Some((layer.unwrap_or(0xFF), mask.unwrap_or(0xFF))))
}

pub(super) fn entity_from_lua(value: i64) -> Result<EntityId, mlua::Error> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    let raw = value as u64;
    let index = (raw >> 32) as u32;
    let generation = raw as u32;
    if generation == 0 {
        return Err(api_error(format!("invalid entity id: {value}")));
    }
    Ok(EntityId::from_raw(index, generation))
}

/// Lua 侧实体 id 的编码（`index << 32 | generation`）。
///
/// 宿主在需要把一个实体交给 Lua（生成脚本、注入参数、测试对拍）时必须用同一份编码；
/// 这里公开它，避免调用方各写一遍位运算。反向解码见 [`LuaRuntime::with_script`] 里注册的 API。
#[must_use]
pub fn entity_lua_id(entity: EntityId) -> i64 {
    ((entity.index() as u64) << 32 | entity.generation() as u64) as i64
}

pub(super) fn entity_to_lua(entity: EntityId) -> i64 {
    entity_lua_id(entity)
}

pub(super) fn require_alive(state: &ScriptState, entity: EntityId) -> Result<(), mlua::Error> {
    if state.world.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}

pub(super) fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

/// 屏幕像素 → 世界射线（六个分量：ox,oy,oz,dx,dy,dz）——可直接喂给 `physics3d.raycast`。
pub(super) fn table_ray(lua: &Lua, origin: Vec3, direction: Vec3) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set("ox", origin.x)?;
    table.set("oy", origin.y)?;
    table.set("oz", origin.z)?;
    table.set("dx", direction.x)?;
    table.set("dy", direction.y)?;
    table.set("dz", direction.z)?;
    Ok(table)
}

pub(super) fn table_i32_pair(lua: &Lua, x: i32, y: i32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

pub(super) fn finite_ui_rect(x: f32, y: f32, width: f32, height: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && width.is_finite()
        && height.is_finite()
        && width > 0.0
        && height > 0.0
}

pub(super) fn finite_color4(color: [f32; 4]) -> bool {
    color.iter().all(|channel| channel.is_finite())
}

/// Converts an arbitrary Lua value into JSON so `save.blob_*` and map state
/// can round-trip through the existing string-keyed save file.
pub(super) fn lua_value_to_json(value: Value) -> Result<serde_json::Value, mlua::Error> {
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

pub(super) fn lua_table_to_json(table: &Table) -> Result<serde_json::Value, mlua::Error> {
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

pub(super) fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> LuaResult<Value> {
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
