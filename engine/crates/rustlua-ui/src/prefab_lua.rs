//! `prefab` 表的 Lua 表面（P1 ⑤-15 从脚本层搬来）。
//!
//! **为什么是"表面 + 端口"而不是整块搬迁**：这张表的七个函数几乎全是转手——真正干活的
//! `LuaRuntime::instantiate_prefab_state` / `despawn_prefab_state` / `set_prefab_node_*_state`
//! 住在 `scene_spawn.rs`（**924 行**），按 `&mut ScriptState` 往世界里装 18 类组件。那是**组合根**，
//! 不是绑定：把它搬进领域 crate 等于让领域 crate 反向依赖脚本层。所以这里只留 Lua 该有的部分
//! （参数形状、错误文案、表结构），实现经 [`PrefabHost`] 由组合根注入。
//!
//! 收益不只是行数：`prefab` 的表面从此可以**脱开**真实世界做契约测试（见本文件测试里的
//! `RecordingHost`）——此前要验证"`prefab.node` 对未知 handle 报什么错"，得先造一份 `.prefab`
//! 文件并跑完整实例化。

use mlua::Lua;
use mlua::Result as LuaResult;
use rustlua_lua_api::api_error;
use std::rc::Rc;

/// 组合根实现这组操作，UI 域只负责把它们接到 Lua 上。
///
/// 约定：`Err(String)` 里是**完整的** Lua 错误文案（含 `prefab.xxx` 前缀与参数细节）。这样
/// 迁移前后脚本看到的字串逐字相同，不必在两侧各拼一半。
pub trait PrefabHost {
    /// 从工程相对路径读 `.prefab` 并实例化，返回实例 handle。
    fn spawn(&self, path: &str) -> Result<i64, String>;
    /// 销毁实例；handle 不存在时返回 `false`（幂等，不报错）。
    fn despawn(&self, handle: i64) -> Result<bool, String>;
    /// 实例的根实体（Lua 侧 id）。
    fn root(&self, handle: i64) -> Result<i64, String>;
    /// 实例里某个名字的节点（Lua 侧 id）。
    fn node(&self, handle: i64, name: &str) -> Result<i64, String>;
    /// 改节点的场景 `Text` 组件文字（不动字体/字号/颜色）。
    fn set_text(&self, handle: i64, name: &str, text: &str) -> Result<(), String>;
    /// 显示/隐藏节点及其整棵子树。
    fn set_visible(&self, handle: i64, name: &str, visible: bool) -> Result<(), String>;
    /// 改节点 `UiLayout` 的四边（anchors 沿用预设里 authored 的值）。
    fn set_rect(&self, handle: i64, name: &str, rect: [f32; 4]) -> Result<(), String>;
}

/// 把七个函数装进 `prefab` 表并设为全局。
pub fn install(lua: &Lua, host: Rc<dyn PrefabHost>) -> LuaResult<()> {
    let prefab = lua.create_table()?;

    let h = Rc::clone(&host);
    prefab.set(
        "spawn",
        lua.create_function(move |_, path: String| -> LuaResult<i64> {
            h.spawn(&path).map_err(api_error)
        })?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "despawn",
        lua.create_function(move |_, handle: i64| -> LuaResult<bool> {
            h.despawn(handle).map_err(api_error)
        })?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "root",
        lua.create_function(move |_, handle: i64| -> LuaResult<i64> {
            h.root(handle).map_err(api_error)
        })?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "node",
        lua.create_function(move |_, (handle, name): (i64, String)| -> LuaResult<i64> {
            h.node(handle, &name).map_err(api_error)
        })?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "set_text",
        // 写**场景** `Text` 组件的文字（不是动态层的 `ui.text`）。这一条是"版面进预设、
        // 取值留给脚本"的另一半：没有它，实例化出来的标签在运行期一个字都改不了。
        // 字体/字号/颜色刻意不改——那些属于版面，改动版面应该改预设文件。
        lua.create_function(
            move |_, (handle, name, text): (i64, String, String)| -> LuaResult<()> {
                h.set_text(handle, &name, &text).map_err(api_error)
            },
        )?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "set_visible",
        // 隐藏/显示预设里的一个节点及其**整棵子树**（与 Unity `GameObject.SetActive` 同语义：
        // 子的显隐是相对父的）。走的是"这个节点不建布局项"这条路，因此画面与点击同时消失——
        // 不会出现"看不见的按钮仍能被点中"。实体本身不销毁：显示回来不需要重设取值。
        lua.create_function(
            move |_, (handle, name, visible): (i64, String, bool)| -> LuaResult<()> {
                h.set_visible(handle, &name, visible).map_err(api_error)
            },
        )?,
    )?;

    let h = Rc::clone(&host);
    prefab.set(
        "set_rect",
        // 写一个节点 `UiLayout` 的四边（offsets），anchors 沿用预设里 authored 的值：
        // 口径是"四边 = 父矩形 × anchor + offset"，所以默认锚点下这四个数就是父坐标里的像素边。
        // 改一处，整棵子树跟着位移（宿主按父链解算），不必逐个子节点通知。
        lua.create_function(
            move |_,
                  (handle, name, left, top, right, bottom): (i64, String, f32, f32, f32, f32)|
                  -> LuaResult<()> {
                h.set_rect(handle, &name, [left, top, right, bottom])
                    .map_err(api_error)
            },
        )?,
    )?;

    lua.globals().set("prefab", prefab)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::{Table, Value};
    use std::cell::RefCell;

    /// 记录调用的假组合根：证明这一层只做形状与文案，不碰世界。
    #[derive(Default)]
    struct RecordingHost {
        calls: RefCell<Vec<String>>,
        fail_spawn: bool,
        /// 实例是否已被销毁（`despawn` 的幂等语义要靠它，跟真实实现一样"第二次返回 false"）。
        despawned: RefCell<bool>,
    }

    impl PrefabHost for RecordingHost {
        fn spawn(&self, path: &str) -> Result<i64, String> {
            self.calls.borrow_mut().push(format!("spawn {path}"));
            if self.fail_spawn {
                return Err(format!("prefab.spawn {path}: 文件不存在"));
            }
            Ok(41)
        }

        fn despawn(&self, handle: i64) -> Result<bool, String> {
            self.calls.borrow_mut().push(format!("despawn {handle}"));
            if handle != 41 || *self.despawned.borrow() {
                return Ok(false);
            }
            *self.despawned.borrow_mut() = true;
            Ok(true)
        }

        fn root(&self, handle: i64) -> Result<i64, String> {
            if handle != 41 {
                return Err(format!("prefab.root: 没有 handle {handle}"));
            }
            Ok(7)
        }

        fn node(&self, handle: i64, name: &str) -> Result<i64, String> {
            if handle != 41 {
                return Err(format!("prefab.node: 没有 handle {handle}"));
            }
            Ok(if name == "title" { 8 } else { 9 })
        }

        fn set_text(&self, handle: i64, name: &str, text: &str) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("set_text {handle} {name} {text}"));
            Ok(())
        }

        fn set_visible(&self, _handle: i64, _name: &str, visible: bool) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("set_visible {visible}"));
            Ok(())
        }

        fn set_rect(&self, _handle: i64, _name: &str, rect: [f32; 4]) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("set_rect {rect:?}"));
            Ok(())
        }
    }

    fn install_recording() -> (Lua, Rc<RecordingHost>) {
        let lua = Lua::new();
        let host = Rc::new(RecordingHost::default());
        install(&lua, host.clone()).expect("install");
        (lua, host)
    }

    #[test]
    fn every_prefab_function_is_installed() {
        let (lua, _) = install_recording();
        let table: Table = lua.globals().get("prefab").expect("prefab table");
        let mut names: Vec<String> = table
            .pairs::<String, Value>()
            .map(|pair| pair.expect("pair").0)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "despawn",
                "node",
                "root",
                "set_rect",
                "set_text",
                "set_visible",
                "spawn"
            ]
        );
    }

    #[test]
    fn calls_are_forwarded_with_their_arguments() {
        let (lua, host) = install_recording();
        let handle: i64 = lua
            .load("return prefab.spawn('ui/hud.prefab')")
            .eval()
            .unwrap();
        assert_eq!(handle, 41);
        let root: i64 = lua
            .load(format!("return prefab.root({handle})"))
            .eval()
            .unwrap();
        assert_eq!(root, 7);
        let node: i64 = lua
            .load(format!("return prefab.node({handle}, 'title')"))
            .eval()
            .unwrap();
        assert_eq!(node, 8);
        lua.load(format!("prefab.set_text({handle}, 'title', 'hi')"))
            .exec()
            .unwrap();
        lua.load(format!("prefab.set_rect({handle}, 'row', 1, 2, 3, 4)"))
            .exec()
            .unwrap();
        lua.load(format!("prefab.set_visible({handle}, 'row', false)"))
            .exec()
            .unwrap();
        let calls = host.calls.borrow().clone();
        assert_eq!(
            calls,
            vec![
                "spawn ui/hud.prefab".to_owned(),
                "set_text 41 title hi".to_owned(),
                "set_rect [1.0, 2.0, 3.0, 4.0]".to_owned(),
                "set_visible false".to_owned(),
            ]
        );
    }

    #[test]
    fn host_errors_reach_lua_verbatim() {
        let lua = Lua::new();
        let host = Rc::new(RecordingHost {
            fail_spawn: true,
            ..RecordingHost::default()
        });
        install(&lua, host).unwrap();
        let message = lua
            .load("return prefab.spawn('missing.prefab')")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("prefab.spawn missing.prefab: 文件不存在"),
            "{message}"
        );
        let message = lua
            .load("return prefab.root(999)")
            .exec()
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("prefab.root: 没有 handle 999"),
            "{message}"
        );
    }

    #[test]
    fn despawn_is_idempotent() {
        let (lua, _) = install_recording();
        assert!(
            lua.load("return prefab.despawn(41)")
                .eval::<bool>()
                .unwrap()
        );
        assert!(
            !lua.load("return prefab.despawn(41)")
                .eval::<bool>()
                .unwrap()
        );
    }
}
