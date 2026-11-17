//! `crate::script` 的测试（WP11 阶段 4）：按域分文件，共用脚手架留在这里。
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

/// 领域接线（能力单元 + 绑定表 + 组件装配器）的测试版。
///
/// 正式的组合根在 `engine::host::script_packs`（见 `docs/62` P1）：本 crate 的单元测试不能
/// 反向依赖 `engine`（那会绕回自己），所以这里把同一套接线手写一遍。这本身就是依赖倒置的
/// 证明——脚本层不认识这些领域，只要有人把"能力单元 + 安装器 + 组件装配器"交进来。
///
/// 忘记接线的后果是**静默**的：表缺失（脚本报 nil index）、场景里的组件被当成未知组件跳过。
/// 所以凡是要实例化带这些组件的场景、或要跑 `camera.*` / `animation.*` 的测试，都必须走它。
pub(crate) fn runtime_with_domains() -> (
    LuaRuntime,
    rustlua_lua_api::Handle<rustlua_camera::camera_state::CameraState>,
    rustlua_lua_api::Handle<rustlua_animation::animation_lua::AnimationState>,
) {
    let camera = rustlua_camera::camera_state::CameraState::handle();
    let animation = rustlua_animation::animation_lua::AnimationState::handle();
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();

    let camera_installer = camera.clone();
    runtime
        .add_deferred_installer(Box::new(move |lua, world, registry| {
            rustlua_camera::camera_lua::install(
                lua,
                &camera_installer,
                world,
                registry.pointer_cell(),
            )?;
            #[cfg(feature = "3d")]
            rustlua_camera::camera_lua::install_3d(
                lua,
                &camera_installer,
                world,
                registry.pointer_cell(),
            )?;
            Ok(())
        }))
        .expect("相机表安装");
    let animation_installer = animation.clone();
    runtime
        .add_deferred_installer(Box::new(move |lua, world, _registry| {
            rustlua_animation::animation_lua::install(lua, &animation_installer, world)?;
            #[cfg(feature = "3d")]
            rustlua_animation::animation_lua::install_3d(lua, &animation_installer, world)?;
            Ok(())
        }))
        .expect("动画表安装");

    runtime.add_component_applier("Camera", |world, id, value, _| {
        let camera = rustlua_camera::camera::Camera::from_component_str(value)
            .map_err(|error| error.to_string())?;
        world
            .insert(id, camera)
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    runtime.add_component_applier("Animation", |world, id, value, _| {
        let machine = rustlua_animation::animation::AnimationStateMachine::from_component_str(value)
            .map_err(|error| error.to_string())?;
        world.insert(id, machine).map_err(|error| error.to_string())?;
        Ok(())
    });
    runtime.add_component_applier("AnimationPlayer", |world, id, value, project_root| {
        let mut player =
            rustlua_animation::animation_player::AnimationPlayer::from_component_str(value)
                .map_err(|error| error.to_string())?;
        let root = project_root.ok_or_else(|| {
            "needs a project root to load its AnimationPlayer clip".to_owned()
        })?;
        let clip_path = root.join(player.clip_path());
        let text = rustlua_infra::vfs::read_resource(&clip_path)
            .and_then(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
            })
            .map_err(|error| {
                format!("AnimationPlayer failed to read {}: {error}", player.clip_path())
            })?;
        let clip = rustlua_animation::anim_asset::AnimClip::from_text(player.clip_path(), &text)
            .map_err(|error| format!("invalid clip {}: {error}", player.clip_path()))?;
        player
            .attach_clip(clip)
            .map_err(|error| format!("AnimationPlayer cannot use {}: {error}", player.clip_path()))?;
        world.insert(id, player).map_err(|error| error.to_string())?;
        Ok(())
    });
    runtime.add_component_applier("Animation3D", |world, id, value, project_root| {
        let descriptor =
            rustlua_animation::animation3d::Animation3dDescriptor::from_component_str(value)
                .map_err(|error| error.to_string())?;
        let root = project_root
            .ok_or_else(|| "needs a project root to load its Animation3D mesh".to_owned())?;
        let animation =
            rustlua_animation::animation3d::Animation3D::from_gltf_path(root, &descriptor)
                .map_err(|error| error.to_string())?;
        world
            .insert(id, animation)
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    (runtime, camera, animation)
}

/// 只需要相机接线时的简写（相机测试与 API 面快照用它）。
pub(crate) fn runtime_with_camera() -> (
    LuaRuntime,
    rustlua_lua_api::Handle<rustlua_camera::camera_state::CameraState>,
) {
    let (runtime, camera, _) = runtime_with_domains();
    (runtime, camera)
}

use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

mod animation;
mod api_surface;
mod audio;
mod camera;
mod diagnostics;
/// 门面棘轮：`script/` 的生产代码只经 `domain` 认领域 crate（见 `script/domain.rs`）。
mod domain_facade;
mod entity;
mod globals;
mod graph_runtime;
mod hot_reload;
mod installers;
mod particles;
mod physics;
mod physics_handle;
mod reentrancy;
mod render_tuning;
mod sample_project;
mod save_data;
mod scene_spawn;
mod texture_gen;
mod tilemap;
mod transform;
mod ui;
mod world_handle;

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
