//! `ui` 的焦点环与滚动：`focus_*` / `scroll_*`（`ui_navigation` 的运行期接线）。
//!
//! 序号统一 0 基；Lua 面板常用 1 基，由游戏侧 `panel_nav.lua` 转换。
//!
//! ## 为什么在本 crate（`docs/62` P1 ⑤-3）
//!
//! 焦点环与滚动位置这两张表只被本模块的 12 个 Lua 函数读写，而 `FocusRing` /
//! `ScrollState` 以及"方向键怎么翻页、贴边、跳列"的规则都在 [`crate::ui_navigation`]——
//! 绑定却曾住在 VM 宿主里，于是"面板怎么导航"两头都有代码。现在状态表与绑定一起归
//! `rustlua-ui`，`rustlua-scripting` 只把能力单元的句柄交出来。
//!
//! ## 借用口径
//!
//! 每个函数只借 [`NavState`] 很短一段（不进渲染、不回调 Lua），所以重入在结构上不会发生；
//! 真发生时报的是 `ui_nav`，而不是"脚本状态被重入"。

// WP11-USE:BEGIN
use std::collections::HashMap;

use mlua::Result as LuaResult;
use mlua::{Lua, Table, Value};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;

use crate::ui_navigation::{FocusItem, FocusRing, NavDirection, ScrollState};
// WP11-USE:END

/// UI 导航的运行期状态：焦点环与滚动位置（均按面板 id 索引）。
///
/// 字段是 `pub` 的：绑定与本结构同在一个 crate，而宿主（`rustlua-scripting`）只需要
/// 持有句柄、在场景切换时调 [`NavState::clear`]，不必逐字段转手。
#[derive(Debug, Default)]
pub struct NavState {
    /// 每个面板的焦点环（key 与 Lua 侧 `ui.focus_open(id, ...)` 的 id 同源）。
    pub focus_rings: HashMap<String, FocusRing>,
    /// 每个面板的滚动位置（key 同焦点环）。
    pub scroll_states: HashMap<String, ScrollState>,
}

impl NavState {
    /// 建一份能力单元（`what` 用于重入报错）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "ui_nav")
    }

    /// 清空全部面板状态（场景切换、重新装配 UI 时用）。
    pub fn clear(&mut self) {
        self.focus_rings.clear();
        self.scroll_states.clear();
    }
}

pub fn install(lua: &Lua, ui: &Table, nav: &Handle<NavState>) -> LuaResult<()> {
    // ---- M20-02：UI 焦点与滚动导航（`rustlua_ui::ui_navigation` 的运行时接线）----
    // 面板用方向键/摇杆方向驱动列表选择与滚动；序号统一 0 基（Lua 面板常用 1 基，
    // 由游戏侧 `panel_nav.lua` 转换）。`focus_open` 按 `columns` 布局项矩形，
    // 使网格面板（背包 2 列）的左右移动与单列列表的上下移动都可用。
    let s = nav.clone();
    ui.set(
        "focus_open",
        lua.create_function(move |_, (id, count, columns): (String, i64, Option<i64>)| {
            let count = count.clamp(0, 4096) as usize;
            let columns = columns.unwrap_or(1).clamp(1, 64) as usize;
            let items: Vec<FocusItem> = (0..count)
                .map(|index| {
                    let column = index % columns;
                    let line = index / columns;
                    FocusItem {
                        id: index as u64 + 1,
                        rect: [column as f32, line as f32, 1.0, 1.0],
                    }
                })
                .collect();
            let mut state = s.borrow_mut()?;
            match state.focus_rings.get_mut(&id) {
                Some(ring) => ring.set_items(items),
                None => {
                    state.focus_rings.insert(id, FocusRing::new(items));
                }
            }
            Ok(count as i64)
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_close",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow_mut()?.focus_rings.remove(&id).is_some())
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_count",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow()?
                .focus_rings
                .get(&id)
                .map_or(0_i64, |ring| ring.len() as i64))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_select",
        lua.create_function(move |_, (id, index): (String, i64)| {
            if index < 0 {
                return Ok(false);
            }
            let mut state = s.borrow_mut()?;
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(false);
            };
            Ok(ring.set_current(index as u64 + 1))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_index",
        lua.create_function(move |_, id: String| {
            let state = s.borrow()?;
            let Some(ring) = state.focus_rings.get(&id) else {
                return Ok(None);
            };
            Ok(ring
                .current()
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_move",
        lua.create_function(move |_, (id, direction): (String, String)| {
            let direction = match direction.trim().to_ascii_lowercase().as_str() {
                "up" => NavDirection::Up,
                "down" => NavDirection::Down,
                "left" => NavDirection::Left,
                "right" => NavDirection::Right,
                other => {
                    return Err(api_error(format!(
                        "ui.focus_move direction must be up/down/left/right, got {other}"
                    )));
                }
            };
            let mut state = s.borrow_mut()?;
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .move_focus(direction)
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "focus_cycle",
        lua.create_function(move |_, (id, forward): (String, Option<bool>)| {
            let mut state = s.borrow_mut()?;
            let Some(ring) = state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .cycle(forward.unwrap_or(true))
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_open",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            s.borrow_mut()?
                .scroll_states
                .insert(id, ScrollState::new(viewport, content));
            Ok(())
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_close",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow_mut()?.scroll_states.remove(&id).is_some())
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_extents",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            let mut state = s.borrow_mut()?;
            let scroll = state.scroll_states.entry(id).or_default();
            scroll.set_extents(viewport, content);
            Ok(scroll.offset())
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_offset",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow()?.scroll_states.get(&id).map(ScrollState::offset))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_max",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow()?
                .scroll_states
                .get(&id)
                .map(ScrollState::max_offset))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_by",
        lua.create_function(move |_, (id, delta): (String, f32)| {
            let mut state = s.borrow_mut()?;
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_by(delta))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_lines",
        lua.create_function(move |_, (id, lines, line_height): (String, f32, f32)| {
            let mut state = s.borrow_mut()?;
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_lines(lines, line_height))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_page",
        lua.create_function(move |_, (id, pages): (String, f32)| {
            let mut state = s.borrow_mut()?;
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.page(pages))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_to_visible",
        lua.create_function(move |_, (id, item_top, item_height): (String, f32, f32)| {
            let mut state = s.borrow_mut()?;
            let scroll = state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_to_visible(item_top, item_height))
        })?,
    )?;
    let s = nav.clone();
    ui.set(
        "scroll_visible",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow()?;
            let Some(scroll) = state.scroll_states.get(&id) else {
                return Ok(Value::Nil);
            };
            let (start, stop) = scroll.visible_range();
            let result = lua.create_table()?;
            // 不能叫 `end`：Lua 里 `range.end` 是关键字，取值必须写 `range['end']`。
            result.set("start", start)?;
            result.set("stop", stop)?;
            Ok(Value::Table(result))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    /// 装的函数名**全部**在这里：Lua 面是 17 个（`api_surface_snapshot` 盯的是全量快照，
    /// 这条单测盯的是"搬家之后一个都没丢"，失败时不需要去 diff 几千行快照）。
    const NAMES: [&str; 17] = [
        "focus_open",
        "focus_close",
        "focus_count",
        "focus_select",
        "focus_index",
        "focus_move",
        "focus_cycle",
        "scroll_open",
        "scroll_close",
        "scroll_extents",
        "scroll_offset",
        "scroll_max",
        "scroll_by",
        "scroll_lines",
        "scroll_page",
        "scroll_to_visible",
        "scroll_visible",
    ];

    fn installed() -> (Lua, Table, Handle<NavState>) {
        let lua = Lua::new();
        let ui = lua.create_table().unwrap();
        let nav = NavState::handle();
        install(&lua, &ui, &nav).unwrap();
        (lua, ui, nav)
    }

    #[test]
    fn every_nav_function_is_installed() {
        let (_lua, ui, _nav) = installed();
        for name in NAMES {
            let value: Value = ui.get(name).unwrap();
            assert!(
                matches!(value, Value::Function(_)),
                "`ui.{name}` 缺失或不是函数：{value:?}"
            );
        }
    }

    #[test]
    fn focus_and_scroll_round_trip_through_the_handle() {
        let (lua, ui, nav) = installed();
        let globals = lua.globals();
        globals.set("ui", &ui).unwrap();

        lua.load(
            r#"
            ui.focus_open("bag", 3, nil)
            assert(ui.focus_count("bag") == 3, "focus_count")
            ui.focus_select("bag", 2)
            assert(ui.focus_index("bag") == 2, "直选第二项")
            ui.focus_move("bag", "down")
            assert(ui.focus_index("bag") ~= nil, "下移后仍有选中项")
            assert(ui.focus_count("bag") == 3, "移动不改变项数")

            ui.scroll_open("bag", 100.0, 300.0)
            ui.scroll_by("bag", 40.0)
            assert(ui.scroll_offset("bag") == 40.0, "滚动偏移")
            assert(ui.scroll_max("bag") == 200.0, "最大偏移 = 内容 - 视口")
            "#,
        )
        .exec()
        .unwrap();

        // 状态确实落在能力单元里（不是 Lua 侧影子表）。
        {
            let state = nav.borrow().unwrap();
            assert_eq!(state.focus_rings.len(), 1);
            assert_eq!(state.scroll_states.len(), 1);
        }
        // 场景切换时的清理走同一条通道。
        nav.borrow_mut().unwrap().clear();
        let state = nav.borrow().unwrap();
        assert!(state.focus_rings.is_empty() && state.scroll_states.is_empty());
    }

    /// 未登记的面板一律"查不到"而不是崩：Lua 侧 `if ui.focus_index(id) then` 的既有写法
    /// 依赖 `nil`（web 兼容层同语义），搬家时这条不能变。
    #[test]
    fn unknown_panel_yields_nil() {
        let (lua, ui, _nav) = installed();
        lua.globals().set("ui", &ui).unwrap();
        let value: Value = lua.load(r#"return ui.focus_index("nope")"#).eval().unwrap();
        assert!(matches!(value, Value::Nil), "应为 nil：{value:?}");
        let count: Value = lua.load(r#"return ui.focus_count("nope")"#).eval().unwrap();
        assert!(
            matches!(count, Value::Integer(0) | Value::Nil),
            "应为 0 或 nil：{count:?}"
        );
    }
}
