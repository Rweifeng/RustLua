//! 物理模拟域的状态单元（P1 ⑤-9）：2D/3D 世界 + 分阶段回写 + 调试绘制请求。
//!
//! 这些字段原先内联在 `rustlua-scripting` 的 `ScriptState` 里（`docs/60` §4.1）。搬到
//! `rustlua-physics` 的理由与相机域同形：**推进规则与消费时机归物理**，VM 宿主只保管一份
//! 句柄。宿主侧方法（`step_physics` / `writeback_physics` / `sync_kinematic_bodies` …）
//! 签名一字未改，内部改成 `Handle::with` / `with_mut`。
//!
//! 两条刻意的保留：
//! - **不合并 2D/3D 的回写队列**：`Physics` 阶段只推进、`Writeback` 阶段才写 ECS，两个队列
//!   分开是为了让"2D 的所有推进都发生在 3D 回写之前"这条顺序不被注册顺序左右。
//! - **调试绘制请求按维数分开**（`collider_debug_request` / `_3d`）：混合场景里"只画 2D
//!   碰撞体"或"只画 3D"都是常见需求，合成一个布尔会丢掉这个区分。

use crate::physics::{ColliderDebugShape, PhysicsContact, PhysicsWorld};
use crate::physics3d::{ColliderDebugShape3d, PhysicsWorld3d};
use rustlua_ecs::entity::EntityId;
use rustlua_lua_api::Handle;
use rustlua_math::math2d::Vec2;
use rustlua_math::math3d::Vec3;

/// 物理域的全部可变状态。
#[derive(Debug, Default)]
pub struct PhysicsState {
    world2d: PhysicsWorld,
    world3d: PhysicsWorld3d,
    /// `Physics` 阶段推进后被移动的动态刚体，等 `Writeback` 阶段再写进 ECS。
    pending_writeback_2d: Vec<(EntityId, Vec2)>,
    pending_writeback_3d: Vec<(EntityId, Vec3)>,
    /// collider 调试绘制的**请求值**。绘制开关属于宿主（渲染在那里发生），这里只登记
    /// Lua 的请求、由宿主每帧取走，与 `camera2d` 的请求/消费模式一致。
    collider_debug_request: Option<bool>,
    /// 3D 侧的同一条请求通道，由 `physics3d.debug_draw(...)` 登记。
    collider_debug_request_3d: Option<bool>,
}

impl PhysicsState {
    /// 建一个带名字的句柄。名字只用于重入报错（`physics is already borrowed ...`）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "physics")
    }

    #[must_use]
    pub fn world2d(&self) -> &PhysicsWorld {
        &self.world2d
    }

    pub fn world2d_mut(&mut self) -> &mut PhysicsWorld {
        &mut self.world2d
    }

    /// E03-3：取走本帧给实体脚本的碰撞事件（2D + 3D）。与 Lua `physics.events()` 的
    /// 队列相互独立，因此"转发给 on_collision"不会把事件从 Lua 侧抢走。
    #[must_use]
    pub fn drain_event_contacts(&mut self) -> Vec<PhysicsContact> {
        let mut contacts = self.world2d_mut().drain_event_contacts();
        contacts.extend(self.world3d_mut().drain_event_contacts());
        contacts
    }

    #[must_use]
    pub fn world3d(&self) -> &PhysicsWorld3d {
        &self.world3d
    }

    pub fn world3d_mut(&mut self) -> &mut PhysicsWorld3d {
        &mut self.world3d
    }

    /// 暂存 2D 一步的移动结果（`step` 的返回值），等 `Writeback` 阶段消费。
    pub fn set_pending_writeback_2d(&mut self, moved: Vec<(EntityId, Vec2)>) {
        self.pending_writeback_2d = moved;
    }

    /// 取走 2D 的待回写项（取走即清空）。
    #[must_use]
    pub fn take_pending_writeback_2d(&mut self) -> Vec<(EntityId, Vec2)> {
        std::mem::take(&mut self.pending_writeback_2d)
    }

    pub fn set_pending_writeback_3d(&mut self, moved: Vec<(EntityId, Vec3)>) {
        self.pending_writeback_3d = moved;
    }

    #[must_use]
    pub fn take_pending_writeback_3d(&mut self) -> Vec<(EntityId, Vec3)> {
        std::mem::take(&mut self.pending_writeback_3d)
    }

    /// 丢弃两份待回写（authority = Script 时用）。
    pub fn clear_pending_writebacks(&mut self) {
        self.pending_writeback_2d.clear();
        self.pending_writeback_3d.clear();
    }

    /// 登记一次 2D 调试绘制请求（`physics.debug_draw`），返回登记的值。
    pub fn request_debug_draw(&mut self, enabled: bool) -> bool {
        self.collider_debug_request = Some(enabled);
        enabled
    }

    /// 取走 2D 调试绘制请求（取走即清空：请求只在被消费的那一帧生效）。
    #[must_use]
    pub fn take_debug_request(&mut self) -> Option<bool> {
        self.collider_debug_request.take()
    }

    pub fn request_debug_draw_3d(&mut self, enabled: bool) -> bool {
        self.collider_debug_request_3d = Some(enabled);
        enabled
    }

    #[must_use]
    pub fn take_debug_request_3d(&mut self) -> Option<bool> {
        self.collider_debug_request_3d.take()
    }

    /// 当前所有 2D 碰撞体的姿态与形状，供宿主做 collider 调试绘制。
    #[must_use]
    pub fn debug_shapes(&self) -> Vec<ColliderDebugShape> {
        self.world2d.debug_shapes()
    }

    #[must_use]
    pub fn debug_shapes_3d(&self) -> Vec<ColliderDebugShape3d> {
        self.world3d.debug_shapes()
    }

    /// 场景切换重建两个模拟世界（旧实体句柄全部作废）。
    ///
    /// 待回写队列与调试请求一并清空：它们引用的是**旧场景**的实体与开关，跨场景带着走会让
    /// 新场景的第一帧读到上一张图的移动。
    pub fn clear_for_scene_transition(&mut self) {
        self.world2d = PhysicsWorld::new();
        self.world3d = PhysicsWorld3d::new();
        self.pending_writeback_2d.clear();
        self.pending_writeback_3d.clear();
        self.collider_debug_request = None;
        self.collider_debug_request_3d = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_names_itself_for_reentrancy_errors() {
        let handle = PhysicsState::handle();
        let _guard = handle.borrow().unwrap();
        let message = handle.borrow_mut().unwrap_err().to_string();
        assert!(message.contains("physics"), "{message}");
        assert!(message.contains("already borrowed"), "{message}");
    }

    /// 回写队列是"取走即清空"：第二次拿到的必须是空，否则同一批刚体会被写两次。
    #[test]
    fn pending_writeback_is_taken_once() {
        let mut state = PhysicsState::default();
        let entity = EntityId::from_raw(1, 1);
        state.set_pending_writeback_2d(vec![(entity, Vec2::new(3.0, 4.0))]);
        state.set_pending_writeback_3d(vec![(entity, Vec3::new(1.0, 2.0, 3.0))]);
        assert_eq!(state.take_pending_writeback_2d().len(), 1);
        assert!(state.take_pending_writeback_2d().is_empty());
        assert_eq!(state.take_pending_writeback_3d().len(), 1);
        assert!(state.take_pending_writeback_3d().is_empty());
    }

    /// 两个维数的调试请求互不影响（混合场景里"只画 2D"是合法需求）。
    #[test]
    fn debug_requests_are_per_dimension_and_one_shot() {
        let mut state = PhysicsState::default();
        assert!(state.request_debug_draw(true), "请求应原样回读");
        assert_eq!(state.take_debug_request(), Some(true));
        assert_eq!(state.take_debug_request(), None, "取走即清空");
        assert_eq!(state.take_debug_request_3d(), None, "2D 的请求不该点亮 3D");
        state.request_debug_draw_3d(false);
        assert_eq!(state.take_debug_request_3d(), Some(false));
    }

    /// 切场景要把两个世界与两条队列一起换掉（旧实体句柄作废）。
    #[test]
    fn scene_transition_resets_worlds_and_queues() {
        let mut state = PhysicsState::default();
        let entity = EntityId::from_raw(2, 1);
        state.set_pending_writeback_2d(vec![(entity, Vec2::new(1.0, 1.0))]);
        state.request_debug_draw(true);
        state.clear_for_scene_transition();
        assert!(state.take_pending_writeback_2d().is_empty());
        assert!(state.take_pending_writeback_3d().is_empty());
        assert_eq!(state.take_debug_request(), None);
        assert!(state.world2d().debug_shapes().is_empty());
        assert!(state.world3d().debug_shapes().is_empty());
    }
}
