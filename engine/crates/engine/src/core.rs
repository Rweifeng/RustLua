//! Compatibility re-export layer.
//!
//! The original `core` module was split into independent crates.
//! This module re-exports every sub-module under its original name so that
//! existing `crate::core::module::Item` paths (used by `host`, `game`,
//! `editor`, etc.) continue to work unchanged.

// ── rustlua-math ──
pub use rustlua_math::{geom3d, math2d, math3d};

// ── rustlua-ecs ──
pub use rustlua_ecs::{
    commands, component_schema, data_schema, dense_storage, entity, entity_script, events,
    hierarchy, particle, property_path, scheduler, spatial_transform, tilemap, transform, world,
};

// ── rustlua-scene ──
pub use rustlua_scene::{
    asset_import, char_collect, prefab, scene, scene_extract, scene_migration, scene_space,
    typed_scene, world_text,
};

// ── rustlua-camera ──
pub use rustlua_camera::camera;

// ── rustlua-animation ──
pub use rustlua_animation::{animation, animation_track, animation3d};

// ── rustlua-render ──
pub use rustlua_render::{
    async_loader, atlas, font, font_subset, layer, light, lod, mesh3d, picking, post_process, quad,
    render_mode, render_plan, renderable, spatial_index, sprite, sprite_batch, text, texture,
    visibility,
};

// ── rustlua-ui ──
pub use rustlua_ui::{
    ui, ui_animation, ui_binding, ui_events, ui_layout, ui_layout_cache, ui_navigation,
    ui_scene_layout, ui_states,
};

// ── rustlua-physics ──
pub use rustlua_physics::{physics, physics3d};

// ── rustlua-assets ──
pub use rustlua_assets::{
    asset_handle, atlas_format, atlas_pack, dependency, gltf_asset, hash, import_cache, importer,
    pack, resource_db, resource_kind,
};

// ── rustlua-audio ──
pub use rustlua_audio::{audio, sink as audio_sink};

// ── rustlua-input ──
pub use rustlua_input::input;

// ── rustlua-gameplay ──
pub use rustlua_gameplay::{gameplay, grid, save, settings};

// ── rustlua-scripting ──
pub use rustlua_scripting::script;

// ── rustlua-infra ──
pub use rustlua_infra::{
    editor_plugins, logging, par, paths, performance, preview_diagnostics, project_config,
    save_slot, time_step, user_dir, vfs, window,
};

// ── infra convenience re-exports ──
pub use rustlua_infra::{PROJECT_WORK_DIR, fixture_games_dir, games_dir, version};
