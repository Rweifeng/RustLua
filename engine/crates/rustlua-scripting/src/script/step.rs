//! `LuaRuntime` 的每帧推进：物理步进与回写、运动学同步、粒子、游戏时钟。
//!
//! 2D/3D 一律**先 step 再 writeback**：被移动的动态刚体先暂存，`Writeback` 阶段才写回 ECS
//! `Transform`；authority = Script 时由 Lua 自己拥有位置，宿主必须 `discard_pending_writeback`。

// WP11-USE:BEGIN
use crate::script::LuaRuntime;
use crate::script::crash;
// `Handle` 也经门面取：`script/` 的生产代码不许直接出现 `rustlua_*` 引用
// （`tests/domain_facade.rs` 的棘轮守着）。
use crate::script::domain::lua_api;
// WP11-USE:END

impl LuaRuntime {
    /// 渲染域的脚本可见状态（P3-R238：宿主用它推进**场景时钟**）。
    ///
    /// 场景时钟是程序化实例运动（实例批次的路径流）的时间基准：宿主推进它，渲染提取与
    /// 脚本读回都按它求值 ⇒ "画出来的姿态"与"读回来的姿态"永远是同一个时刻。
    #[must_use]
    pub fn renderable_state(
        &self,
    ) -> lua_api::Handle<crate::script::domain::render::renderable_lua::RenderableState> {
        self.state.borrow().renderable.clone()
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
        {
            // 这条钩子的错误历来只记日志（不中断时钟）：现场照抓，但标成非致命。
            match crash::call_guarded(&self.lua, function, ()) {
                Ok(Ok(_)) => {}
                Ok(Err(reason)) => {
                    self.finalize_crash(Some("on_day_start"), false);
                    tracing::warn!(target: "script", %reason, "on_day_start callback failed");
                }
                Err(error) => {
                    tracing::warn!(target: "script", %error, "on_day_start callback failed");
                }
            }
        }
    }
}
