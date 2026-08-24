//! Window-independent lifecycle state used by the application runner.

/// High-level state of the application lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppLifecycle {
    Initializing,
    Running,
    Paused,
    Exiting,
    Exited,
}

impl AppLifecycle {
    /// Returns whether the application should advance its simulation.
    #[must_use]
    pub const fn is_simulation_active(self) -> bool {
        matches!(self, Self::Running)
    }

    /// Transitions to a new state while enforcing the M1 lifecycle contract.
    pub fn transition(&mut self, next: Self) -> Result<(), LifecycleError> {
        let valid = matches!(
            (*self, next),
            (Self::Initializing, Self::Running)
                | (Self::Initializing, Self::Exiting)
                | (Self::Running, Self::Paused)
                | (Self::Running, Self::Exiting)
                | (Self::Paused, Self::Running)
                | (Self::Paused, Self::Exiting)
                | (Self::Exiting, Self::Exited)
        );
        if valid {
            *self = next;
            Ok(())
        } else {
            Err(LifecycleError {
                from: *self,
                to: next,
            })
        }
    }
}

/// Invalid lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleError {
    pub from: AppLifecycle,
    pub to: AppLifecycle,
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid lifecycle transition: {:?} -> {:?}",
            self.from, self.to
        )
    }
}

impl std::error::Error for LifecycleError {}

/// Returns whether a window size can be rendered.
#[must_use]
pub const fn has_renderable_size(width: u32, height: u32) -> bool {
    width > 0 && height > 0
}

#[cfg(test)]
mod tests {
    use super::{AppLifecycle, LifecycleError, has_renderable_size};

    #[test]
    fn follows_initialize_run_pause_resume_exit_sequence() {
        let mut lifecycle = AppLifecycle::Initializing;
        for state in [
            AppLifecycle::Running,
            AppLifecycle::Paused,
            AppLifecycle::Running,
            AppLifecycle::Exiting,
            AppLifecycle::Exited,
        ] {
            lifecycle
                .transition(state)
                .expect("valid lifecycle transition");
        }
        assert_eq!(lifecycle, AppLifecycle::Exited);
        assert!(!lifecycle.is_simulation_active());
    }

    #[test]
    fn rejects_skipping_initialization() {
        let mut lifecycle = AppLifecycle::Initializing;
        assert_eq!(
            lifecycle.transition(AppLifecycle::Paused),
            Err(LifecycleError {
                from: AppLifecycle::Initializing,
                to: AppLifecycle::Paused,
            })
        );
    }

    #[test]
    fn zero_sized_windows_are_not_renderable() {
        assert!(!has_renderable_size(0, 720));
        assert!(!has_renderable_size(1280, 0));
        assert!(has_renderable_size(1280, 720));
    }
}
