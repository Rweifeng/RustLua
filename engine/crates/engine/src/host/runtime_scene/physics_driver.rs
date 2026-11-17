//! 宿主侧的物理驱动（`docs/62` P1 依赖倒置的落点之一）。
//!
//! 物理的**绑定**住在 `rustlua-physics`，**能力单元**由 [`crate::host::script_packs`] 建，
//! 这里负责宿主每帧的推进：2D/3D 先 step 再 writeback，`authority = Script` 时先同步
//! 运动学刚体、后丢弃回写；`entity.destroy` 记下的刚体移除在推进前统一摘掉。
//!
//! 之所以整段搬到宿主：`rustlua-scripting` 只要还留着 `step_physics` / `writeback_physics`
//! 这些方法，`ScriptState` 就必须钉着 `Handle<PhysicsState>`，Cargo.toml 那条边就删不掉。
//! 搬过来之后脚本层对物理域零引用，而调用方只把 `scene.script.step_physics(..)` 改成
//! `scene.step_physics(..)`——行为与阶段顺序一字未动。

use super::RuntimeScene;
use rustlua_ecs::transform::{Transform, Transform3D};
use rustlua_scripting::script::{EntityEvent, LuaBridgeError};
use rustlua_physics::physics::ColliderDebugShape;
use rustlua_physics::physics3d::ColliderDebugShape3d;

impl RuntimeScene {
    /// 物理一步：**只推进模拟**，把被移动的动态刚体暂存起来等 `Writeback` 阶段回写。
    ///
    /// 2D 与 3D 的所有推进都发生在任何回写之前——否则混合 2D/3D 的场景里"谁先看到谁的
    /// 结果"取决于注册顺序而不是类型化的阶段。
    pub fn step_physics(&self, delta_seconds: f32) {
        // `entity.destroy` 只记账，刚体在这里统一摘——先摘再推进，避免推进一具已销毁
        // 实体的刚体。ECS 绑定不反向依赖物理域，所以两端由宿主接线。
        self.drain_pending_physics_removals();
        self.physics_state.with_mut(|physics| {
            let moved = physics.world2d_mut().step(delta_seconds);
            physics.set_pending_writeback_2d(moved);
        });
    }

    /// 摘掉 `entity.destroy` 记下的刚体（2D + 3D），取走即清。
    fn drain_pending_physics_removals(&self) {
        let entities = self.script.entity_state_handle();
        let pending = entities.with_mut(|entities| entities.take_physics_removals());
        if pending.is_empty() {
            return;
        }
        self.physics_state.with_mut(|physics| {
            for entity in pending {
                let _ = physics.world2d_mut().remove_body(entity);
                let _ = physics.world3d_mut().remove_body(entity);
            }
        });
    }

    /// 2D 物理回写：把上一阶段暂存的动态刚体位置写进 ECS `Transform`。
    pub fn writeback_physics(&mut self) -> usize {
        let moved = self
            .physics_state
            .with_mut(|physics| physics.take_pending_writeback_2d());
        let mut written = 0;
        for (entity, position) in moved {
            self.script.with_world_mut(|world| {
                if let Ok(Some(transform)) = world.get_mut::<Transform>(entity) {
                    transform.position = position;
                    written += 1;
                }
            });
        }
        written
    }

    /// 3D 物理一步（只推进，回写见 [`RuntimeScene::writeback_physics3d`]）。
    pub fn step_physics3d(&self, delta_seconds: f32) {
        self.physics_state.with_mut(|physics| {
            let moved = physics.world3d_mut().step(delta_seconds);
            physics.set_pending_writeback_3d(moved);
        });
    }

    /// 3D 物理回写。
    pub fn writeback_physics3d(&mut self) -> usize {
        let moved = self
            .physics_state
            .with_mut(|physics| physics.take_pending_writeback_3d());
        let mut written = 0;
        for (entity, position) in moved {
            self.script.with_world_mut(|world| {
                if let Ok(Some(transform)) = world.get_mut::<Transform3D>(entity) {
                    transform.position = position;
                    written += 1;
                }
            });
        }
        written
    }

    /// 丢弃暂存的物理回写（authority = Script 时不需要回写；不清掉会被下一步误当成移动）。
    pub fn discard_pending_writeback(&self) {
        self.physics_state
            .with_mut(|physics| physics.clear_pending_writebacks());
    }

    /// 静态/运动学同步：物理步**之前**把 ECS 里动态刚体的位置推给模拟世界。
    /// 只在 authority = Script 时调用，否则物理世界会拿上一帧的位置继续积分。
    pub fn sync_kinematic_bodies(&self) -> usize {
        let physics = self.physics_state.clone();
        let mut synced = 0;
        self.script.with_world(|world| {
            // 先收集再改：`kinematic_positions` 借 `&self`，`set_position` 要 `&mut self`。
            let targets: Vec<_> = physics.with(|p| p.world2d().kinematic_positions(world));
            for (entity, position) in targets {
                if physics.with_mut(|p| p.world2d_mut().set_position(entity, position).is_ok()) {
                    synced += 1;
                }
            }
        });
        synced
    }

    /// 静态/运动学同步（**3D**，与 2D 同构）。
    pub fn sync_kinematic_bodies_3d(&self) -> usize {
        let physics = self.physics_state.clone();
        let mut synced = 0;
        self.script.with_world(|world| {
            let targets: Vec<_> = physics.with(|p| p.world3d().kinematic_positions(world));
            for (entity, position) in targets {
                if physics.with_mut(|p| p.world3d_mut().set_position(entity, position).is_ok()) {
                    synced += 1;
                }
            }
        });
        synced
    }

    /// 把本帧物理接触转发给双方实体的 `on_collision`（只投 `started` 边沿）。
    /// 接触来自物理域单独的事件队列，Lua `physics.events()` 仍能拿到同一批事件。
    pub fn dispatch_collision_contacts(&self) -> Result<usize, LuaBridgeError> {
        let contacts = self
            .physics_state
            .with_mut(|physics| physics.drain_event_contacts());
        let mut delivered = 0;
        for contact in contacts.into_iter().filter(|contact| contact.started) {
            if self
                .script
                .dispatch_entity_event(contact.a, EntityEvent::Collision { other: contact.b })?
            {
                delivered += 1;
            }
            if contact.a != contact.b
                && self
                    .script
                    .dispatch_entity_event(contact.b, EntityEvent::Collision { other: contact.a })?
            {
                delivered += 1;
            }
        }
        Ok(delivered)
    }

    /// 取走 Lua 通过 `physics.debug_draw(...)` 登记的请求（取走即清空）。
    pub fn take_collider_debug_request(&self) -> Option<bool> {
        self.physics_state.with_mut(|p| p.take_debug_request())
    }

    /// 取走 Lua 通过 `physics3d.debug_draw(...)` 登记的 3D 请求。
    pub fn take_collider_debug_request_3d(&self) -> Option<bool> {
        self.physics_state.with_mut(|p| p.take_debug_request_3d())
    }

    /// 当前 2D 物理世界里的碰撞体姿态与形状，供调试绘制。
    pub fn collider_debug_shapes(&self) -> Vec<ColliderDebugShape> {
        self.physics_state.with(|p| p.debug_shapes())
    }

    /// 当前 3D 物理世界里的碰撞体姿态与形状。
    pub fn collider_debug_shapes_3d(&self) -> Vec<ColliderDebugShape3d> {
        self.physics_state.with(|p| p.debug_shapes_3d())
    }
}
