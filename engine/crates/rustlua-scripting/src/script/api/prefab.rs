//! `prefab`：运行期把 `.prefab` 实体树实例化进活世界（UI 预设的运行时载体）。
//!
//! 为什么读文件走 `rustlua_infra::vfs` 而不是 `fs`：三个宿主（PC / WebGL / 微信）的包内
//! 读取只有 vfs 这一条路（pack 解码在里头），直连 `fs` 在发布包里就是"开发机能跑、真机
//! 白屏"（M32 的教训，`data.load` 同口径）。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use crate::script::values::entity_to_lua;
use mlua::Lua;
use mlua::Result as LuaResult;
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let prefab = lua.create_table()?;

    let s = Rc::clone(&state);
    prefab.set(
        "spawn",
        lua.create_function(move |_, path: String| -> LuaResult<i64> {
            let root = {
                let state = s.lua_borrow()?;
                state.project_root.clone().ok_or_else(|| {
                    api_error("prefab.spawn 需要已加载工程（project_root 未设置）")
                })?
            };
            let file = root.join(&path);
            let bytes = rustlua_infra::vfs::read(&file)
                .map_err(|error| api_error(format!("prefab.spawn {path}: {error}")))?;
            let text = String::from_utf8(bytes).map_err(|error| {
                api_error(format!("prefab.spawn {path}: 不是 UTF-8 文本: {error}"))
            })?;
            let prefab = rustlua_scene::prefab::Prefab::from_text(&text)
                .map_err(|error| api_error(format!("prefab.spawn {path}: {error}")))?;
            // `from_text` 末尾已经 `validate()`（版本、root 存在、parent 边成树），
            // 这里不再重复校验一次。
            let mut state = s.lua_borrow_mut()?;
            let spawn = crate::script::LuaRuntime::instantiate_prefab_state(&mut state, &prefab)
                .map_err(|error| api_error(format!("prefab.spawn {path}: {error}")))?;
            let handle = state.next_prefab_handle;
            state.next_prefab_handle += 1;
            state.prefab_spawns.insert(handle, spawn);
            Ok(handle)
        })?,
    )?;

    let s = Rc::clone(&state);
    prefab.set(
        "despawn",
        lua.create_function(move |_, handle: i64| -> LuaResult<bool> {
            let mut state = s.lua_borrow_mut()?;
            let Some(spawn) = state.prefab_spawns.remove(&handle) else {
                return Ok(false);
            };
            crate::script::LuaRuntime::despawn_prefab_state(&mut state, &spawn)
                .map_err(|error| api_error(format!("prefab.despawn {handle}: {error}")))?;
            Ok(true)
        })?,
    )?;

    let s = Rc::clone(&state);
    prefab.set(
        "root",
        lua.create_function(move |_, handle: i64| -> LuaResult<i64> {
            let state = s.lua_borrow()?;
            let spawn = state
                .prefab_spawns
                .get(&handle)
                .ok_or_else(|| api_error(format!("prefab.root: 没有 handle {handle}")))?;
            Ok(entity_to_lua(spawn.root))
        })?,
    )?;

    let s = Rc::clone(&state);
    prefab.set(
        "node",
        lua.create_function(move |_, (handle, name): (i64, String)| -> LuaResult<i64> {
            let state = s.lua_borrow()?;
            let spawn = state
                .prefab_spawns
                .get(&handle)
                .ok_or_else(|| api_error(format!("prefab.node: 没有 handle {handle}")))?;
            let id = spawn.names.get(&name).copied().ok_or_else(|| {
                api_error(format!("prefab.node: 实例 {handle} 里没有节点 {name:?}"))
            })?;
            Ok(entity_to_lua(id))
        })?,
    )?;

    let s = Rc::clone(&state);
    prefab.set(
        "set_text",
        // 写**场景** `Text` 组件的文字（不是动态层的 `ui.text`）。这一条是"版面进预设、
        // 取值留给脚本"的另一半：没有它，实例化出来的标签在运行期一个字都改不了，
        // 于是 title_menu 只能继续"场景层画一遍、脚本层盖一遍"（现状：它把场景标签
        // 用 `ui.scene_text_visible(name,false)` 全数隐藏，正是这个缺口的症状）。
        // 字体/字号/颜色刻意不改——那些属于版面，改动版面应该改预设文件。
        lua.create_function(
            move |_, (handle, name, text): (i64, String, String)| -> LuaResult<()> {
                let mut state = s.lua_borrow_mut()?;
                let id = {
                    let spawn = state.prefab_spawns.get(&handle).ok_or_else(|| {
                        api_error(format!("prefab.set_text: 没有 handle {handle}"))
                    })?;
                    spawn.names.get(&name).copied().ok_or_else(|| {
                        api_error(format!(
                            "prefab.set_text: 实例 {handle} 里没有节点 {name:?}"
                        ))
                    })?
                };
                let written = match state.world.get_mut::<rustlua_ui::ui::UiText>(id) {
                    Ok(Some(component)) => {
                        component.text = text;
                        true
                    }
                    _ => false,
                };
                if !written {
                    return Err(api_error(format!(
                        "prefab.set_text: 节点 {name:?} 没有 Text 组件（预设里它是图片/面板/容器）"
                    )));
                }
                Ok(())
            },
        )?,
    )?;

    lua.globals().set("prefab", prefab)?;
    Ok(())
}
