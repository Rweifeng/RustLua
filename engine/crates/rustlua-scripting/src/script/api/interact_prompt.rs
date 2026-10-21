//! `interact.prompt` / `clear_prompt` / `get_prompt`（P1 ⑤-13 后 `interact` 表留在脚本层的三个函数）。
//!
//! 为什么它们没跟着 `interact.query` 搬去 ECS：这三支要写 UI 的覆盖层文本（`UiState.texts`
//! 的 `__interact_prompt`），而 ECS 绑定不能反向依赖 UI 域。文案本身存在
//! `EntityState.interact_prompt`（ECS 账本），屏幕文字这一笔留在本层——两个句柄都在手边。
//!
//! 装配上仍是一张 `interact` 表：`rustlua_ecs::entity_lua` 填 `query`，本文件补另外三个键。

use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::UiText;
use mlua::Result as LuaResult;
use mlua::{Lua, Table};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn register(
    lua: &Lua,
    interact: Table,
    state: Rc<RefCell<ScriptState>>,
) -> LuaResult<()> {
    let s = Rc::clone(&state);
    interact.set(
        "prompt",
        lua.create_function(move |_, text: String| {
            let state = s.lua_borrow()?;
            let entities = state.entity_state.clone();
            entities.borrow_mut()?.interact_prompt = Some(text.clone());
            let ui = state.ui_state.clone();
            ui.borrow_mut()?.texts_mut().insert(
                "__interact_prompt".to_owned(),
                UiText {
                    text,
                    visible: true,
                    position: [16.0, 48.0],
                    scale: 1.0,
                    font_size: Some(18.0),
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "clear_prompt",
        lua.create_function(move |_, ()| {
            let state = s.lua_borrow()?;
            let entities = state.entity_state.clone();
            entities.borrow_mut()?.interact_prompt = None;
            let ui = state.ui_state.clone();
            ui.borrow_mut()?.texts_mut().remove("__interact_prompt");
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    interact.set(
        "get_prompt",
        lua.create_function(move |_, ()| {
            let entities = s.lua_borrow()?.entity_state.clone();
            Ok(entities.borrow()?.interact_prompt.clone())
        })?,
    )?;
    Ok(())
}
