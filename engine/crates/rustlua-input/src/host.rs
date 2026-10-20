//! 输入域的**宿主门面**：宿主每帧喂输入、读动作映射、取震动请求都走这里。
//!
//! 为什么需要它而不是让宿主直接拿 `Handle<InputState>`：宿主侧有 5 个动作（喂快照要继承虚拟键、
//! 推导航手感、读诊断、连发判定、取震动），每个都带一点输入域语义。放在这里，宿主代码只认
//! `HostInput`；`Handle` 保持"纯借用"，不掺业务。
//!
//! 与 Lua 侧共享同一份状态（`Handle` 是共享单元）：`host.set_input(...)` 之后，
//! 脚本里的 `input.is_down` 立刻读到新快照。

use std::cell::Cell;
use std::rc::Rc;

use rustlua_lua_api::{Handle, Registry};

use crate::input::{ActionMap, InputSnapshot};
use crate::nav::NavSettings;
use crate::state::InputState;

/// 宿主侧输入门面（每个 Lua 运行时一份；与绑定共享状态）。
///
/// Clone 只是克隆句柄与共享单元，**不复制状态**：两处克隆指向同一份 [InputState]。
#[derive(Clone)]
pub struct HostInput {
    state: Handle<InputState>,
    /// 本步固定步长（秒）：`LuaRuntime::set_delta` 写、[`Self::set_input`] 读来推进逻辑时间。
    delta_seconds: Rc<Cell<f32>>,
    /// 指针位置（视口坐标）：`camera3d.mouse_ray` 之类读它做屏幕→世界换算。
    pointer: Rc<Cell<Option<(f32, f32)>>>,
}

impl HostInput {
    /// 从注册表取共享单元（注册表持有它们，`LuaRuntime::set_delta` 也写同一份）。
    #[must_use]
    pub fn new(registry: &Registry) -> Self {
        Self {
            state: Handle::new(InputState::default(), "input"),
            delta_seconds: registry.delta_cell(),
            pointer: registry.pointer_cell(),
        }
    }

    /// 与 Lua 侧共享的状态句柄（绑定安装器要用它）。
    #[must_use]
    pub fn state(&self) -> Handle<InputState> {
        self.state.clone()
    }

    /// 喂入本步的输入快照（每个固定步一次）。
    ///
    /// 两件事必须与旧实现逐字一致：
    /// 1. **逻辑时间按每个固定步累加**——catch-up 帧（一帧多步）也走同一条时间线，
    ///    连发节奏与帧率无关；
    /// 2. **虚拟键跨帧保持**——脚本只在抬起时注入 `false`，整份替换会把上一帧的注入清空，
    ///    摇杆/虚拟按钮就永远读不到按住，所以把旧虚拟键并入新快照。
    pub fn set_input(&self, snapshot: InputSnapshot) {
        let delta = self.delta_seconds.get();
        let mut next = snapshot;
        self.state.with_mut(|state| {
            state.advance(delta);
            next.inherit_virtual_keys(&state.snapshot);
            state.snapshot = next;
        });
        // 指针位置顺带发布给其它领域绑定（`camera3d.mouse_ray`）：它不属于任何一边的
        // 私有状态，放注册表里让写入方与读取方各取所需。
        let (x, y) = self.state.with(|state| state.snapshot.cursor_position());
        self.pointer.set(Some((x as f32, y as f32)));
    }

    /// 当前命名动作映射（只读快照，与脚本侧同一份）。
    #[must_use]
    pub fn action_map(&self) -> ActionMap {
        self.state.with(|state| state.action_map.clone())
    }

    /// 注入命名动作映射（改键面板 / 按设置文件启动）。
    pub fn set_action_map(&self, map: ActionMap) {
        self.state.with_mut(|state| state.action_map = map);
    }

    /// 输入层诊断（最新在后）。
    #[must_use]
    pub fn input_diagnostics(&self) -> Vec<String> {
        self.state.with(|state| state.diagnostics.clone())
    }

    /// 当前菜单导航手感（引擎每帧读它同步设备层阈值）。
    #[must_use]
    pub fn nav_settings(&self) -> NavSettings {
        self.state.with(|state| state.nav)
    }

    /// 推导航手感（设备层阈值与连发节奏同源）。
    pub fn set_nav_settings(&self, nav: NavSettings) {
        self.state.with_mut(|state| state.nav = nav);
    }

    /// 时间制连发判定（与 Lua `input.action_repeat` 同源实现）。
    pub fn action_repeat(&self, name: &str) -> bool {
        self.state.with_mut(|state| state.repeat_step(name))
    }

    /// 取走震动请求（帧末交给设备层）。
    pub fn take_rumble_request(&self) -> Option<(f32, f32, u32)> {
        self.state.with_mut(|state| state.take_rumble_request())
    }

    /// 指针位置（视口坐标）；未喂过输入时为 `None`。
    #[must_use]
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer.get()
    }

    /// 与 Lua 侧共享状态是否同一份（组合根自检用）。
    #[must_use]
    pub fn shares_state_with(&self, other: &Self) -> bool {
        self.state.same(&other.state)
    }
}
