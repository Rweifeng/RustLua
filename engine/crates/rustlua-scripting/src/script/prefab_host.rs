//! `prefab` 表的组合根实现（P1 ⑤-15）。
//!
//! Lua 表面已搬到 `rustlua_ui::prefab_lua`，这里只实现 [`PrefabHost`]：把每个操作接到
//! `LuaRuntime` 的静态入口上。**组合根留在脚本层是对的**——`instantiate_prefab_state` 要按
//! `&mut ScriptState` 装配 18 类组件，领域 crate 不能反向依赖本层。
//!
//! 错误文案的完整拼装在这里（`prefab.xxx {参数}: {原因}`），因为"原因"来自 `LuaBridgeError`，
//! 只有本层看得到；UI 域那边原样转给 Lua，两边各拼一半会让同一次失败出现两种前缀。

use crate::script::LuaRuntime;
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::domain::ui::prefab_lua::PrefabHost;
use crate::script::values::entity_to_lua;
use std::cell::RefCell;
use std::rc::Rc;

/// 把 Lua 的 `prefab.*` 调用转给运行期。
pub(crate) struct PrefabOps {
    state: Rc<RefCell<ScriptState>>,
}

impl PrefabOps {
    pub(crate) fn new(state: Rc<RefCell<ScriptState>>) -> Self {
        Self { state }
    }
}

impl PrefabHost for PrefabOps {
    fn spawn(&self, path: &str) -> Result<i64, String> {
        // 读文件走 `rustlua_infra::vfs`：三个宿主（PC / WebGL / 微信）的包内读取只有这一条路，
        // 直连 `fs` 在发布包里就是"开发机能跑、真机白屏"（M32 的教训，`data.load` 同口径）。
        let root = {
            let state = self.state.lua_borrow().map_err(|error| error.to_string())?;
            state
                .project_root
                .borrow()
                .map_err(|error| error.to_string())?
                .path()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| "prefab.spawn 需要已加载工程（project_root 未设置）".to_owned())?
        };
        let file = root.join(path);
        let bytes = crate::script::domain::infra::vfs::read(&file)
            .map_err(|error| format!("prefab.spawn {path}: {error}"))?;
        let text = String::from_utf8(bytes)
            .map_err(|error| format!("prefab.spawn {path}: 不是 UTF-8 文本: {error}"))?;
        let prefab = crate::script::domain::scene::prefab::Prefab::from_text(&text)
            .map_err(|error| format!("prefab.spawn {path}: {error}"))?;
        // `from_text` 末尾已经 `validate()`（版本、root 存在、parent 边成树），这里不重复校验。
        let mut state = self
            .state
            .lua_borrow_mut()
            .map_err(|error| error.to_string())?;
        let spawn = LuaRuntime::instantiate_prefab_state(&mut state, &prefab)
            .map_err(|error| format!("prefab.spawn {path}: {error}"))?;
        let handle = state.next_prefab_handle;
        state.next_prefab_handle += 1;
        state.prefab_spawns.insert(handle, spawn);
        Ok(handle)
    }

    fn despawn(&self, handle: i64) -> Result<bool, String> {
        let mut state = self
            .state
            .lua_borrow_mut()
            .map_err(|error| error.to_string())?;
        let Some(spawn) = state.prefab_spawns.remove(&handle) else {
            return Ok(false);
        };
        LuaRuntime::despawn_prefab_state(&mut state, &spawn)
            .map_err(|error| format!("prefab.despawn {handle}: {error}"))?;
        Ok(true)
    }

    fn root(&self, handle: i64) -> Result<i64, String> {
        let state = self.state.lua_borrow().map_err(|error| error.to_string())?;
        let spawn = state
            .prefab_spawns
            .get(&handle)
            .ok_or_else(|| format!("prefab.root: 没有 handle {handle}"))?;
        Ok(entity_to_lua(spawn.root))
    }

    fn node(&self, handle: i64, name: &str) -> Result<i64, String> {
        let state = self.state.lua_borrow().map_err(|error| error.to_string())?;
        let spawn = state
            .prefab_spawns
            .get(&handle)
            .ok_or_else(|| format!("prefab.node: 没有 handle {handle}"))?;
        let id = spawn
            .names
            .get(name)
            .copied()
            .ok_or_else(|| format!("prefab.node: 实例 {handle} 里没有节点 {name:?}"))?;
        Ok(entity_to_lua(id))
    }

    fn set_text(&self, handle: i64, name: &str, text: &str) -> Result<(), String> {
        let state = self
            .state
            .lua_borrow_mut()
            .map_err(|error| error.to_string())?;
        let id = {
            let spawn = state
                .prefab_spawns
                .get(&handle)
                .ok_or_else(|| format!("prefab.set_text: 没有 handle {handle}"))?;
            spawn
                .names
                .get(name)
                .copied()
                .ok_or_else(|| format!("prefab.set_text: 实例 {handle} 里没有节点 {name:?}"))?
        };
        // 写不进世界（重入借不到）与"节点没有 Text 组件"在这里合并成同一句报错：
        // 对脚本来说两者的可行动信息相同——这个节点此刻改不了文字。
        let written = state
            .world_mut()
            .map(|mut world| {
                world
                    .get_mut::<crate::script::domain::ui::ui::UiText>(id)
                    .ok()
                    .flatten()
                    .map(|component| component.text = text.to_owned())
                    .is_some()
            })
            .unwrap_or_default();
        if !written {
            return Err(format!(
                "prefab.set_text: 节点 {name:?} 没有 Text 组件（预设里它是图片/面板/容器）"
            ));
        }
        Ok(())
    }

    fn set_visible(&self, handle: i64, name: &str, visible: bool) -> Result<(), String> {
        let mut state = self
            .state
            .lua_borrow_mut()
            .map_err(|error| error.to_string())?;
        LuaRuntime::set_prefab_node_visible_state(&mut state, handle, name, visible)
            .map_err(|error| format!("prefab.set_visible {handle} {name:?}: {error}"))
    }

    fn set_rect(&self, handle: i64, name: &str, rect: [f32; 4]) -> Result<(), String> {
        let mut state = self
            .state
            .lua_borrow_mut()
            .map_err(|error| error.to_string())?;
        LuaRuntime::set_prefab_node_rect_state(&mut state, handle, name, rect)
            .map_err(|error| format!("prefab.set_rect {handle} {name:?}: {error}"))
    }
}
