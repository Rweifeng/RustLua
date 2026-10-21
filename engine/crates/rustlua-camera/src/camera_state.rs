//! 相机域的**脚本侧状态单元**（P1 ⑤-8）：Lua 的 `camera` / `camera3d` / `camera2d` 三张表
//! 在宿主状态里真正落到的那些字段。
//!
//! 为什么它和 `Camera` 组件分开：组件在 ECS 里（`Handle<World>`），而这里存的是**相机作为
//! "当前取景"的会话态**——哪台相机被显式选中、2D 控制器（跟随/死区/平滑/抖动/请求-回填）、
//! 以及宿主每帧回填的两份**已烘焙视口快照**。这三样都不属于实体，也不该跟着场景实体走。
//!
//! 与 `rustlua-gameplay::clock_state` / `inventory_state` 同一个形状（`Handle<State>` +
//! `install`），因此脚本层的 `ScriptState` 只留一句 `Handle`，业务规则与状态都在本 crate。

use rustlua_ecs::entity::EntityId;
use rustlua_lua_api::Handle;

use crate::camera::{Camera2D, Camera2dController, Camera3D};

/// 相机会话态：选中项 + 2D 控制器 + 每帧烘焙快照。
#[derive(Debug, Default)]
pub struct CameraState {
    /// 脚本显式选中的相机实体（`camera.set_active`）。`None` = 交给宿主的自动选择
    /// （`CameraSelection`）决定。
    active: Option<EntityId>,
    /// 2D 相机控制器：跟随 / 偏移 / 死区 / 平滑 / 抖动 + 请求-回填交接。
    controller: Camera2dController,
    /// 宿主每帧回填的 2D 相机与视口（`camera2d.screen_to_world` 等读它）。
    view_2d: Option<(Camera2D, [f32; 2])>,
    /// 宿主每帧回填的 3D 相机与视口（`camera3d.ray` / `world_to_screen` 读它）。
    view_3d: Option<(Camera3D, [f32; 2])>,
}

impl CameraState {
    /// 装上真名的句柄（重入报错会点名 `camera`）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "camera")
    }

    /// 脚本显式选中的相机实体。
    #[must_use]
    pub fn active(&self) -> Option<EntityId> {
        self.active
    }

    /// 登记/清除脚本的显式选中。生存性校验由调用方（绑定层）负责——它要借世界。
    pub fn set_active(&mut self, active: Option<EntityId>) {
        self.active = active;
    }

    /// 2D 控制器（只读）。
    #[must_use]
    pub fn controller(&self) -> &Camera2dController {
        &self.controller
    }

    /// 2D 控制器（读写）：宿主每帧推进、脚本登记请求都走这里。
    pub fn controller_mut(&mut self) -> &mut Camera2dController {
        &mut self.controller
    }

    /// 宿主回填的 2D 相机快照。
    #[must_use]
    pub fn view_2d(&self) -> Option<(Camera2D, [f32; 2])> {
        self.view_2d
    }

    pub fn set_view_2d(&mut self, camera: Camera2D, viewport: [f32; 2]) {
        self.view_2d = Some((camera, viewport));
    }

    /// 宿主回填的 3D 相机快照。
    #[must_use]
    pub fn view_3d(&self) -> Option<(Camera3D, [f32; 2])> {
        self.view_3d
    }

    pub fn set_view_3d(&mut self, camera: Camera3D, viewport: [f32; 2]) {
        self.view_3d = Some((camera, viewport));
    }

    /// 场景切换：显式选中失效（它指向旧场景的实体），控制器与快照保持原样。
    ///
    /// 保留控制器是刻意的：跟随目标由宿主每帧从 ECS 取，切场景后旧的 follow id 自然查不到
    /// `Transform`、退化成"不跟随时保持当前位置"；而平滑/死区/抖动这些**创作意图**不该因为
    /// 换场景被重置。`view_2d`/`view_3d` 是每帧回填的，留着也不会被用到旧值。
    pub fn clear_for_scene_transition(&mut self) {
        self.active = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_math::math2d::Vec2;

    #[test]
    fn new_state_has_no_active_camera_and_no_baked_view() {
        let state = CameraState::default();
        assert_eq!(state.active(), None);
        assert!(state.view_2d().is_none());
        assert!(state.view_3d().is_none());
        // 控制器默认不跟随、不做平滑。
        assert_eq!(state.controller().follow_target(), None);
        assert_eq!(state.controller().smoothing(), 0.0);
    }

    #[test]
    fn handle_is_named_and_shared_by_clone() {
        let state = CameraState::handle();
        let clone = state.clone();
        state.with_mut(|state| state.set_active(Some(EntityId::from_raw(3, 1))));
        assert_eq!(
            clone.with(|state| state.active()),
            Some(EntityId::from_raw(3, 1)),
            "同一份相机态：句柄副本必须看到写入"
        );
    }

    #[test]
    fn controller_requests_round_trip_through_the_handle() {
        let state = CameraState::handle();
        state.with_mut(|state| {
            state.controller_mut().request_center(Vec2::new(7.0, -2.0));
            state.controller_mut().request_zoom(2.5);
        });
        assert_eq!(
            state.with_mut(|state| state.controller_mut().take_pending_center()),
            Some(Vec2::new(7.0, -2.0))
        );
        assert_eq!(
            state.with_mut(|state| state.controller_mut().take_pending_zoom()),
            Some(2.5)
        );
    }

    #[test]
    fn scene_transition_only_drops_the_active_selection() {
        let mut state = CameraState::default();
        state.set_active(Some(EntityId::from_raw(1, 2)));
        state.controller_mut().set_smoothing(4.0);
        state
            .controller_mut()
            .follow(Some(EntityId::from_raw(1, 2)));
        state.clear_for_scene_transition();
        assert_eq!(state.active(), None, "旧场景的选中必须失效");
        assert_eq!(
            state.controller().smoothing(),
            4.0,
            "平滑系数是创作意图，不随换场景重置"
        );
        assert_eq!(
            state.controller().follow_target(),
            Some(EntityId::from_raw(1, 2)),
            "跟随目标也不在这里清：宿主每帧从 ECS 解析，旧 id 查不到就退化成保持不变"
        );
    }
}
