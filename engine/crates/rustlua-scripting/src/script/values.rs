//! Lua ↔ Rust 的值桥：实体句柄编码、Lua ↔ JSON 互转、参数有限性校验。
//!
//! 这里不放任何**业务**逻辑，只放被绑定函数反复复用的表示层小工具。
//!
//! **P1 ⑤-14 之后的边界**：实体 id 的**解码**（Lua i64 → `EntityId`）与常用返回表构造已经跟着
//! 各自的绑定搬去领域 crate（`rustlua-ecs` / `rustlua-render` / `rustlua-physics` / …），因为它们
//! 和"报错文案要逐字一致"绑在一起。本文件只留**编码**一侧与 JSON 互转：
//!
//! - 编码（[`entity_lua_id`]）：宿主在生成脚本、注入参数、测试对拍时都要把实体交给 Lua，
//!   必须与各领域绑定用同一份位运算，所以它是公开的。
//! - 数值校验（[`finite_ui_rect`] / [`finite_color4`] 已随 UI 绑定搬走）：绑定层宁可**拒绝**
//!   NaN/Inf，也不静默当 0——NaN 会污染整帧布局或深度测试，事后无从定位。

use mlua::Value;

use crate::script::domain::ecs::entity::EntityId;

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

/// Lua 侧实体 id 的编码（`index << 32 | generation`）。
///
/// 宿主在需要把一个实体交给 Lua（生成脚本、注入参数、测试对拍）时必须用同一份编码；
/// 这里公开它，避免调用方各写一遍位运算。反向解码在领域绑定的各 `entity_from_lua` 里
/// （P1 ⑤-14 后不再有脚本层那一个）。
#[must_use]
pub fn entity_lua_id(entity: EntityId) -> i64 {
    ((entity.index() as u64) << 32 | entity.generation() as u64) as i64
}

pub(super) fn entity_to_lua(entity: EntityId) -> i64 {
    entity_lua_id(entity)
}

// 存档 blob / 地图状态的 Lua ↔ JSON 转换已搬到契约层（`docs/62` P1 ⑤）：
// 绑定下沉到领域 crate 后仍要用它，而领域 crate 不能反向依赖脚本层。
// `json_to_lua_value` 的最后一个调用点随 ⑤-4c 的 `data.*` / `map.state` 绑定搬去了
// `rustlua-gameplay`，这里只再导出脚本层仍在用的那一个。
pub(super) use crate::script::domain::lua_api::values::lua_value_to_json;

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    /// 编码与解码必须互为逆运算：宿主交给 Lua 的整数，绑定侧要能解回同一个句柄。
    #[test]
    fn entity_id_encodes_index_and_generation() {
        let entity = EntityId::from_raw(7, 3);
        assert_eq!(entity_lua_id(entity), (7_i64 << 32) | 3);
        let decoded = EntityId::from_lua_i64(entity_lua_id(entity)).expect("roundtrip");
        assert_eq!(decoded, entity);
    }

    /// `format_lua_value` 只做诊断展示：表/函数一律折叠成 `<table>` / `<function>`。
    #[test]
    fn lua_values_are_formatted_for_diagnostics() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        assert_eq!(format_lua_value(Value::Nil), "nil");
        assert_eq!(format_lua_value(Value::Integer(3)), "3");
        assert_eq!(format_lua_value(Value::Table(table)), "<table>");
    }
}
