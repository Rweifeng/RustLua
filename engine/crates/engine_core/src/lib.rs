//! Core types for the Rust + Lua game engine.

/// Texture atlas regions and named Sprite sub-assets.
pub mod atlas;
/// Deferred world mutations for safe ECS system iteration.
pub mod commands;
/// Generational entity handles and allocation.
pub mod entity;
/// Typed, frame-bounded event queues.
pub mod events;
/// Incremental and atomic asset import cache.
pub mod import_cache;
/// Platform-independent keyboard, mouse, and cursor snapshots.
pub mod input;
/// Logging and context-rich engine errors.
pub mod logging;
/// Coordinate, camera, and color-space conventions for 2D rendering.
pub mod math2d;
/// Runtime frame and update performance sampling.
pub mod performance;
/// Persistent project asset identities, import states, dependencies, and cache metadata.
pub mod resource_db;
/// Versioned scene serialization, validation, and migration boundary.
pub mod scene;
/// Deterministic stage-based system scheduling.
pub mod scheduler;
/// Lua VM and host API bridge.
pub mod script;
/// CPU-side 2D Sprite geometry and UV generation.
pub mod sprite;
/// CPU-side batching and sorting for 2D Sprites.
pub mod sprite_batch;
/// Fixed-rate simulation timing primitives.
pub mod text;
/// PNG decoding and CPU-side texture asset representation.
pub mod texture;
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
