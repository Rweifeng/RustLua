//! `runtime_scene` 的测试集合：WP12 阶段 1 从 `runtime_scene.rs` 原样搬来的 4 个
//! `#[cfg(test)] mod`（100 项测试）。断言、造数据与读盘路径一律没改——测试走
//! `crate::core::games_dir()`（= `CARGO_MANIFEST_DIR` 向上三级 = 仓库根），与源文件位置无关。
//!
//! 可见性依据（`tasks/done/WP12-host拆分.md` §3.1.5）：本目录里的每个模块都是
//! `runtime_scene` 的后代，`use super::*;` 的 glob 会把父模块的**私有项**一并带下来，
//! 所以搬迁没有把任何私有 helper 改成 `pub`。原 `mod sdf_tests` 顶部三个被多个域文件
//! 共用的 helper 提到本模块，域文件同样靠 `use super::*;` 取到它们。

use super::*;

// WP12 阶段 3b：这些名字原先由 `runtime_scene.rs` 顶部的 use 块带进来（测试靠 glob 取用）。
// 生产代码搬走后父模块的导入被裁掉，这里改由测试自己声明——断言与造数据的路径一字未改。
use crate::core::{
    layer::RenderLayer,
    math3d::Vec3,
    quad::Quad,
    resource_kind::ResourceKind,
    scheduler::{ScheduleStage, SchedulerError, System},
    script::ScriptErrorPolicy,
    sprite::Sprite,
    transform::Transform3D,
    ui::{UiImage, UiText},
    ui_layout::UiRect,
};
use crate::host::frame::SceneMesh;

// 示例重组后，展示工程从 examples/ 迁入 examples/game_api/（多项目布局）；
// 此处路径同步修正，恢复 showcase/ui_test 等测试的真实覆盖（M18-03）。
fn game_root() -> std::path::PathBuf {
    crate::core::games_dir().join("examples").join("game_api")
}

// 示例工程"逐场景实例化 + 步进"的门已迁到 `host::sample_project`
// （`sample_project_runs_every_scene_headless`）：它**从 scenes/ 目录读**场景清单，
// 因此新增/改名场景不再需要在这里手抄一份名单（名单漂移过：M18-03 与示例工程重建都踩过）。
// 那一组门还一并覆盖脚本编译、API 覆盖率、组件覆盖率与入口菜单接线。

fn production_sample_root() -> std::path::PathBuf {
    crate::core::games_dir().join("examples").join("game_api")
}

/// 写一个只含 `update()` 的临时工程脚本，返回路径（文件名带测试名，避免并行互踩）。
fn write_guard_script(name: &str, body: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("rustlua-{name}-{}.lua", std::process::id()));
    std::fs::write(&path, body).unwrap();
    path
}

/// 拆分批 8 的场景夹具：一台透视相机 + 一个网格（`scene_3d` 以 drawable 为准，没有网格就
/// 直接返回 `None`），可选第二台相机与 `pose=transform`。
fn frame_view_scene(
    name: &str,
    second_camera: Option<&str>,
    camera_payload: &str,
) -> crate::core::scene::Scene {
    let mut scene = crate::core::scene::Scene::new(name);
    let camera = crate::core::scene::SceneEntityId(1);
    let mesh = crate::core::scene::SceneEntityId(2);
    scene.add_entity(camera, "Camera", None).unwrap();
    scene.add_entity(mesh, "Mesh", None).unwrap();
    scene
        .entities
        .get_mut(&camera)
        .unwrap()
        .components
        .insert("Camera".into(), camera_payload.into());
    let entity = scene.entities.get_mut(&mesh).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity
        .components
        .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
    if let Some(payload) = second_camera {
        let second = crate::core::scene::SceneEntityId(3);
        scene.add_entity(second, "Camera2", None).unwrap();
        scene
            .entities
            .get_mut(&second)
            .unwrap()
            .components
            .insert("Camera".into(), payload.into());
    }
    scene
}

mod animation_layers;
mod camera_physics;
mod custom_fonts;
mod frame_extraction;
mod frame_ledger;
mod frame_view;
mod gamepad_input;
mod render_channels;
mod runtime_snapshot;
mod scene_instantiation;
mod scenes_and_clicks;
mod schedule;
mod script_policy;
mod spatial;
mod tilemap;
mod transforms;
mod ui;
mod ui_layout;
mod world_depth;
