//! WP12 阶段 0 守卫：`engine::host::` 对外符号面的存在性 pin。
//!
//! 事实源是 `scripts/verify/engine-pub-surface.mjs` 读的 rustdoc 产物（路径与项的种类），那份账抓不到**同一个名字还挂在同一个类型上**
//! 这件事在 nightly 缺席时的另一面，所以这里用手写的路径引用补一条编译期断言：下面每一行都只解析路径、不调用，
//! `RuntimeScene`/`Renderer` 的方法一旦被搬进 `pub mod` 子模块而忘记私有化，或从 `host/mod.rs` 的 `pub use` 里漏掉，
//! 这个文件就编译不过。它与 `editor`/`game`/`game_web`/`assetc` 的工作区编译是同一件事的两面：后者证明**用得到的**没坏，
//! 这份清单证明**该在的**都还在（清单由 `grep "^    pub fn " host/runtime_scene.rs host/renderer.rs` 生成）。

#[allow(dead_code, unused_variables)]
fn pin_paths() {
    // 类型与 `host/mod.rs` 的重导出。
    let _ = rustlua_gpu::Renderer::new;
    let _ = rustlua_gpu::Renderer::cached_mesh_count;
    let _ = rustlua_gpu::Renderer::clear_gpu_particles;
    let _ = rustlua_gpu::Renderer::configure_gpu_particles;
    let _ = rustlua_gpu::Renderer::copy_texture_to_cpu;
    let _ = rustlua_gpu::Renderer::frame_views;
    let _ = rustlua_gpu::Renderer::gpu_particle_status;
    let _ = rustlua_gpu::Renderer::gpu_particle_systems;
    let _ = rustlua_gpu::Renderer::gpu_timestamp_reason;
    let _ = rustlua_gpu::Renderer::gpu_timestamp_supported;
    let _ = rustlua_gpu::Renderer::last_frame_metrics;
    let _ = rustlua_gpu::Renderer::last_frame_passes;
    let _ = rustlua_gpu::Renderer::last_frame_plan;
    let _ = rustlua_gpu::Renderer::loaded_texture_count;
    let _ = rustlua_gpu::Renderer::mesh_identity_stats;
    let _ = rustlua_gpu::Renderer::new;
    let _ = rustlua_gpu::Renderer::new_offscreen;
    let _ = rustlua_gpu::Renderer::post_process;
    let _ = rustlua_gpu::Renderer::push_gpu_particle_burst;
    let _ = rustlua_gpu::Renderer::read_gpu_particles;
    let _ = rustlua_gpu::Renderer::readback_busy_count;
    let _ = rustlua_gpu::Renderer::readback_frame;
    let _ = rustlua_gpu::Renderer::readback_in_flight;
    let _ = rustlua_gpu::Renderer::readback_metrics;
    let _ = rustlua_gpu::Renderer::readback_pending;
    let _ = rustlua_gpu::Renderer::readback_slot_count;
    let _ = rustlua_gpu::Renderer::render;
    let _ = rustlua_gpu::Renderer::render_into_texture;
    let _ = rustlua_gpu::Renderer::resize;
    let _ = rustlua_gpu::Renderer::resource_stats;
    let _ = rustlua_gpu::Renderer::set_frame_views;
    let _ = rustlua_gpu::Renderer::set_post_process;
    let _ = rustlua_gpu::Renderer::set_world_sprite_camera;
    let _ = rustlua_gpu::Renderer::step_gpu_particles;
    let _ = rustlua_gpu::Renderer::texture_identity_stats;
    let _ = rustlua_gpu::Renderer::vertex_buffer_capacity;
    let _ = rustlua_gpu::Renderer::vertex_buffer_growths;
    let _ = rustlua_gpu::Renderer::vertex_bytes_peak;
    let _ = rustlua_gpu::Renderer::wait_for_gpu_timestamp;
    // `RuntimeScene`。
    let _ = engine::host::RuntimeScene::add_frame_system;
    let _ = engine::host::RuntimeScene::add_step_system;
    let _ = engine::host::RuntimeScene::asset_entries;
    let _ = engine::host::RuntimeScene::asset_handle;
    let _ = engine::host::RuntimeScene::asset_count;
    let _ = engine::host::RuntimeScene::camera_center_2d;
    let _ = engine::host::RuntimeScene::from_scene;
    let _ = engine::host::RuntimeScene::from_scene_edit;
    let _ = engine::host::RuntimeScene::gamepad_backend_name;
    let _ = engine::host::RuntimeScene::gamepad_connected;
    let _ = engine::host::RuntimeScene::last_particle_dispatch;
    let _ = engine::host::RuntimeScene::last_schedule_report;
    let _ = engine::host::RuntimeScene::last_spatial_report;
    let _ = engine::host::RuntimeScene::local_transform_of;
    let _ = engine::host::RuntimeScene::new;
    let _ = engine::host::RuntimeScene::new_with_layout;
    let _ = engine::host::RuntimeScene::pause_music;
    let _ = engine::host::RuntimeScene::propagate_spatial_now;
    let _ = engine::host::RuntimeScene::prefab_ui_rects;
    let _ = engine::host::RuntimeScene::quit_requested;
    let _ = engine::host::RuntimeScene::registered_system_names;
    let _ = engine::host::RuntimeScene::reload_scripts;
    let _ = engine::host::RuntimeScene::render_view_2d;
    let _ = engine::host::RuntimeScene::renderable_conflicts;
    let _ = engine::host::RuntimeScene::resume_music;
    let _ = engine::host::RuntimeScene::run_script_steps;
    let _ = engine::host::RuntimeScene::runtime_spawned_entities;
    let _ = engine::host::RuntimeScene::runtime_entity_count;
    let _ = engine::host::RuntimeScene::scene_3d;
    let _ = engine::host::RuntimeScene::scene_3d_with_camera;
    let _ = engine::host::RuntimeScene::scene_entity_count;
    let _ = engine::host::RuntimeScene::scene_entity_handles;
    let _ = engine::host::RuntimeScene::scene_entity_local_transforms;
    let _ = engine::host::RuntimeScene::scene_parent_issues;
    let _ = engine::host::RuntimeScene::script_diagnostics;
    let _ = engine::host::RuntimeScene::script_disabled_callbacks;
    let _ = engine::host::RuntimeScene::script_error_policy;
    let _ = engine::host::RuntimeScene::script_error_total;
    let _ = engine::host::RuntimeScene::set_camera_2d;
    let _ = engine::host::RuntimeScene::set_collider_debug_draw;
    let _ = engine::host::RuntimeScene::set_collider_debug_draw_3d;
    let _ = engine::host::RuntimeScene::set_font_atlas;
    let _ = engine::host::RuntimeScene::set_gamepad_backend;
    let _ = engine::host::RuntimeScene::set_gamepad_deadzone;
    let _ = engine::host::RuntimeScene::set_local_transform;
    let _ = engine::host::RuntimeScene::set_particle_compute_available;
    let _ = engine::host::RuntimeScene::set_render_ui_overlay;
    let _ = engine::host::RuntimeScene::set_scene_entity_transform_preview;
    let _ = engine::host::RuntimeScene::set_scene_entity_ui_layout_preview;
    let _ = engine::host::RuntimeScene::set_script_error_policy;
    let _ = engine::host::RuntimeScene::set_spatial_incremental;
    let _ = engine::host::RuntimeScene::set_spatial_index;
    let _ = engine::host::RuntimeScene::set_spatial_index_cell_size;
    let _ = engine::host::RuntimeScene::set_spatial_inheritance;
    let _ = engine::host::RuntimeScene::set_spatial_skip_unchanged;
    let _ = engine::host::RuntimeScene::set_sprite_culling;
    let _ = engine::host::RuntimeScene::set_sprite_parallelism;
    let _ = engine::host::RuntimeScene::set_system_parallelism;
    let _ = engine::host::RuntimeScene::set_text_render_mode;
    let _ = engine::host::RuntimeScene::set_tilemap_cell;
    let _ = engine::host::RuntimeScene::set_tilemap_culling;
    let _ = engine::host::RuntimeScene::set_ui_reference;
    let _ = engine::host::RuntimeScene::set_vector_atlas;
    let _ = engine::host::RuntimeScene::set_world_sprite_depth;
    let _ = engine::host::RuntimeScene::sprite_view_projection;
    let _ = engine::host::RuntimeScene::step_scene_emitters;
    let _ = engine::host::RuntimeScene::take_script_micros;
    let _ = engine::host::RuntimeScene::take_spatial_path_counts;
    let _ = engine::host::RuntimeScene::take_spatial_recomputed_total;
    let _ = engine::host::RuntimeScene::take_spatial_skipped_frames;
    let _ = engine::host::RuntimeScene::take_view_filtered;
    let _ = engine::host::RuntimeScene::ui_rects_by_name;
    let _ = engine::host::RuntimeScene::vertices;
    let _ = engine::host::RuntimeScene::view_filtered;
    let _ = engine::host::RuntimeScene::world_transform_2d_of;
    let _ = engine::host::RuntimeScene::world_transform_3d_of;
    let _ = engine::host::RuntimeScene::world_view_has_3d_content;
    // 自由函数（编辑器 `panels/game_view.rs` 直接用 `engine::host::load_project_font_atlas`）。
    let _ = engine::host::load_project_vector_atlas;
    let _ = engine::host::load_project_font_atlas;
    // `load_custom_*` 只在 `runtime_scene` 下 pub，`host/mod.rs` 没有把它抬到 `host::`——按现状 pin 规范路径。
    let _ = engine::host::runtime_scene::load_custom_vector_atlas;
    let _ = engine::host::runtime_scene::load_custom_font_atlas;
    // 其余对外类型（`host/mod.rs` 的 `pub use` 面）。
    let _: Option<rustlua_render::frame::BoundsSource> = None;
    let _: Option<rustlua_render::frame::OverlayVertex> = None;
    let _: Option<rustlua_render::frame::Scene3d> = None;
    let _: Option<rustlua_render::frame::Scene3dBounds> = None;
    let _: Option<rustlua_render::frame::Scene3dDrawable> = None;
    let _: Option<rustlua_render::frame::SceneDraw> = None;
    let _: Option<rustlua_render::frame::SceneMesh> = None;
    let _: Option<engine::host::FakeBackend> = None;
    let _: Option<Box<dyn engine::host::GamepadBackend>> = None;
    let _: Option<engine::host::GamepadDeadzone> = None;
    let _: Option<engine::host::GamepadFrame> = None;
    let _: Option<engine::host::GamepadPoller> = None;
    let _: Option<rustlua_gpu::ReadbackMetrics> = None;
    let _: Option<rustlua_gpu::ReadbackStatus> = None;
    let _: Option<rustlua_gpu::RenderFrameMetrics> = None;
    let _: Option<rustlua_gpu::RenderOutcome> = None;
    let _: Option<rustlua_gpu::RendererResourceStats> = None;
    let _: Option<engine::host::RuntimeScene> = None;
    let _: Option<engine::host::RuntimeSpawnedEntity> = None;
    let _: Option<engine::host::ScriptErrorPolicy> = None;
}

/// 入参是 `impl Trait` 的方法与自由函数（`scene_entity_ui_layout_rects` 收 `impl IntoIterator`、
/// `with_script` 收 `impl FnOnce`）取不出路径值，用一次**只编译、不运行**的调用钉住路径。
///
/// M77-01 之后 `dispatch_gpu_particle_bursts` 收 `impl ParticleRenderer`：这里把实参写成
/// `rustlua_gpu::Renderer`，同时钉住"trait 解耦后 GPU 渲染器依然接得上"这条接缝。
#[allow(dead_code)]
fn pin_impl_trait_methods(
    scene: &mut engine::host::RuntimeScene,
    renderer: &mut rustlua_gpu::Renderer,
    report: &engine::host::EnvironmentReport,
) {
    let _ = scene.scene_entity_ui_layout_rects([], [800.0, 600.0]);
    let _ = scene.with_script(|_runtime| 1_u32);
    let _ = scene.dispatch_gpu_particle_bursts(renderer, 1.0 / 60.0);
    let _ = engine::host::apply_scene_environment(renderer, scene, report);
}

#[test]
fn host_pub_surface_pins_compile() {
    pin_paths();
    // 这条门的全部断言就是"这个文件编译得过"：路径解析不了是编译错误，不是测试失败。
}
