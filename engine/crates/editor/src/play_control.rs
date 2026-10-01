//! Play-mode state machine with Pause / Step / Restart (docs/55 `ER-10`).
//!
//! The editor's Play mode used to be a bare enum whose transitions were assigned ad hoc at each
//! call site, with no notion of "paused" and no way to advance exactly one frame. This module
//! owns those semantics in one place so they can be tested without a GPU:
//!
//! * legal transitions only — `pause()` while editing, `step()` while running, `restart()`
//!   while stopped all report [`PlayControlError`] instead of silently doing nothing;
//! * a **frame plan**: fixed-timestep accumulation with a bounded catch-up (the same contract
//!   the preview uses), so a hitch cannot make the game run away;
//! * pause freezes logical time (plan requests zero steps) while a step request runs exactly
//!   the requested number of fixed steps and then freezes again;
//! * failure isolation: `fail()` records the reason and returns to `StartFailed` without
//!   touching counters, so a broken preview cannot corrupt editor state.

/// 每秒固定逻辑步数（与预览/独立宿主的 60 Hz 约定一致）。
pub const STEPS_PER_SECOND: u32 = 60;

/// 单帧最多补的步数：卡顿后不追赶过度（与运行时宿主同一上限口径）。
pub const MAX_CATCH_UP_STEPS: u32 = 5;

/// Play 状态机状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayState {
    /// 编辑中（未运行）。
    #[default]
    Editing,
    /// 正在创建运行时会话（初始化尚未成功前不算 Playing）。
    Starting,
    /// 运行中。
    Playing,
    /// 已暂停：逻辑时间冻结，可单帧步进。
    Paused,
    /// 正在释放运行时会话。
    Stopping,
    /// 启动失败：保持编辑态可用，错误不拖垮编辑器。
    StartFailed,
}

impl PlayState {
    /// 是否有运行时会话（Playing/Paused/Stopping 期间都持有会话）。
    #[must_use]
    pub fn has_session(self) -> bool {
        matches!(self, Self::Playing | Self::Paused | Self::Stopping)
    }

    /// 逻辑时间是否在推进。
    #[must_use]
    pub fn is_running(self) -> bool {
        self == Self::Playing
    }

    /// 编辑器是否处于"Play 模式"（含启动中/停止中的过渡态）。
    ///
    /// 与 [`Self::has_session`] 的区别：`Starting` 还没拿到运行时会话，但编辑器已经
    /// **离开了编辑态**（快照已隔离、工具栏必须显示"停止"而不是"开始"），
    /// 所以 shell 侧的 `is_playing()` 用这个口径。
    #[must_use]
    pub fn is_in_play_mode(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Playing | Self::Paused | Self::Stopping
        )
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Editing => "编辑中",
            Self::Starting => "启动中",
            Self::Playing => "运行中",
            Self::Paused => "已暂停",
            Self::Stopping => "停止中",
            Self::StartFailed => "启动失败",
        }
    }
}

/// 非法状态迁移。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlayControlError {
    /// 该操作在当前状态下无意义（例如编辑态调用 pause）。
    #[error("{action} 在 {} 状态下无效", state.label())]
    InvalidTransition {
        action: &'static str,
        state: PlayState,
    },
    /// 已经在运行/已经暂停等重复请求。
    #[error("{action} 已在 {} 状态", state.label())]
    AlreadyInState {
        action: &'static str,
        state: PlayState,
    },
    /// 步进次数必须 ≥ 1。
    #[error("步进帧数必须大于 0")]
    EmptyStep,
}

/// 本帧要跑多少个逻辑步、每步多长。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramePlan {
    pub steps: u32,
    pub delta_seconds: f32,
    /// 本帧结束时是否处于暂停（调用方据此决定是否消费一次性输入边沿）。
    pub paused: bool,
}

impl FramePlan {
    #[must_use]
    pub fn idle(delta_seconds: f32) -> Self {
        Self {
            steps: 0,
            delta_seconds,
            paused: false,
        }
    }

    /// 是否有逻辑步要跑。
    #[must_use]
    pub fn runs_logic(&self) -> bool {
        self.steps > 0
    }
}

/// Play 模式控制器：状态 + 暂停 + 单帧步进 + 固定步累加。
#[derive(Clone, Debug)]
pub struct PlayController {
    state: PlayState,
    fixed_delta: f32,
    accumulator: f32,
    /// 待执行的单帧步进次数（暂停态下由 Step 请求置位）。
    pending_steps: u32,
    /// 进入暂停时是否已经完成一次"冻结渲染"（避免重复处理）。
    starts: u64,
    stops: u64,
    steps_run: u64,
    failures: u64,
    last_failure: Option<String>,
}

impl Default for PlayController {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayController {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PlayState::Editing,
            fixed_delta: 1.0 / STEPS_PER_SECOND as f32,
            accumulator: 0.0,
            pending_steps: 0,
            starts: 0,
            stops: 0,
            steps_run: 0,
            failures: 0,
            last_failure: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> PlayState {
        self.state
    }

    /// 是否真的持有运行时会话（`Starting` 尚未持有，`Stopping` 仍在释放）。
    ///
    /// 注意与 [`PlayState::is_in_play_mode`] 的区别：这里问的是"会话在不在"，
    /// 不是"编辑器离没离开编辑态"。
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.state.has_session()
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.state == PlayState::Paused
    }

    #[must_use]
    pub fn fixed_delta(&self) -> f32 {
        self.fixed_delta
    }

    /// 待执行的单帧步进次数。
    #[must_use]
    pub fn pending_steps(&self) -> u32 {
        self.pending_steps
    }

    #[must_use]
    pub fn starts(&self) -> u64 {
        self.starts
    }

    #[must_use]
    pub fn stops(&self) -> u64 {
        self.stops
    }

    #[must_use]
    pub fn steps_run(&self) -> u64 {
        self.steps_run
    }

    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failures
    }

    #[must_use]
    pub fn last_failure(&self) -> Option<&str> {
        self.last_failure.as_deref()
    }

    /// 开始启动（`Editing` → `Starting`）。
    pub fn begin_start(&mut self) -> Result<(), PlayControlError> {
        match self.state {
            PlayState::Editing | PlayState::StartFailed => {
                self.state = PlayState::Starting;
                self.accumulator = 0.0;
                self.pending_steps = 0;
                self.last_failure = None;
                Ok(())
            }
            other => Err(PlayControlError::InvalidTransition {
                action: "开始运行",
                state: other,
            }),
        }
    }

    /// 启动成功（`Starting` → `Playing`）。
    pub fn mark_running(&mut self) -> Result<(), PlayControlError> {
        if self.state != PlayState::Starting {
            return Err(PlayControlError::InvalidTransition {
                action: "启动成功",
                state: self.state,
            });
        }
        self.state = PlayState::Playing;
        self.starts += 1;
        Ok(())
    }

    /// 启动失败（`Starting` → `StartFailed`）：错误留在控制器里，编辑器继续可用。
    pub fn fail_start(&mut self, reason: impl Into<String>) {
        self.state = PlayState::StartFailed;
        self.failures += 1;
        self.last_failure = Some(reason.into());
        self.accumulator = 0.0;
        self.pending_steps = 0;
    }

    /// 运行中记录一次致命错误（保持会话但冻结逻辑，等待用户停止/重启）。
    pub fn fail_running(&mut self, reason: impl Into<String>) {
        if self.state.has_session() {
            self.state = PlayState::Paused;
        }
        self.failures += 1;
        self.last_failure = Some(reason.into());
        self.accumulator = 0.0;
    }

    /// 暂停（`Playing` → `Paused`）。
    pub fn pause(&mut self) -> Result<(), PlayControlError> {
        match self.state {
            PlayState::Playing => {
                self.state = PlayState::Paused;
                self.accumulator = 0.0;
                Ok(())
            }
            PlayState::Paused => Err(PlayControlError::AlreadyInState {
                action: "暂停",
                state: PlayState::Paused,
            }),
            other => Err(PlayControlError::InvalidTransition {
                action: "暂停",
                state: other,
            }),
        }
    }

    /// 恢复（`Paused` → `Playing`）。
    pub fn resume(&mut self) -> Result<(), PlayControlError> {
        match self.state {
            PlayState::Paused => {
                self.state = PlayState::Playing;
                self.accumulator = 0.0;
                self.pending_steps = 0;
                Ok(())
            }
            PlayState::Playing => Err(PlayControlError::AlreadyInState {
                action: "恢复运行",
                state: PlayState::Playing,
            }),
            other => Err(PlayControlError::InvalidTransition {
                action: "恢复运行",
                state: other,
            }),
        }
    }

    /// 暂停/运行之间切换（工具栏单按钮用）。
    pub fn toggle_pause(&mut self) -> Result<PlayState, PlayControlError> {
        match self.state {
            PlayState::Playing => {
                self.pause()?;
                Ok(self.state)
            }
            PlayState::Paused => {
                self.resume()?;
                Ok(self.state)
            }
            other => Err(PlayControlError::InvalidTransition {
                action: "暂停切换",
                state: other,
            }),
        }
    }

    /// 请求在暂停态下步进 `frames` 个固定帧（运行中调用会先暂停，符合 Unity 习惯）。
    pub fn request_step(&mut self, frames: u32) -> Result<(), PlayControlError> {
        if frames == 0 {
            return Err(PlayControlError::EmptyStep);
        }
        match self.state {
            PlayState::Playing => {
                self.state = PlayState::Paused;
            }
            PlayState::Paused => {}
            other => {
                return Err(PlayControlError::InvalidTransition {
                    action: "单帧步进",
                    state: other,
                });
            }
        }
        self.accumulator = 0.0;
        self.pending_steps = self.pending_steps.saturating_add(frames);
        Ok(())
    }

    /// 开始停止（`Playing`/`Paused` → `Stopping`）。
    pub fn begin_stop(&mut self) -> Result<(), PlayControlError> {
        match self.state {
            PlayState::Playing | PlayState::Paused => {
                self.state = PlayState::Stopping;
                self.pending_steps = 0;
                self.accumulator = 0.0;
                Ok(())
            }
            other => Err(PlayControlError::InvalidTransition {
                action: "停止运行",
                state: other,
            }),
        }
    }

    /// 停止完成（`Stopping` → `Editing`）。
    pub fn finish_stop(&mut self) -> Result<(), PlayControlError> {
        if self.state != PlayState::Stopping {
            return Err(PlayControlError::InvalidTransition {
                action: "停止完成",
                state: self.state,
            });
        }
        self.state = PlayState::Editing;
        self.stops += 1;
        self.pending_steps = 0;
        self.accumulator = 0.0;
        Ok(())
    }

    /// 重启请求：`Playing`/`Paused` 下先把状态交给调用方释放再重建会话。
    ///
    /// 返回 `true` 表示调用方应执行「停止后重新开始」；编辑态下直接开始。
    pub fn request_restart(&mut self) -> Result<bool, PlayControlError> {
        match self.state {
            PlayState::Editing | PlayState::StartFailed => {
                self.begin_start()?;
                Ok(false)
            }
            PlayState::Playing | PlayState::Paused => {
                // 不在这里改状态：调用方会先 stop()（内部走 begin_stop → Stopping → Editing）
                // 再 play()。若提前改成 Stopping，stop() 里的 begin_stop() 会因状态不符而 Err，
                // 导致状态卡在 Stopping、后续 Stop/暂停按钮全部 NoOp。
                Ok(true)
            }
            other => Err(PlayControlError::InvalidTransition {
                action: "重启运行",
                state: other,
            }),
        }
    }

    /// 测试辅助：直接把状态机置到某个状态（用于验证既有面板/命令在特定状态下的行为）。
    ///
    /// 不能标 `#[cfg(test)]`：`editor` 包的 bin 目标在测试时把 lib 当外部依赖编译，
    /// 那时 lib 并不带 `cfg(test)`，bin 侧测试（命令路由/工具栏/Game 视图）就拿不到
    /// 这个辅助函数。它只改状态字段，不参与任何生产路径。
    #[doc(hidden)]
    pub fn set_state_for_test(&mut self, state: PlayState) {
        self.state = state;
        self.accumulator = 0.0;
        self.pending_steps = 0;
    }

    /// 计算本帧的固定步计划。
    ///
    /// - 运行中：按 `elapsed_seconds` 累加，补步上限 [`MAX_CATCH_UP_STEPS`]（超额丢弃）；
    /// - 暂停中且有待步进：执行剩余步数，每步一个固定 delta；
    /// - 暂停中且无待步进：0 步（逻辑时间冻结）。
    pub fn plan_frame(&mut self, elapsed_seconds: f32) -> FramePlan {
        let delta = self.fixed_delta;
        match self.state {
            PlayState::Playing => {
                let elapsed = if elapsed_seconds.is_finite() && elapsed_seconds > 0.0 {
                    elapsed_seconds
                } else {
                    delta
                };
                self.accumulator += elapsed;
                let mut steps = 0_u32;
                while self.accumulator >= delta && steps < MAX_CATCH_UP_STEPS {
                    self.accumulator -= delta;
                    steps += 1;
                }
                if self.accumulator >= delta {
                    // 追上不动的部分直接丢弃，避免死亡螺旋。
                    self.accumulator %= delta;
                }
                self.steps_run += u64::from(steps);
                FramePlan {
                    steps,
                    delta_seconds: delta,
                    paused: false,
                }
            }
            PlayState::Paused => {
                let steps = self.pending_steps;
                self.pending_steps = 0;
                self.steps_run += u64::from(steps);
                FramePlan {
                    steps,
                    delta_seconds: delta,
                    paused: true,
                }
            }
            _ => FramePlan {
                steps: 0,
                delta_seconds: delta,
                paused: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> PlayController {
        let mut controller = PlayController::new();
        controller.begin_start().expect("start");
        controller.mark_running().expect("running");
        controller
    }

    #[test]
    fn start_stop_cycle_tracks_counters_and_state() {
        let mut controller = PlayController::new();
        assert_eq!(controller.state(), PlayState::Editing);
        assert!(!controller.is_playing());
        controller.begin_start().expect("begin");
        assert_eq!(controller.state(), PlayState::Starting);
        assert!(!controller.is_playing(), "Starting 尚未持有会话");
        controller.mark_running().expect("running");
        assert_eq!(controller.state(), PlayState::Playing);
        assert!(controller.is_playing());
        controller.begin_stop().expect("stop");
        assert_eq!(controller.state(), PlayState::Stopping);
        assert!(controller.is_playing(), "Stopping 仍在释放会话");
        controller.finish_stop().expect("finish");
        assert_eq!(controller.state(), PlayState::Editing);
        assert_eq!(controller.starts(), 1);
        assert_eq!(controller.stops(), 1);
    }

    #[test]
    fn illegal_transitions_are_reported_not_ignored() {
        let mut controller = PlayController::new();
        assert!(matches!(
            controller.pause(),
            Err(PlayControlError::InvalidTransition { .. })
        ));
        assert!(matches!(
            controller.resume(),
            Err(PlayControlError::InvalidTransition { .. })
        ));
        assert!(matches!(
            controller.request_step(1),
            Err(PlayControlError::InvalidTransition { .. })
        ));
        assert!(matches!(
            controller.begin_stop(),
            Err(PlayControlError::InvalidTransition { .. })
        ));

        let mut running = playing();
        running.pause().expect("pause");
        assert!(matches!(
            running.pause(),
            Err(PlayControlError::AlreadyInState { .. })
        ));
        running.resume().expect("resume");
        assert!(matches!(
            running.resume(),
            Err(PlayControlError::AlreadyInState { .. })
        ));
        assert!(matches!(
            running.request_step(0),
            Err(PlayControlError::EmptyStep)
        ));
    }

    #[test]
    fn fixed_step_accumulates_and_caps_catch_up() {
        let mut controller = playing();
        // 恰好一帧：1 步。
        let plan = controller.plan_frame(1.0 / 60.0);
        assert_eq!(plan.steps, 1);
        assert!(!plan.paused);
        // 不足一帧：0 步（累加器留存）。
        let plan = controller.plan_frame(1.0 / 120.0);
        assert_eq!(plan.steps, 0);
        let plan = controller.plan_frame(1.0 / 120.0);
        assert_eq!(plan.steps, 1, "两个半帧应凑成一步");
        // 长时间卡顿：补步有上限，不会一次跑几十步。
        let plan = controller.plan_frame(1.0);
        assert_eq!(plan.steps, MAX_CATCH_UP_STEPS);
        // 非法 elapsed（NaN/负数）按一个固定步处理，不产生 0/无穷步。
        let plan = controller.plan_frame(f32::NAN);
        assert_eq!(plan.steps, 1);
        let plan = controller.plan_frame(-5.0);
        assert_eq!(plan.steps, 1);
        assert_eq!(
            controller.steps_run(),
            // 1（整帧）+ 1（两个半帧凑一）+ 5（补步上限）+ 1（NaN）+ 1（负数）
            1 + 1 + u64::from(MAX_CATCH_UP_STEPS) + 1 + 1
        );
    }

    #[test]
    fn pause_freezes_logical_time() {
        let mut controller = playing();
        controller.pause().expect("pause");
        for _ in 0..10 {
            let plan = controller.plan_frame(1.0 / 60.0);
            assert_eq!(plan.steps, 0, "暂停期间不得推进逻辑");
            assert!(plan.paused);
            assert!(!plan.runs_logic());
        }
        assert_eq!(controller.steps_run(), 0, "暂停期间累计逻辑步必须为 0");
        controller.resume().expect("resume");
        assert_eq!(controller.plan_frame(1.0 / 60.0).steps, 1);
    }

    #[test]
    fn step_runs_exactly_the_requested_frames_then_freezes() {
        let mut controller = playing();
        controller.request_step(3).expect("step");
        assert_eq!(controller.pending_steps(), 3);

        let first = controller.plan_frame(1.0 / 60.0);
        assert_eq!(first.steps, 3, "一次请求的 3 帧在下一帧全部执行");
        assert!(first.paused, "步进后仍处于暂停");
        assert_eq!(controller.pending_steps(), 0);

        let second = controller.plan_frame(1.0 / 60.0);
        assert_eq!(second.steps, 0, "步进用完即冻结");
        assert!(controller.is_paused());
        assert_eq!(controller.steps_run(), 3);
    }

    #[test]
    fn step_from_running_pauses_first() {
        let mut controller = playing();
        controller.request_step(1).expect("step");
        assert_eq!(controller.state(), PlayState::Paused);
        assert_eq!(controller.plan_frame(0.0).steps, 1);
    }

    #[test]
    fn failure_is_isolated_and_recorded() {
        let mut controller = PlayController::new();
        controller.begin_start().expect("begin");
        controller.fail_start("GPU 初始化失败");
        assert_eq!(controller.state(), PlayState::StartFailed);
        assert_eq!(controller.failures(), 1);
        assert_eq!(controller.last_failure(), Some("GPU 初始化失败"));
        assert_eq!(controller.starts(), 0, "启动失败不计入成功次数");
        // 失败后仍可重新开始。
        controller.begin_start().expect("retry");
        controller.mark_running().expect("running");
        assert_eq!(controller.starts(), 1);

        // 运行中致命错误：冻结逻辑但保留会话（可停止/重启）。
        controller.fail_running("脚本 panic");
        assert_eq!(controller.state(), PlayState::Paused);
        assert!(controller.is_playing());
        assert_eq!(
            controller.plan_frame(1.0).steps,
            0,
            "故障后不得继续推进逻辑"
        );
        assert_eq!(controller.failures(), 2);
    }

    /// 重启请求的契约：**由调用方**先释放会话再重新开始。
    ///
    /// `request_restart` 在运行/暂停态**故意不改状态**（保持 `Playing`/`Paused`），
    /// 否则调用方接着走的 `stop()` → `begin_stop()` 会因状态已不是运行态而 `Err`，
    /// 状态卡在 `Stopping`、后续 Stop/暂停全变 NoOp（见方法上的注释与
    /// `panels::game_view::restart_play` 的 `Ok(true) => { stop(); play(); }`）。
    /// 因此这里模拟调用方的真实两步，而不是断言 `request_restart` 自己变成 `Stopping`。
    #[test]
    fn restart_requests_stop_then_start() {
        let mut controller = playing();
        assert!(controller.request_restart().expect("restart"));
        assert_eq!(
            controller.state(),
            PlayState::Playing,
            "运行态不得被 request_restart 提前改成 Stopping：调用方还要走 stop()"
        );
        // 调用方的第一步：stop() 内部就是 begin_stop → 释放 → finish_stop。
        controller.begin_stop().expect("begin stop");
        assert_eq!(controller.state(), PlayState::Stopping);
        controller.finish_stop().expect("finish stop");
        assert_eq!(controller.state(), PlayState::Editing);
        assert_eq!(controller.stops(), 1);
        // 第二步：play()。
        controller.begin_start().expect("begin");
        controller.mark_running().expect("running");
        assert_eq!(controller.starts(), 2, "重启计为第二次启动");

        // 编辑态重启：直接开始。
        let mut idle = PlayController::new();
        assert!(!idle.request_restart().expect("restart from editing"));
        assert_eq!(idle.state(), PlayState::Starting);
    }

    #[test]
    fn toggle_pause_switches_both_ways() {
        let mut controller = playing();
        assert_eq!(controller.toggle_pause().expect("pause"), PlayState::Paused);
        assert_eq!(
            controller.toggle_pause().expect("resume"),
            PlayState::Playing
        );
        let mut idle = PlayController::new();
        assert!(controller.toggle_pause().is_ok());
        assert!(idle.toggle_pause().is_err(), "编辑态没有可切换的暂停");
    }
}
