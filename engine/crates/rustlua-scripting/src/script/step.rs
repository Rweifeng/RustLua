//! `LuaRuntime` 的每帧推进：物理步进与回写、运动学同步、粒子、游戏时钟。
//!
//! 2D/3D 一律**先 step 再 writeback**：被移动的动态刚体先暂存，`Writeback` 阶段才写回 ECS
//! `Transform`；authority = Script 时由 Lua 自己拥有位置，宿主必须 `discard_pending_writeback`。

// WP11-USE:BEGIN
use crate::script::LuaRuntime;
use crate::script::domain::ecs::transform::Transform;
use crate::script::domain::ecs::transform::Transform3D;
// WP11-USE:END

impl LuaRuntime {
    /// 物理一步：**只推进模拟**，把被移动的动态刚体暂存起来等 `Writeback` 阶段回写。
    ///
    /// WP7 把"推进"与"回写"拆成两个阶段（`Physics` → `Writeback`），于是 2D 与 3D 的**所有**
    /// 推进都发生在**任何**回写之前——此前 2D 的回写夹在 3D 推进之前，混合 2D/3D 的场景里
    /// "谁先看到谁的结果"取决于注册顺序而不是类型化的阶段。
    pub fn step_physics(&mut self, delta_seconds: f32) {
        // P1 ⑤-13：`entity.destroy` 只记账，刚体在这里统一摘——**先摘再推进**，避免推进
        // 一具已销毁实体的刚体（ECS 绑定不反向依赖物理域，所以两端由宿主接线）。
        self.drain_pending_physics_removals();
        // P1 ⑤-9：物理世界住在 `Handle<PhysicsState>` 里，一步推进与暂存回写在同一把借用内完成。
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| {
            let moved = physics.world2d_mut().step(delta_seconds);
            physics.set_pending_writeback_2d(moved);
        });
    }

    /// 摘掉 `entity.destroy` 记下的刚体（2D + 3D），取走即清。
    fn drain_pending_physics_removals(&mut self) {
        let entities = self.state.borrow().entity_state.clone();
        let pending = entities.with_mut(|entities| entities.take_physics_removals());
        if pending.is_empty() {
            return;
        }
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| {
            for entity in pending {
                let _ = physics.world2d_mut().remove_body(entity);
                let _ = physics.world3d_mut().remove_body(entity);
            }
        });
    }

    /// 2D 物理回写：把上一阶段暂存的动态刚体位置写进 ECS `Transform`。
    pub fn writeback_physics(&mut self) -> usize {
        let physics = self.state.borrow().physics.clone();
        let moved = physics.with_mut(|physics| physics.take_pending_writeback_2d());
        let mut written = 0;
        let state = self.state.borrow_mut();
        for (entity, position) in moved {
            let Ok(mut world) = state.world_mut() else {
                continue;
            };
            if let Ok(Some(transform)) = world.get_mut::<Transform>(entity) {
                transform.position = position;
                written += 1;
            }
        }
        written
    }

    /// 3D 物理一步（只推进，回写见 [`LuaRuntime::writeback_physics3d`]）。
    pub fn step_physics3d(&mut self, delta_seconds: f32) {
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| {
            let moved = physics.world3d_mut().step(delta_seconds);
            physics.set_pending_writeback_3d(moved);
        });
    }

    /// 3D 物理回写。
    pub fn writeback_physics3d(&mut self) -> usize {
        let physics = self.state.borrow().physics.clone();
        let moved = physics.with_mut(|physics| physics.take_pending_writeback_3d());
        let mut written = 0;
        let state = self.state.borrow_mut();
        for (entity, position) in moved {
            let Ok(mut world) = state.world_mut() else {
                continue;
            };
            if let Ok(Some(transform)) = world.get_mut::<Transform3D>(entity) {
                transform.position = position;
                written += 1;
            }
        }
        written
    }

    /// 丢弃暂存的物理回写（authority = Script 时不需要回写；不清掉会被下一步误当成移动）。
    pub fn discard_pending_writeback(&mut self) {
        let physics = self.state.borrow().physics.clone();
        physics.with_mut(|physics| physics.clear_pending_writebacks());
    }

    /// WP7 静态/运动学同步：物理步**之前**把 ECS 里动态刚体的位置推给模拟世界。
    ///
    /// 只在 authority = [`rustlua_physics::physics::PhysicsAuthority::Script`] 时调用：那种模式下脚本/ECS 是权威，
    /// 若不先同步，物理世界会拿上一帧的位置继续积分（表现为"脚本设了位置但刚体没动"）。
    /// 返回同步的刚体数。
    pub fn sync_kinematic_bodies(&mut self) -> usize {
        let physics = self.state.borrow().physics.clone();
        let world = self.state.borrow().world.clone();
        let Ok(world) = world.borrow() else {
            return 0;
        };
        let Ok(mut physics) = physics.borrow_mut() else {
            return 0;
        };
        // 先收集再改：`kinematic_positions` 借 `&self`，`set_position` 要 `&mut self`。
        let targets: Vec<_> = physics.world2d().kinematic_positions(&world);
        let mut synced = 0;
        for (entity, position) in targets {
            if physics.world2d_mut().set_position(entity, position).is_ok() {
                synced += 1;
            }
        }
        synced
    }

    /// WP7 静态/运动学同步（**3D**，与 2D 同构）：`authority = Script` 时在物理步之前把 ECS 里
    /// 动态刚体的位置推给 3D 模拟世界。此前 3D 侧没有这一步，"脚本设了位置但刚体没动"在 3D 下
    /// 无解（只能改用速度或直接 `set_position`）。返回同步的刚体数。
    pub fn sync_kinematic_bodies_3d(&mut self) -> usize {
        let physics = self.state.borrow().physics.clone();
        let world = self.state.borrow().world.clone();
        let Ok(world) = world.borrow() else {
            return 0;
        };
        let Ok(mut physics) = physics.borrow_mut() else {
            return 0;
        };
        let targets: Vec<_> = physics.world3d().kinematic_positions(&world);
        let mut synced = 0;
        for (entity, position) in targets {
            if physics.world3d_mut().set_position(entity, position).is_ok() {
                synced += 1;
            }
        }
        synced
    }

    /// Advances the particle system by `dt` seconds.
    pub fn step_particles(&mut self, delta_seconds: f32) {
        self.state.borrow_mut().particles.step(delta_seconds);
    }

    /// P1-③：并行推进粒子（`ParticleSystem::step_with_parallelism`）。默认传
    /// `Parallelism::SEQUENTIAL` 即旧行为（逐位一致，回滚档）。
    pub fn step_particles_with_parallelism(
        &mut self,
        delta_seconds: f32,
        parallelism: crate::script::domain::infra::par::Parallelism,
    ) {
        self.state
            .borrow_mut()
            .particles
            .step_with_parallelism(delta_seconds, parallelism);
    }

    /// Advances the game clock using real `delta_seconds` and the Lua-configured
    /// rate (`clock.set_rate`). Paused clocks and zero rates do nothing.
    pub fn step_clock(&mut self, delta_seconds: f32) {
        // P1 ⑤-5：时钟是能力单元，宿主只借它这一小块（不再借整份 `ScriptState`）。
        let clock = self.state.borrow().game_clock.clone();
        let before = clock.with(|clock| clock.day);
        let after = clock.with_mut(|clock| {
            clock.step(delta_seconds);
            clock.day
        });
        // A clock that crosses midnight while auto-advancing fires `on_day_start`
        // so daily settlement (crops/weather) is not skipped (M16-08).
        if after > before
            && let Ok(function) = self.lua.globals().get::<mlua::Function>("on_day_start")
            && let Err(error) = function.call::<()>(())
        {
            tracing::warn!(target: "script", error = %error, "on_day_start callback failed");
        }
    }
}
