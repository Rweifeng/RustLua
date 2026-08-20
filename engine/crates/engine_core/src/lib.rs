//! Core types for the Rust + Lua game engine.

/// Platform-independent keyboard, mouse, and cursor snapshots.
pub mod input;
/// Logging and context-rich engine errors.
pub mod logging;
/// Runtime frame and update performance sampling.
pub mod performance;
/// Lua VM and host API bridge.
pub mod script;
/// Fixed-rate simulation timing primitives.
pub mod time_step;
/// Window and application lifecycle primitives.
pub mod window;

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
