//! Core types for the Rust + Lua game engine.

/// Deferred world mutations for safe ECS system iteration.
pub mod commands;
/// Generational entity handles and allocation.
pub mod entity;
/// Typed, frame-bounded event queues.
pub mod events;
/// Platform-independent keyboard, mouse, and cursor snapshots.
pub mod input;
/// Logging and context-rich engine errors.
pub mod logging;
/// Runtime frame and update performance sampling.
pub mod performance;
/// Deterministic stage-based system scheduling.
pub mod scheduler;
/// Lua VM and host API bridge.
pub mod script;
/// Fixed-rate simulation timing primitives.
pub mod time_step;
/// 2D transform and velocity components with a minimal integration system.
pub mod transform;
/// Window and application lifecycle primitives.
pub mod window;
/// Entity world and type-erased component storage.
pub mod world;

/// Returns the engine core package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn reports_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
