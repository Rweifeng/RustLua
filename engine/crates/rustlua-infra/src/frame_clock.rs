//! Host-side frame clock: turns wall-clock frame intervals into fixed simulation steps.
//!
//! `time_step::FixedTimeStep` owns the arithmetic; this module owns the **policy** a host driver
//! must apply to raw frame intervals. Both hosts use it, and that is the point: the browser host
//! (`game_web::gpu_host`, driven by `requestAnimationFrame`) and the WeChat engine host
//! (`game_web::host_runtime`, driven by `wx.requestAnimationFrame`) must not drift apart in how
//! they treat a frame interval. Three rules are shared, and each one is a bug we can name:
//!
//! * **Clamp absurd intervals.** A tab switch, a debugger pause or a long GC hands the next frame
//!   an interval of seconds. Feeding that to the fixed-step clock burns the whole catch-up budget
//!   on a single frame (a visible jump) — so the interval is capped first and the event is counted.
//! * **Tolerate sub-ulp "exactly one frame".** `performance.now()` differences regularly land a few
//!   ulps below `1/60`; without tolerance the step count jitters between 0 and 1 (see
//!   `FixedTimeStep::with_step_tolerance`).
//! * **Count what was dropped.** The spiral-of-death guard silently discards time; a host that
//!   cannot report the tally cannot tell "the device is slow" from "our clock is wrong".

use crate::time_step::{FixedTimeStep, TimeStepError};

/// What one wall-clock frame interval should do to the simulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePlan {
    /// Fixed simulation updates to run for this interval.
    pub steps: u32,
    /// Duration handed to every fixed update.
    pub delta_seconds: f32,
    /// Wall time actually fed to the fixed-step clock (already clamped).
    pub elapsed_seconds: f64,
    /// `true` when the raw interval exceeded the cap and was clamped to it.
    pub clamped: bool,
    /// Wall time still waiting for the next fixed update.
    pub accumulator_seconds: f64,
    /// Fixed updates discarded in this frame by the catch-up bound.
    pub dropped_steps: u64,
    /// 1-based index of this frame (the first planned frame is 1).
    pub frame_index: u64,
}

/// Frame clock with a bounded catch-up window and a clamped input interval.
#[derive(Debug, Clone, Copy)]
pub struct FrameClock {
    steps: FixedTimeStep,
    max_frame_seconds: f64,
    frame_index: u64,
    clamped_frames: u64,
}

impl FrameClock {
    /// Creates a clock at `updates_per_second`, bounded by `max_catch_up_steps` per frame and by
    /// `max_frame_seconds` of wall time accepted from a single interval.
    ///
    /// A typical host passes `(60, 8, 0.25)`.
    pub fn new(
        updates_per_second: u32,
        max_catch_up_steps: u32,
        max_frame_seconds: f64,
    ) -> Result<Self, FrameClockError> {
        if !(max_frame_seconds.is_finite() && max_frame_seconds > 0.0) {
            return Err(FrameClockError::InvalidFrameCap);
        }
        Ok(Self {
            steps: FixedTimeStep::new(updates_per_second, max_catch_up_steps)?
                .with_step_tolerance(FRAME_INTERVAL_TOLERANCE_SECONDS),
            max_frame_seconds,
            frame_index: 0,
            clamped_frames: 0,
        })
    }

    /// Overrides the "has the accumulator reached one fixed step" tolerance.
    ///
    /// Defaults to [`FRAME_INTERVAL_TOLERANCE_SECONDS`]. Callers that keep their own documented copy
    /// of that constant (the browser bridge does, because it mirrors the old JS values) should pass
    /// it here so the two cannot drift apart silently.
    #[must_use]
    pub fn with_tolerance(mut self, tolerance_seconds: f64) -> Self {
        self.steps = self.steps.with_step_tolerance(tolerance_seconds);
        self
    }

    /// Plans the fixed updates for one frame whose raw wall-clock interval was `raw_seconds`.
    ///
    /// Negative or non-finite intervals are treated as zero (a host must never be able to rewind
    /// or poison the clock with `NaN`; `NaN` comparisons are all false, which would otherwise make
    /// the step loop silently never run and then never drain).
    pub fn plan(&mut self, raw_seconds: f64) -> FramePlan {
        let raw = if raw_seconds.is_finite() && raw_seconds > 0.0 {
            raw_seconds
        } else {
            0.0
        };
        let elapsed = raw.min(self.max_frame_seconds);
        let clamped = raw > elapsed;
        if clamped {
            self.clamped_frames += 1;
        }
        let report = self.steps.advance(elapsed);
        self.frame_index += 1;
        FramePlan {
            steps: report.steps,
            delta_seconds: report.delta_seconds,
            elapsed_seconds: elapsed,
            clamped,
            accumulator_seconds: report.accumulator_seconds,
            dropped_steps: report.dropped_steps,
            frame_index: self.frame_index,
        }
    }

    /// Discards partial wall-clock time (called when simulation resumes after a pause).
    pub fn reset_accumulator(&mut self) {
        self.steps.reset_accumulator();
    }

    /// Frames planned so far.
    #[must_use]
    pub const fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Frames whose raw interval had to be clamped to the cap.
    #[must_use]
    pub const fn clamped_frames(&self) -> u64 {
        self.clamped_frames
    }

    /// The interval cap in seconds.
    #[must_use]
    pub const fn max_frame_seconds(&self) -> f64 {
        self.max_frame_seconds
    }

    /// Total fixed updates emitted since creation.
    #[must_use]
    pub const fn total_steps(&self) -> u64 {
        self.steps.total_steps()
    }

    /// Total fixed updates discarded since creation.
    #[must_use]
    pub const fn total_dropped_steps(&self) -> u64 {
        self.steps.total_dropped_steps()
    }

    /// Wall-clock time still waiting for the next fixed update.
    #[must_use]
    pub const fn accumulator_seconds(&self) -> f64 {
        self.steps.accumulator_seconds()
    }

    /// The fixed update duration.
    #[must_use]
    pub const fn fixed_delta_seconds(&self) -> f32 {
        self.steps.fixed_delta_seconds()
    }
}

/// Slack applied to the "has the accumulator reached one fixed step" test, in seconds.
///
/// Same value the browser path uses (`game_web::lua_bridge`): far below any real frame interval,
/// far above the ulps that `performance.now()` differences produce.
pub const FRAME_INTERVAL_TOLERANCE_SECONDS: f64 = 1e-9;

/// Invalid frame clock configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameClockError {
    /// The fixed-step configuration was rejected.
    #[error(transparent)]
    TimeStep(#[from] TimeStepError),
    /// The per-frame interval cap must be a positive, finite number of seconds.
    #[error("max frame seconds must be positive and finite")]
    InvalidFrameCap,
}

#[cfg(test)]
mod tests {
    use super::{FrameClock, FrameClockError};

    const FRAME_60: f64 = 1.0 / 60.0;

    #[test]
    fn sixty_hz_frames_each_run_exactly_one_step() {
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        for expected in 1..=5 {
            let plan = clock.plan(FRAME_60);
            assert_eq!(plan.steps, 1, "frame {expected}");
            assert_eq!(plan.frame_index, expected);
            assert!(!plan.clamped);
            assert_eq!(plan.dropped_steps, 0);
        }
        assert_eq!(clock.total_steps(), 5);
    }

    #[test]
    fn sub_ulp_one_frame_still_plans_one_step() {
        // performance.now() 两次读数之差落在 1/60 之下几个 ulp 是常态：不允许抖动成 0 步。
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        assert_eq!(clock.plan(FRAME_60 - 1e-12).steps, 1);
        // 但"真的没到一帧"必须仍然是 0 步：容差不能把半帧当成一帧。
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        assert_eq!(clock.plan(FRAME_60 - 1e-3).steps, 0);
    }

    #[test]
    fn thirty_hz_frames_run_two_steps() {
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        let plan = clock.plan(1.0 / 30.0);
        assert_eq!(plan.steps, 2);
        assert!((plan.delta_seconds - (1.0 / 60.0) as f32).abs() < f32::EPSILON);
    }

    #[test]
    fn long_stall_is_clamped_instead_of_burning_the_catch_up_budget() {
        // 切标签页/断点/长 GC 会送来一个"秒级"间隔。不夹住就会把整个追帧预算烧在一帧上。
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        let plan = clock.plan(3.0);
        assert!(plan.clamped);
        assert_eq!(plan.elapsed_seconds, 0.25);
        assert_eq!(plan.steps, 8, "夹到 0.25 s ⇒ 15 步被上限截到 8");
        assert_eq!(plan.dropped_steps, 7);
        assert_eq!(clock.clamped_frames(), 1);
        assert!(plan.accumulator_seconds < FRAME_60);
    }

    #[test]
    fn nonsense_intervals_are_zero_and_never_poison_the_clock() {
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let plan = clock.plan(bad);
            assert_eq!(plan.steps, 0, "interval {bad}");
            assert_eq!(plan.elapsed_seconds, 0.0);
            assert!(!plan.clamped);
        }
        // 仍能正常走：坏输入没有把累加器弄脏。
        assert_eq!(clock.plan(FRAME_60).steps, 1);
        assert_eq!(clock.clamped_frames(), 0);
        assert_eq!(clock.frame_index(), 5);
    }

    #[test]
    fn reset_drops_partial_time_after_a_pause() {
        let mut clock = FrameClock::new(60, 8, 0.25).unwrap();
        assert_eq!(clock.plan(0.01).steps, 0);
        clock.reset_accumulator();
        assert_eq!(clock.accumulator_seconds(), 0.0);
        assert_eq!(clock.plan(FRAME_60).steps, 1);
    }

    #[test]
    fn tolerance_override_is_honoured() {
        // 默认容差：比一帧少 1e-12 秒（几个 ulp）仍算一帧。
        assert_eq!(
            FrameClock::new(60, 8, 0.25).unwrap().plan(FRAME_60 - 1e-12).steps,
            1
        );
        // 收紧到 0：同样的间隔不再算一帧（证明这个开关真的在起作用，不是摆设）。
        let mut strict = FrameClock::new(60, 8, 0.25).unwrap().with_tolerance(0.0);
        assert_eq!(strict.plan(FRAME_60 - 1e-12).steps, 0);
        // 放松则相反：差 1e-4 秒也判一帧。
        let mut loose = FrameClock::new(60, 8, 0.25).unwrap().with_tolerance(1e-3);
        assert_eq!(loose.plan(FRAME_60 - 1e-4).steps, 1);
    }

    #[test]
    fn rejects_a_useless_cap() {
        assert!(matches!(
            FrameClock::new(60, 8, 0.0),
            Err(FrameClockError::InvalidFrameCap)
        ));
        assert!(matches!(
            FrameClock::new(60, 8, f64::NAN),
            Err(FrameClockError::InvalidFrameCap)
        ));
        // 固定步长自身的不合法配置照样透传。
        assert!(matches!(
            FrameClock::new(0, 8, 0.25),
            Err(FrameClockError::TimeStep(_))
        ));
    }
}
