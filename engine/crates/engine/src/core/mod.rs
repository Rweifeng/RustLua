//! Core types for the Rust + Lua game engine.

/// Named animation states driven by sprite-sheet frame clips.
pub mod animation;
/// 3D skeletal animation component driven by glTF skins and clips.
pub mod animation3d;
/// Texture atlas regions and named Sprite sub-assets.
pub mod atlas;
/// Named sound-effect playback backed by rodio.
pub mod audio;
/// Perspective camera and view-frustum primitives for 3D rendering.
pub mod camera3d;
/// 打包时字符收集：扫描场景 Text 与 Lua 字符串，产出去重的字符集供字形预热。
pub mod char_collect;
/// Deferred world mutations for safe ECS system iteration.
pub mod commands;
/// E21: typed schema registry over the legacy string component format.
pub mod component_schema;
/// Lightweight JSON-schema validation for `data` tables loaded via Lua.
pub mod data_schema;
/// Dependency-graph extraction, resolution, propagation, and cycle detection.
pub mod dependency;
/// E27: versioned, fault-isolated editor plugin registry.
pub mod editor_plugins;
/// Generational entity handles and allocation.
pub mod entity;
/// Typed, frame-bounded event queues.
pub mod events;
pub mod font;
/// 打包时 TrueType 字体子集化：把字体裁剪到用到的字符，缩小发布体积。
pub mod font_subset;
/// Gameplay data types for clocks, items, and inventories.
pub mod gameplay;
/// CPU-side glTF/GLB mesh and scene asset importer.
pub mod gltf_asset;
/// Grid helpers for tile-based world/cell conversion and facing.
pub mod grid;
/// Content fingerprinting shared by the import cache and resource database.
pub mod hash;
/// Incremental and atomic asset import cache.
pub mod import_cache;
/// Importer trait, builtin registry, and batch runner.
pub mod importer;
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
/// 游戏工程路径解析（已迁至 `C:\GitHub\games`）。
pub mod paths;
pub use paths::games_dir;
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
pub mod physics3d;
/// Prefab extraction, instantiation, overrides, and rebuild policy.
pub mod prefab;
/// E24: structured preview diagnostics with stable categories and deduping.
pub mod preview_diagnostics;
/// Persistent project asset identities, import states, dependencies, and cache metadata.
pub mod resource_db;
/// Unified coarse-grained resource families shared by editor, assetc, and engine.
pub mod resource_kind;
/// Versioned save-data serialization with corrupt/version rejection.
pub mod save;
/// Versioned scene serialization, validation, and migration boundary.
pub mod scene;
/// Shared extraction of serialized 3D render components.
pub mod scene_extract;
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
/// Screen-space UI scene components (Text / Image / Button).
pub mod ui;
/// E26: pure UI animation tracks and isolated preview playback.
pub mod ui_animation;
/// E23: minimal one-way ViewModel property bindings.
pub mod ui_binding;
/// E22: structured UI event bindings with legacy clickEvent conversion.
pub mod ui_events;
/// E15/E16/E17 UI layout layer: Anchor+Offset, Container auto-layout, Canvas.
pub mod ui_layout;
/// E25: resolved-layout cache with subtree-aware invalidation.
pub mod ui_layout_cache;
/// E17: parent-chain and container resolution over serialized UI nodes.
pub mod ui_scene_layout;
/// E20: reusable button visual-state configuration.
pub mod ui_states;
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
