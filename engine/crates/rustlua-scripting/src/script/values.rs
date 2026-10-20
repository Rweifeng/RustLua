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
use rustlua_ecs::entity::EntityId;
use rustlua_math::math3d::Vec3;

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

/// 宽容版解码：**不是句柄形态**的整数返回 `None`，而不是报错。
///
/// 只给"谓词/查询"用（`entity.is_alive`、`entity.persist_id`）。为什么必须宽容：
/// 实体 id 会被工程**存进自己的数据**（`map.state("buildings")` 里的 `rec.entity` → 随存档落盘），
/// 而 web 旧运行时的实体 id 是 `1, 2, 3…` 这种小整数（它的 Lua 预置表自己发号，
/// `web-runtime.js:2027`）。玩家带着旧运行时的存档进来时，`is_alive(1)` 若抛错，工程里遍地是
/// `if rec.entity and entity.is_alive(rec.entity) then` 的写法 ⇒ 脚本当场报错、建筑与地块的
/// 实体链接全部恢复失败。返回 `false` 则退化成"这个实体不在了"，工程走它既有的重建分支——
/// 与旧运行时语义一致（旧运行时的 `is_alive` 对未知 id 就是 `false`）。
///
/// **不要**拿它做写操作的前置解码：`transform.set` 之类必须继续用严格版
/// （改动一个不存在的实体是 bug，要当场报出来）。
#[must_use]
pub(super) fn entity_from_lua_lenient(value: i64) -> Option<EntityId> {
    if value < 0 {
        return None;
    }
    let raw = value as u64;
    let generation = raw as u32;
    if generation == 0 {
        return None;
    }
    Some(EntityId::from_raw((raw >> 32) as u32, generation))
}

/// Lua 侧实体 id 的编码（`index << 32 | generation`）。
///
/// 宿主在需要把一个实体交给 Lua（生成脚本、注入参数、测试对拍）时必须用同一份编码；
/// 这里公开它，避免调用方各写一遍位运算。反向解码是同文件里的私有 `entity_from_lua`，
/// 以及私有 `script::api` 各模块注册给 Lua 的那些取实体 id 的函数。
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

// 存档 blob / 地图状态的 Lua ↔ JSON 转换已搬到契约层（`docs/62` P1 ⑤）：
// 绑定下沉到领域 crate 后仍要用它，而领域 crate 不能反向依赖脚本层。
// 这里再导出，`crate::script::values::{json_to_lua_value, lua_value_to_json}` 的旧路径不动。
pub(super) use rustlua_lua_api::values::{json_to_lua_value, lua_value_to_json};
