//! Core types for the Rust + Lua game engine.

/// Named animation states driven by sprite-sheet frame clips.
pub mod animation;
/// Texture atlas regions and named Sprite sub-assets.
pub mod atlas;
/// Named sound-effect playback backed by rodio.
pub mod audio;
/// Perspective camera and view-frustum primitives for 3D rendering.
pub mod camera3d;
/// Deferred world mutations for safe ECS system iteration.
pub mod commands;
/// Generational entity handles and allocation.
pub mod entity;
/// Typed, frame-bounded event queues.
pub mod events;
pub mod font;
/// CPU-side glTF/GLB mesh and scene asset importer.
pub mod gltf_asset;
/// Incremental and atomic asset import cache.
pub mod import_cache;
/// Platform-independent keyboard, mouse, and cursor snapshots.
pub mod input;
/// Directional light component for 3D scenes.
pub mod light;
/// Logging and context-rich engine errors.
pub mod logging;
/// Coordinate, camera, and color-space conventions for 2D rendering.
pub mod math2d;
/// Coordinate, quaternion, and matrix primitives for 3D rendering.
pub mod math3d;
/// GPU-friendly CPU mesh and material data for 3D rendering.
pub mod mesh3d;
/// 发布资源编解码（压缩 + 加密）。
pub mod pack;
/// Pooled particle bursts with spawn/update/recycle.
pub mod particle;
/// Runtime frame and update performance sampling.
pub mod performance;
/// Rapier-backed rigid bodies, colliders, sensors, and collision events.
pub mod physics;
/// Prefab extraction, instantiation, overrides, and rebuild policy.
pub mod prefab;
/// Persistent project asset identities, import states, dependencies, and cache metadata.
pub mod resource_db;
/// Versioned save-data serialization with corrupt/version rejection.
pub mod save;
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
/// Grid tile maps with a visual layer and a separate collision layer.
pub mod tilemap;
pub mod time_step;
/// 2D transform and velocity components with a minimal integration system.
pub mod transform;
/// 3D transform component and model-matrix construction.
pub mod transform3d;
/// Window and application lifecycle primitives.
pub mod window;
/// Entity world and type-erased component storage.
pub mod world;

/// Returns the engine core package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Per-project working directory holding local runtime data: the resource
/// database, import cache, and save data. Local and gitignored.
pub const PROJECT_WORK_DIR: &str = ".cache";

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn reports_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
