//! `core::script` 的测试（WP11 阶段 4）：按域分文件，共用脚手架留在这里。
//!
//! 每个域文件只写一行 `use super::*;`——glob 把本模块的脚手架、以及本模块从 `script`
//! 拿到的私有项一并带过去（私有项对后代模块可见，glob 是传递的）。
//! 拆分时**测试项数不得减少**：`cargo test -p engine --lib` 的总数就是这条门的读数。

use super::*;

use rustlua_animation::{animation::AnimationStateMachine, animation3d::Animation3D};
use rustlua_ecs::transform::Transform3D;
use rustlua_render::{
    light::Light,
    mesh3d::MeshRenderer,
    sprite::{Sprite, SpriteAsset},
};
use rustlua_scene::scene::Scene;
use rustlua_ui::ui_events::UiClickTarget;

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

mod animation;
mod api_surface;
mod audio;
mod camera;
mod diagnostics;
mod entity;
mod globals;
mod hot_reload;
// TODO: input tests reference `engine::host::gamepad` — relocate to engine/tests/
#[cfg(any())]
mod input;
mod particles;
mod physics;
mod sample_project;
mod save_data;
mod scene_spawn;
mod tilemap;
mod transform;
mod ui;

/// 一帧「左摇杆上推」输入（含数字态，模拟 `GamepadPoller::poll_into` 的产物）。
#[allow(dead_code)] // 唯一调用方在 `mod input` 里，该模块因迁移暂时被 [cfg(any())] 关掉
fn held_stick_up(value: f32) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.set_gamepad_axis(1, value);
    input.set_gamepad_axis_digital(1, rustlua_input::input::AxisDirection::Positive, true);
    input
}
fn script(contents: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    // 进程内单调计数 + 纳秒时间戳：并行测试在同一时间刻度内调用时
    // 也不会拿到同一路径（曾因纯时间戳碰撞互相覆盖脚本导致偶发失败）。
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("rustlua-lua-{id}-{unique}.lua"));
    fs::write(&path, contents).unwrap();
    path
}

fn production_sample_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

/// 构造一个同时带 2D 与 3D 实体的场景，供统一 `transform` 测试复用。
///
/// 空间由实体携带的**其它组件**推断（`spatial_transform::entity_space`），所以 3D 那个
/// 必须真的带一个 3D 专属组件（这里是 `MeshRenderer`）——否则它会被判成 2D。
fn unified_transform_scene() -> rustlua_scene::scene::Scene {
    use rustlua_scene::scene::Scene;
    use rustlua_scene::scene::SceneEntityId;
    let mut scene = Scene::new("unified-transform-probe");
    scene.add_entity(SceneEntityId(1), "Flat", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene.add_entity(SceneEntityId(2), "Solid", None).unwrap();
    {
        let components = &mut scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components;
        components.insert(
            "Transform".into(),
            "position=1,2,3;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        components.insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
    }
    scene.add_entity(SceneEntityId(3), "Bare", None).unwrap();
    scene
}
