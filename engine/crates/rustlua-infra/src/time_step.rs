//! Deterministic fixed-time-step scheduling for simulation updates.

/// Result of consuming one wall-clock interval with a fixed-step scheduler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepReport {
    /// Number of simulation updates to execute for this interval.
    pub steps: u32,
    /// Duration passed to every simulation update.
    pub delta_seconds: f32,
    /// Remaining wall-clock time waiting for the next fixed update.
    pub accumulator_seconds: f64,
    /// Number of simulation updates discarded to avoid a spiral of death.
    pub dropped_steps: u64,
    /// Total simulation updates discarded since scheduler creation.
    pub total_dropped_steps: u64,
    /// Total simulation updates emitted since scheduler creation.
    pub total_steps: u64,
}

/// Fixed-rate simulation clock with a bounded catch-up window.
#[derive(Debug, Clone, Copy)]
pub struct FixedTimeStep {
    fixed_delta_seconds: f64,
    max_catch_up_steps: u32,
    step_tolerance_seconds: f64,
    accumulator_seconds: f64,
    total_dropped_steps: u64,
    total_steps: u64,
}

impl FixedTimeStep {
    /// Creates a scheduler at `updates_per_second`, bounded by `max_catch_up_steps`.
    pub fn new(updates_per_second: u32, max_catch_up_steps: u32) -> Result<Self, TimeStepError> {
        if updates_per_second == 0 {
            return Err(TimeStepError::InvalidFrequency);
        }
        if max_catch_up_steps == 0 {
            return Err(TimeStepError::InvalidCatchUpLimit);
        }
        Ok(Self {
            fixed_delta_seconds: 1.0 / f64::from(updates_per_second),
            max_catch_up_steps,
            step_tolerance_seconds: 0.0,
            accumulator_seconds: 0.0,
            total_dropped_steps: 0,
            total_steps: 0,
        })
    }

    /// Opts into a tolerance for the "has the accumulator reached one fixed step" test.
    ///
    /// The default is **zero**, i.e. a bit-exact `>=`. That must stay the default: native callers
    /// construct `elapsed` themselves (`1.0 / 60.0` is bit-identical to the fixed delta), and the
    /// tests below pin that contract.
    ///
    /// Wall-clock input is different. A browser derives `elapsed` from two `performance.now()`
    /// readings, and "exactly one frame" regularly lands a few ulps **below** `1/60`. Under the
    /// strict comparison that becomes 0 steps and the next frame catches it up — the average rate
    /// stays correct and the clock never stalls, but the step count jitters between 0 and 1, which
    /// shows up as a jittery simulation rather than as a bug. A slack far below any real frame
    /// interval pins "exactly one frame" to 1 step; the worst-case error is one tolerance per
    /// frame. `game_web::lua_bridge` uses `1e-9` seconds for this.
    #[must_use]
    pub const fn with_step_tolerance(mut self, tolerance_seconds: f64) -> Self {
        self.step_tolerance_seconds = if tolerance_seconds > 0.0 {
            tolerance_seconds
        } else {
            0.0
        };
        self
    }

    /// Advances the scheduler by wall-clock time and reports fixed updates to run.
    pub fn advance(&mut self, elapsed_seconds: f64) -> StepReport {
        self.accumulator_seconds += elapsed_seconds.max(0.0);
        let mut steps = 0;
        while self.accumulator_seconds
            >= self.fixed_delta_seconds - self.step_tolerance_seconds
            && steps < self.max_catch_up_steps
        {
            self.accumulator_seconds -= self.fixed_delta_seconds;
            steps += 1;
        }

        let dropped_steps = if self.accumulator_seconds >= self.fixed_delta_seconds {
            let dropped =
                ((self.accumulator_seconds / self.fixed_delta_seconds) + 1e-9).floor() as u64;
            self.accumulator_seconds -= dropped as f64 * self.fixed_delta_seconds;
            dropped
        } else {
            0
        };
        self.total_dropped_steps += dropped_steps;
        self.total_steps += u64::from(steps);

        StepReport {
            steps,
            delta_seconds: self.fixed_delta_seconds as f32,
            accumulator_seconds: self.accumulator_seconds,
            dropped_steps,
            total_dropped_steps: self.total_dropped_steps,
            total_steps: self.total_steps,
        }
    }

    /// Discards partial wall-clock time when simulation resumes after a pause.
    pub fn reset_accumulator(&mut self) {
        self.accumulator_seconds = 0.0;
    }

    #[must_use]
    pub const fn fixed_delta_seconds(&self) -> f32 {
        self.fixed_delta_seconds as f32
    }

    /// Wall-clock time still waiting for the next fixed update.
    #[must_use]
    pub const fn accumulator_seconds(&self) -> f64 {
        self.accumulator_seconds
    }

    /// Total simulation updates discarded since creation (the "spiral of death" guard's tally).
    #[must_use]
    pub const fn total_dropped_steps(&self) -> u64 {
        self.total_dropped_steps
    }

    /// Total simulation updates emitted since creation.
    #[must_use]
    pub const fn total_steps(&self) -> u64 {
        self.total_steps
    }
}

/// Invalid fixed-step configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimeStepError {
    #[error("updates per second must be greater than zero")]
    InvalidFrequency,
    #[error("maximum catch-up steps must be greater than zero")]
    InvalidCatchUpLimit,
}

#[cfg(test)]
mod tests {
    use super::FixedTimeStep;

    #[test]
    fn emits_deterministic_sixty_hz_steps() {
        let mut clock = FixedTimeStep::new(60, 64).unwrap();
        assert_eq!(clock.advance(1.0 / 60.0).steps, 1);
        assert_eq!(clock.advance(1.0 / 60.0).steps, 1);
        assert_eq!(clock.advance(0.5).total_steps, 32);
        assert!((clock.fixed_delta_seconds() - (1.0 / 60.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn bounds_catch_up_and_drops_excess_steps() {
        let mut clock = FixedTimeStep::new(60, 4).unwrap();
        let report = clock.advance(1.0);
        assert_eq!(report.steps, 4);
        assert_eq!(report.dropped_steps, 56);
        assert!(report.accumulator_seconds < 1.0 / 60.0);
    }

    #[test]
    fn pause_resume_does_not_replay_stale_time() {
        let mut clock = FixedTimeStep::new(60, 8).unwrap();
        assert_eq!(clock.advance(0.01).steps, 0);
        clock.reset_accumulator();
        assert_eq!(clock.advance(0.01).steps, 0);
        assert_eq!(clock.advance(1.0 / 60.0).steps, 1);
    }

    #[test]
    fn default_tolerance_keeps_bit_exact_strictness() {
        // 默认容差为 0 ⇒ 与改动前逐位一致：差几个 ulp 的"正好一帧"仍判成 0 步。
        let mut clock = FixedTimeStep::new(60, 8).unwrap();
        assert_eq!(clock.advance(1.0 / 60.0 - 1e-12).steps, 0);
        let mut clock = FixedTimeStep::new(60, 8).unwrap();
        assert_eq!(clock.advance(1.0 / 60.0).steps, 1);
        assert_eq!(clock.accumulator_seconds(), 0.0);
    }

    #[test]
    fn step_tolerance_absorbs_sub_ulp_frame_intervals() {
        // 计时器读数（performance.now 两次之差）落在 1/60 之下几个 ulp 是常态，不是异常。
        let mut tolerant = FixedTimeStep::new(60, 8).unwrap().with_step_tolerance(1e-9);
        assert_eq!(tolerant.advance(1.0 / 60.0 - 1e-12).steps, 1);
        // 容差不得改变"真的没到一帧"的判定（1e-9 远小于任何真实帧间隔）。
        let mut tolerant = FixedTimeStep::new(60, 8).unwrap().with_step_tolerance(1e-9);
        assert_eq!(tolerant.advance(1.0 / 60.0 - 1e-3).steps, 0);
        // 负容差按 0 处理（不会意外放宽）。
        let mut clock = FixedTimeStep::new(60, 8).unwrap().with_step_tolerance(-1.0);
        assert_eq!(clock.advance(1.0 / 60.0 - 1e-12).steps, 0);
    }

    #[test]
    fn counters_are_readable_without_perturbing_the_clock() {
        let mut clock = FixedTimeStep::new(60, 4).unwrap().with_step_tolerance(1e-9);
        let report = clock.advance(1.0);
        assert_eq!(report.steps, 4);
        assert_eq!(clock.total_dropped_steps(), report.total_dropped_steps);
        assert_eq!(clock.total_steps(), 4);
        assert!(clock.accumulator_seconds() < 1.0 / 60.0);
    }
}
