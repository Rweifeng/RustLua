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
            accumulator_seconds: 0.0,
            total_dropped_steps: 0,
            total_steps: 0,
        })
    }

    /// Advances the scheduler by wall-clock time and reports fixed updates to run.
    pub fn advance(&mut self, elapsed_seconds: f64) -> StepReport {
        self.accumulator_seconds += elapsed_seconds.max(0.0);
        let mut steps = 0;
        while self.accumulator_seconds >= self.fixed_delta_seconds
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
}
