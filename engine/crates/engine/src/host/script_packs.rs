//! 领域 Lua 绑定与组件的**组合根**（`docs/62` P1 依赖倒置的落点）。
//!
//! ## 为什么这一段在 `engine` 而不是 `rustlua-scripting`
//!
//! 相机域早就是"绑定住在归属 crate"的形状（`rustlua_camera::camera_lua`），但
//! `rustlua-scripting` 仍然钉着三条边：
//!
//! 1. `ScriptState` 里存 `Handle<CameraState>`；
//! 2. `api::register_all` 里调 `camera_lua::install`（要世界，而世界在 `ScriptState` 里）；
//! 3. `spawn_entity_components` 里写死 `components.get("Camera")`，直接构造 `Camera` 组件；
//!
//! 只要这三处还在，"绑定下沉"就只下沉了一半：删掉绑定文件也删不掉 Cargo.toml 里那行。
//! 现在三处都由本模块接管：
//!
//! - 能力单元在这里创建，`rustlua-scripting` 拿不到它的类型；
//! - VM 装配期由 [`LuaRuntime::add_deferred_installer`] 回调（世界作为参数传进来）；
//! - 场景装配期由 [`LuaRuntime::add_component_applier`] 按组件名回调；
//! - 切场景清理由 [`LuaRuntime::add_scene_transition_hook`] 回调。
//!
//! ## 为什么宿主必须调这个函数
//!
//! 不调不会报错，只会让 `camera` / `camera2d` / `camera3d` 三张表**静默缺失**（脚本报
//! `attempt to index a nil value`），场景里声明的 `Camera` 组件也会被当成未知组件跳过。
//! 所以凡是自己建 `LuaRuntime` 的宿主（引擎运行时、示例工程、编辑器预览、离线工具）
//! 都必须走 [`install`]；`tests/installers.rs` 那条 API 面断言就是防这个漏的。

use rustlua_animation::animation_lua::AnimationState;
use rustlua_camera::camera_state::CameraState;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::World;
use rustlua_lua_api::Handle;
use rustlua_scripting::script::LuaRuntime;

/// 宿主需要留着自己驱动的那部分领域句柄。
///
/// 相机每帧被推进/读取（`step_camera2d` / 帧末回填投影），动画每步要把帧事件推进一次性
/// 队列——两者都必须由组合根握住同一个 `Handle`，宿主与 Lua 绑定看的是同一份状态。
pub struct ScriptDomainHandles {
    pub camera: Handle<CameraState>,
    pub animation: Handle<AnimationState>,
}

/// 建一个**带全部领域绑定**的运行时：`LuaRuntime::with_installers(..)` + [`install`]。
///
/// 凡是自己建 `LuaRuntime` 的宿主都该走这里。直接用 `with_installers` 不是编译错误，代价是
/// `camera*` 三张表与 `Camera` 组件静默消失（见模块文档）——那种失败在编辑器面板里表现为
/// "脚本突然报 nil index"，在没有脚本的离线工具里则完全看不见。
pub fn runtime(
    installers: std::rc::Rc<rustlua_lua_api::Registry>,
) -> Result<(LuaRuntime, ScriptDomainHandles), rustlua_scripting::script::LuaBridgeError> {
    let mut script = LuaRuntime::with_installers(installers)?;
    let handles = install(&mut script);
    Ok((script, handles))
}

/// 把当前这批领域绑定/组件/清理钩子接到 `script` 上。
///
/// 顺序无关紧要：安装器只装全局表，装配器只按名字接管组件，钩子只在切场景时跑。
pub fn install(script: &mut LuaRuntime) -> ScriptDomainHandles {
    let camera = CameraState::handle();

    // ① Lua 表：`camera` / `camera2d`（基底）与 `camera3d`（3D 包）。
    let installer = camera.clone();
    script
        .add_deferred_installer(Box::new(move |lua, world, registry| {
            rustlua_camera::camera_lua::install(lua, &installer, world, registry.pointer_cell())?;
            #[cfg(feature = "3d")]
            rustlua_camera::camera_lua::install_3d(
                lua,
                &installer,
                world,
                registry.pointer_cell(),
            )?;
            Ok(())
        }))
        .expect("相机表安装失败：VM 已建好却装不进绑定，脚本会拿不到 camera.*");

    // ② 场景里的 `Camera` 组件：类型知识在相机域，本层只按名字转交。
    //
    // 「唯一的 `Camera` 组件」这条不变量没变：投影由判别字段给出，落进世界的就是这一个
    // 组件类型，消费方按投影分支取值（`as_orthographic` / `as_perspective`）。
    script.add_component_applier(
        "Camera",
        |world: &mut World, id: EntityId, value: &str, _project_root| {
            let camera = rustlua_camera::camera::Camera::from_component_str(value)
                .map_err(|error| error.to_string())?;
            world
                .insert(id, camera)
                .map_err(|error| error.to_string())?;
            Ok(())
        },
    );

    // ③ 切场景：选中项记的是实体 id，换世界后必须作废（控制器与每帧快照保留）。
    let transition = camera.clone();
    script.add_scene_transition_hook(move || {
        transition.with_mut(|state| state.clear_for_scene_transition());
    });

    // ④ 动画域：`animation` / `animation3d` 两张表 + 帧事件的一次性队列。
    let animation = AnimationState::handle();
    let animation_installer = animation.clone();
    script
        .add_deferred_installer(Box::new(move |lua, world, _registry| {
            rustlua_animation::animation_lua::install(lua, &animation_installer, world)?;
            // M81：animation3d 属 3D 包，2D 构建不登记（与原来的 API 面基线一致）。
            #[cfg(feature = "3d")]
            rustlua_animation::animation_lua::install_3d(lua, &animation_installer, world)?;
            Ok(())
        }))
        .expect("动画表安装失败：VM 已建好却装不进绑定，脚本会拿不到 animation.*");

    // ⑤ 场景里的三类动画组件。资源路径解析也在这里完成：组合根认识工程根，脚本层不认识。
    script.add_component_applier("Animation", |world, id, value, _| {
        let machine =
            rustlua_animation::animation::AnimationStateMachine::from_component_str(value)
                .map_err(|error| error.to_string())?;
        world.insert(id, machine).map_err(|error| error.to_string())?;
        Ok(())
    });
    script.add_component_applier("AnimationPlayer", |world, id, value, project_root| {
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
    script.add_component_applier("Animation3D", |world, id, value, project_root| {
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

    // ⑥ 切场景：旧实体 id 已失效，帧事件与命名事件都随世界一起作废。
    let animation_transition = animation.clone();
    script.add_scene_transition_hook(move || {
        animation_transition.with_mut(|state| state.clear_for_scene_transition());
    });

    ScriptDomainHandles { camera, animation }
}
