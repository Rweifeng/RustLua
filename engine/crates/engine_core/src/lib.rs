//! Core types for the Rust + Lua game engine.

/// Logging and context-rich engine errors.
#[doc = ""]
pub mod logging;
/// Lua VM and host API bridge.
pub mod script;
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
