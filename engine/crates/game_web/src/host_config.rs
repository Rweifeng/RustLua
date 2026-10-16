//! 统一宿主的帧时钟配置（P1 第二片）。
//!
//! ## 为什么这一层单独存在
//!
//! 抽取前两个外壳各自持有一份时钟：浏览器 `gpu_host` 用三个模块常量
//! （`SIM_UPDATES_PER_SECOND` / `SIM_MAX_CATCH_UP_STEPS` / `SIM_MAX_FRAME_SECONDS`）造一次，
//! 微信 `host_runtime` 用 `engine_host_begin` 的 options 再造一次。**策略本身**早已在
//! [`rustlua_infra::frame_clock`] 里只有一份，但"策略被谁按什么参数装出来"有两个入口——
//! 这正是"两台宿主慢慢漂开"最容易发生的地方（一个改 8 步上限、另一个没改，读数上完全看不出来）。
//!
//! 现在两个外壳都只给一个 [`HostConfig`]，时钟由 [`crate::engine_host::EngineHost`] 造并持有。
//!
//! ## 刻意不加 wasm 门
//!
//! 这一层是纯数据 + 一次 `FrameClock` 构造，没有任何 `wgpu` / `web_sys` 依赖，
//! 所以它能在桌面单测里守住两条**只有在这里才能守**的契约：
//!
//! 1. 默认值与文档写的 `(60, 8, 0.25)` 一致（改了常量就会有测试红，而不是等某台设备行为变了才发现）；
//! 2. **喂 Lua 的步长必须是 f64 现算**。`FramePlan::delta_seconds` 是 f32（`1.0f32/60.0`
//!    = `0.016666668`），而 JS 侧喂给 Lua 的是 `1/60` 的 double（`0.016666666666666666`）——
//!    两者不是同一个数。游戏脚本里那些"每步走 `dt * speed`"的累加会因此慢慢分叉，
//!    而当时长/位移出现小差时，没人会怀疑到 f32 与 f64 的区别上。

use rustlua_infra::frame_clock::{FrameClock, FrameClockError};

/// 一台宿主跑固定步所需的全部时钟参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostConfig {
    /// 每秒固定步数（典型 60）。
    pub updates_per_second: u32,
    /// 单帧最多追几步（防"死亡螺旋"）。
    pub max_catch_up_steps: u32,
    /// 单帧接受的墙钟间隔上限（秒）：切标签页/断点/长 GC 会送来秒级间隔。
    pub max_frame_seconds: f64,
}

impl Default for HostConfig {
    /// `(60, 8, 0.25)`：与 `rustlua_infra::frame_clock` 模块文档里写的"典型宿主参数"同一个数，
    /// 也是两个外壳抽取前的取值。
    fn default() -> Self {
        Self {
            updates_per_second: 60,
            max_catch_up_steps: 8,
            max_frame_seconds: 0.25,
        }
    }
}

impl HostConfig {
    /// 显式给出三个参数（可 const 构造：外壳的模块常量可以直接用它）。
    #[must_use]
    pub const fn new(
        updates_per_second: u32,
        max_catch_up_steps: u32,
        max_frame_seconds: f64,
    ) -> Self {
        Self {
            updates_per_second,
            max_catch_up_steps,
            max_frame_seconds,
        }
    }

    /// 按本配置造一台时钟。**校验只有这一份**（`FrameClock::new` 自己会拒绝非法配置），
    /// 外壳不该各自再判一遍。
    pub fn clock(self) -> Result<FrameClock, FrameClockError> {
        FrameClock::new(
            self.updates_per_second,
            self.max_catch_up_steps,
            self.max_frame_seconds,
        )
    }

    /// 交给 Lua 的固定步长，**按 f64 现算**（见模块文档第 2 条）。
    ///
    /// 不要用 [`FramePlan::delta_seconds`] 去喂 Lua：那条值是 f32，与 JS 侧喂进来的
    /// `1/60` double 不是同一个数。
    ///
    /// [`FramePlan::delta_seconds`]: rustlua_infra::frame_clock::FramePlan::delta_seconds
    #[must_use]
    pub fn fixed_delta_seconds_f64(self) -> f64 {
        1.0 / f64::from(self.updates_per_second)
    }
}

#[cfg(test)]
mod tests {
    use super::HostConfig;

    #[test]
    fn default_matches_the_documented_contract() {
        // 两个外壳抽取前用的就是这三个数；改动必须显式改这里，不能悄悄漂。
        let config = HostConfig::default();
        assert_eq!(config.updates_per_second, 60);
        assert_eq!(config.max_catch_up_steps, 8);
        assert_eq!(config.max_frame_seconds, 0.25);
        // 默认配置必须是能造出时钟的：否则"默认路径"从第一帧起就是坏的。
        assert!(config.clock().is_ok());
    }

    #[test]
    fn lua_step_is_a_f64_and_differs_from_the_f32_plan_value() {
        let config = HostConfig::default();
        // ① 恰好是 `1/60` 的 double（JS 侧喂进来的那个数）。
        assert_eq!(config.fixed_delta_seconds_f64(), 1.0 / 60.0);
        // ② 与 `FrameClock` 报出的 f32 步长**不是同一个数**——这正是"必须现算"的理由。
        //    这条断言就是那个取舍的守卫：谁把实现换成 `f64::from(plan.delta_seconds)`，
        //    这里立刻红。
        let clock_delta = f64::from(config.clock().unwrap().fixed_delta_seconds());
        assert_ne!(
            config.fixed_delta_seconds_f64(),
            clock_delta,
            "f32 步长与 f64 步长相等了：要么 FrameClock 改了口径，要么这里退回了 f32"
        );
    }

    #[test]
    fn nonsense_configs_are_rejected_by_the_one_validator() {
        // 校验不在这里重写：非法配置必须由 `FrameClock::new` 拒绝，且这个错误要能传出去。
        assert!(HostConfig::new(0, 8, 0.25).clock().is_err());
        assert!(HostConfig::new(60, 8, 0.0).clock().is_err());
        assert!(HostConfig::new(60, 8, f64::NAN).clock().is_err());
        // 合法配置照常通过（证明上面三条不是因为"clock() 恒失败"才绿的）。
        assert!(HostConfig::new(30, 4, 0.5).clock().is_ok());
    }
}
