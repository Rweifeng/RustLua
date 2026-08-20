//! Core types for the Rust + Lua game engine.
//!
//! This crate intentionally starts with a dependency-free API surface. Runtime
//! systems will be added in later milestones behind explicit, tested modules.

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
