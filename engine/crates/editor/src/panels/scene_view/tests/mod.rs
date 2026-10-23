//! `scene_view` 的回归测试：指针状态机、gizmo 命中与拖动、投影/瓦片映射、
//! UI 包围盒，以及样例工程的端到端绘制。

use super::*;
use crate::EditorApp;
use crate::EditorCamera;
use editor::open_project;
use rustlua_infra::logging::LogSink;
use rustlua_ui::ui_layout::UiLayoutPreset;

// ---- E11：交互入口（`scene_pointer_*`）的独立单测 ----
//
// 这三个方法是从 `draw_scene_2d` 里抽出来的，就是为了让"绘制与交互可独立测试"落地：
// 不需要 egui 帧、不需要 GPU，直接给定实体包围框与指针位置即可断言相位与副作用。

fn interaction_entity(id: u64, center: egui::Pos2) -> SceneBoxedEntity {
    SceneBoxedEntity {
        id: SceneEntityId(id),
        name: format!("E{id}"),
        transform: TransformFields::default(),
        center,
        size: egui::vec2(40.0, 40.0),
        ui: None,
    }
}

/// 只关心朝向的旋转结果（生产路径 `gizmo_rotate_about` 的 `Pivot` 分支）。
///
/// 轴固定喂 `Vec3::Y`：这里的断言谈的是"世界 vs 局部"，三支环共用同一段代码，
/// 换一根轴只是把 `Y` 换成 `X/Z`，不需要在这里重复一遍。
fn rotate_only(
    space: crate::scene_interaction::GizmoSpace,
    rotation: rustlua_math::math3d::Quat,
    angle: f32,
) -> rustlua_math::math3d::Quat {
    crate::gizmo::rotate_about(
        space,
        Vec3::Y,
        &rustlua_ecs::transform::Transform3D {
            position: Vec3::new(0.0, 0.0, 0.0),
            rotation,
            scale: Vec3::new(1.0, 1.0, 1.0),
        },
        None,
        angle,
    )
    .1
}
use rustlua_ecs::transform::Vec2;
use rustlua_scene::scene::SceneEntityId;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos()
}

/// Headless wgpu 设备/队列（fallback adapter）；无 GPU 时返回 None，测试跳过。
fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-scene-view-gpu-test"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rustlua-m14-01-scene-view-{label}-{}",
        unique_token()
    ))
}

fn sample_project_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

fn copy_dir(source: &Path, destination: &Path) {
    if !source.is_dir() {
        return;
    }
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target_path).unwrap();
            copy_dir(&source_path, &target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&source_path, &target_path).unwrap();
        }
    }
}

fn copy_sample_project(label: &str) -> PathBuf {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    fs::create_dir_all(&root).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    copy_dir(&source.join("assets"), &root.join("assets"));
    root
}

// ---- E11：交互状态机是生产路径的事实源（docs/55 ER-07 首批接线） ----

fn interaction_app() -> EditorApp {
    let root = copy_sample_project("interaction-state");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    app
}

mod entities;
mod gizmo;
mod interaction;
mod projection;
mod render;
mod sample_project;
mod tilemap;
mod ui_source_layout;
