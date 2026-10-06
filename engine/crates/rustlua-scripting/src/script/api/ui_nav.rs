//! `ui` 的焦点环与滚动：`focus_*` / `scroll_*`（`rustlua_ui::ui_navigation` 的运行期接线）。
//!
//! 序号统一 0 基；Lua 面板常用 1 基，由游戏侧 `panel_nav.lua` 转换。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use mlua::Result as LuaResult;
use mlua::{Lua, Table, Value};
use rustlua_ui::ui_navigation::FocusItem;
use rustlua_ui::ui_navigation::FocusRing;
use rustlua_ui::ui_navigation::NavDirection;
use rustlua_ui::ui_navigation::ScrollState;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, ui: Table, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    // ---- M20-02：UI 焦点与滚动导航（`rustlua_ui::ui_navigation` 的运行时接线）----
    // 面板用方向键/摇杆方向驱动列表选择与滚动；序号统一 0 基（Lua 面板常用 1 基，
    // 由游戏侧 `panel_nav.lua` 转换）。`focus_open` 按 `columns` 布局项矩形，
    // 使网格面板（背包 2 列）的左右移动与单列列表的上下移动都可用。
    let s = Rc::clone(&state);
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
            let mut state = s.lua_borrow_mut()?;
            match state.ui_state.focus_rings.get_mut(&id) {
                Some(ring) => ring.set_items(items),
                None => {
                    state.ui_state.focus_rings.insert(id, FocusRing::new(items));
                }
            }
            Ok(count as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_close",
        lua.create_function(move |_, id: String| {
            Ok(s.lua_borrow_mut()?
                .ui_state
                .focus_rings
                .remove(&id)
                .is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_count",
        lua.create_function(move |_, id: String| {
            Ok(s.lua_borrow()?
                .ui_state
                .focus_rings
                .get(&id)
                .map_or(0_i64, |ring| ring.len() as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_select",
        lua.create_function(move |_, (id, index): (String, i64)| {
            if index < 0 {
                return Ok(false);
            }
            let mut state = s.lua_borrow_mut()?;
            let Some(ring) = state.ui_state.focus_rings.get_mut(&id) else {
                return Ok(false);
            };
            Ok(ring.set_current(index as u64 + 1))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_index",
        lua.create_function(move |_, id: String| {
            let state = s.lua_borrow()?;
            let Some(ring) = state.ui_state.focus_rings.get(&id) else {
                return Ok(None);
            };
            Ok(ring
                .current()
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
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
            let mut state = s.lua_borrow_mut()?;
            let Some(ring) = state.ui_state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .move_focus(direction)
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "focus_cycle",
        lua.create_function(move |_, (id, forward): (String, Option<bool>)| {
            let mut state = s.lua_borrow_mut()?;
            let Some(ring) = state.ui_state.focus_rings.get_mut(&id) else {
                return Ok(None);
            };
            Ok(ring
                .cycle(forward.unwrap_or(true))
                .and_then(|item| usize::try_from(item.id).ok())
                .and_then(|id| id.checked_sub(1))
                .map(|index| index as i64))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_open",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            s.lua_borrow_mut()?
                .ui_state
                .scroll_states
                .insert(id, ScrollState::new(viewport, content));
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_close",
        lua.create_function(move |_, id: String| {
            Ok(s.lua_borrow_mut()?
                .ui_state
                .scroll_states
                .remove(&id)
                .is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_extents",
        lua.create_function(move |_, (id, viewport, content): (String, f32, f32)| {
            let mut state = s.lua_borrow_mut()?;
            let scroll = state.ui_state.scroll_states.entry(id).or_default();
            scroll.set_extents(viewport, content);
            Ok(scroll.offset())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_offset",
        lua.create_function(move |_, id: String| {
            Ok(s.lua_borrow()?
                .ui_state
                .scroll_states
                .get(&id)
                .map(ScrollState::offset))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_max",
        lua.create_function(move |_, id: String| {
            Ok(s.lua_borrow()?
                .ui_state
                .scroll_states
                .get(&id)
                .map(ScrollState::max_offset))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_by",
        lua.create_function(move |_, (id, delta): (String, f32)| {
            let mut state = s.lua_borrow_mut()?;
            let scroll = state.ui_state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_by(delta))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_lines",
        lua.create_function(move |_, (id, lines, line_height): (String, f32, f32)| {
            let mut state = s.lua_borrow_mut()?;
            let scroll = state.ui_state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_lines(lines, line_height))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_page",
        lua.create_function(move |_, (id, pages): (String, f32)| {
            let mut state = s.lua_borrow_mut()?;
            let scroll = state.ui_state.scroll_states.entry(id).or_default();
            Ok(scroll.page(pages))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_to_visible",
        lua.create_function(move |_, (id, item_top, item_height): (String, f32, f32)| {
            let mut state = s.lua_borrow_mut()?;
            let scroll = state.ui_state.scroll_states.entry(id).or_default();
            Ok(scroll.scroll_to_visible(item_top, item_height))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scroll_visible",
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(scroll) = state.ui_state.scroll_states.get(&id) else {
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
