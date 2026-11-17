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
use rustlua_physics::physics_state::PhysicsState;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::world::World;
use rustlua_lua_api::Handle;
use rustlua_scripting::script::LuaRuntime;
use std::rc::Rc;

/// 宿主需要留着自己驱动的那部分领域句柄。
///
/// 相机每帧被推进/读取（`step_camera2d` / 帧末回填投影），动画每步要把帧事件推进一次性
/// 队列——两者都必须由组合根握住同一个 `Handle`，宿主与 Lua 绑定看的是同一份状态。
pub struct ScriptDomainHandles {
    pub camera: Handle<CameraState>,
    pub animation: Handle<AnimationState>,
    pub physics: Handle<PhysicsState>,
    /// UI 动态层（文本/条/面板/图/网格/视图模型/隐藏集）。宿主每帧驱动 UI 渲染与绑定，
    /// Lua 的 `ui.*` 绑定写的是同一份；组合根建句柄、经延迟安装器交给绑定。
    pub ui: Handle<rustlua_ui::ui_state::UiState>,
    /// UI 导航栈（焦点环/滚动位置）。
    pub nav: Handle<rustlua_ui::lua_nav::NavState>,
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

    // ⑦ 物理域：`tilemap` / `physics` / `physics3d` 三张表。tilemap 绑定还要实体账本，
    //    那个句柄从 LuaRuntime 取（ecs 能力单元，不新增依赖边）。
    let physics = PhysicsState::handle();
    let entities = script.entity_state_handle();
    let physics_installer = physics.clone();
    let entities_installer = entities.clone();
    script
        .add_deferred_installer(Box::new(move |lua, world, _registry| {
            rustlua_physics::tilemap_lua::install(
                lua,
                world,
                &entities_installer,
                &physics_installer,
            )?;
            rustlua_physics::physics_lua::install(lua, &physics_installer, world)?;
            #[cfg(feature = "3d")]
            rustlua_physics::physics_lua::install_3d(lua, &physics_installer, world)?;
            Ok(())
        }))
        .expect("物理表安装失败：VM 已建好却装不进绑定，脚本会拿不到 physics.*");

    // ⑧ 刚体装配：ECS 世界里只有 `Transform` / `Tilemap`，真正的碰撞体住在 PhysicsState
    //    自己的两个模拟世界里。装配器同时读 ECS、写物理世界——这正是它必须在组合根的原因。
    let applier = physics.clone();
    script.add_component_applier("Collider", move |world, id, value, _| {
        let collider = rustlua_physics::physics::ColliderDesc::from_component_str(value)
            .map_err(|error| error.to_string())?;
        let position = world
            .get::<rustlua_ecs::transform::Transform>(id)
            .ok()
            .flatten()
            .map(|transform| transform.position)
            .unwrap_or_default();
        applier.with_mut(|state| {
            state
                .world2d_mut()
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| error.to_string())
        })
    });
    let applier = physics.clone();
    script.add_component_applier("Tilemap", move |world, id, _, _| {
        // ECS 的 `Tilemap` 组件由脚本层的手写分支先插入（在装配器循环之前），这里只加它的
        // 固态格对应的静态刚体——所以是"装配后副作用"，不是组件本体的装配。
        let Some(tilemap) = world.get::<rustlua_ecs::tilemap::Tilemap>(id).ok().flatten() else {
            return Ok(());
        };
        if !tilemap.solid.iter().any(|solid| *solid) {
            return Ok(());
        }
        let colliders = rustlua_physics::tilemap_lua::solid_cell_colliders(tilemap);
        let position = world
            .get::<rustlua_ecs::transform::Transform>(id)
            .ok()
            .flatten()
            .map(|transform| transform.position)
            .unwrap_or_default();
        applier.with_mut(|state| {
            state
                .world2d_mut()
                .add_body(id, position, false, colliders)
                .map_err(|error| error.to_string())
        })
    });
    let applier = physics.clone();
    script.add_component_applier("Collider3d", move |world, id, value, _| {
        let collider = rustlua_physics::physics3d::ColliderDesc3d::from_component_str(value)
            .map_err(|error| error.to_string())?;
        let position = world
            .get::<rustlua_ecs::transform::Transform3D>(id)
            .ok()
            .flatten()
            .map(|transform| transform.position)
            .unwrap_or(rustlua_math::math3d::Vec3::ZERO);
        applier.with_mut(|state| {
            state
                .world3d_mut()
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| error.to_string())
        })
    });

    // ⑨ 切场景：两个模拟世界连同回写队列、调试请求一起重建。
    let transition = physics.clone();
    script.add_scene_transition_hook(move || {
        transition.with_mut(|state| state.clear_for_scene_transition());
    });

    // ── ⑩ UI 域：七处 Lua 绑定 ────────────────────────────────────────────
    // `ui` 表由脚本层在建 VM 时创建并存进 globals（还装了 measure_text）；每次建 VM 都是
    // 一张新表。因此这里在**回调内现场取表**再补键，绝不捕获 Table 句柄——上次把取表放在
    // 注册期，热重载重建 VM 后就把 ui.image 等装进了旧表，脚本读到 nil。
    // 能力单元由组合根创建（P1 ⑤-12 后脚本层不再持有 UI 状态）；同一份句柄在这里装给
    // 绑定、经 `ScriptDomainHandles` 交给宿主的每帧读写——两边各造一份会让"脚本写的
    // 文字宿主读不到"。设计基准尺寸也在这里登记一次：视口未配置时按它归一布局。
    let ui = rustlua_ui::ui_state::UiState::handle();
    ui.with_mut(|state| state.set_reference(rustlua_ui::ui::UI_REFERENCE_SIZE));
    let nav = rustlua_ui::lua_nav::NavState::handle();
    let ui_installer = ui.clone();
    let nav_installer = nav.clone();
    let prefab_host = script.prefab_host();
    script
        .add_deferred_installer(Box::new(move |lua, world, _registry| {
            // `world_text` 与 `prefab` 是独立全局表，不写进 ui 表。world 句柄由回调参数给出。
            rustlua_ui::world_text_lua::install(lua, world)?;
            rustlua_ui::prefab_lua::install(lua, Rc::clone(&prefab_host))?;
            // 现场取脚本层刚建好的 ui 表（引用语义，克隆只是多一个句柄）。
            let table: rustlua_lua_api::Table = lua.globals().get("ui")?;
            rustlua_ui::ui_lua::install(lua, table.clone(), &ui_installer)?;
            rustlua_ui::ui_panel_lua::install(lua, table.clone(), &ui_installer)?;
            rustlua_ui::ui_controls_lua::install(lua, table.clone(), world)?;
            rustlua_ui::lua_nav::install(lua, &table, &nav_installer)?;
            rustlua_ui::lua_advanced::install(lua, &table, &ui_installer)?;
            Ok(())
        }))
        .expect("UI 表安装失败：VM 已建好却装不进绑定，脚本会拿不到 ui.* / prefab / world_text");

    // ── ⑪ interact.prompt / clear_prompt / get_prompt ─────────────────────
    // 这三支要同时写 ECS 账本的文案与 UI 覆盖层文本（`__interact_prompt`），是跨域操作，
    // 所以归组合根；`interact` 表本身由 ECS 域建好，这里现场取表补最后三个键。
    // 回调可能被多次重放（热重载重建 VM），内层闭包捕获的 handle 必须每次 clone 局部副本。
    let prompt_entities = script.entity_state_handle();
    let prompt_ui = ui.clone();
    let prompt_ui_clear = ui.clone();
    script
        .add_deferred_installer(Box::new(move |lua, _, _| {
            let interact: rustlua_lua_api::Table = lua.globals().get("interact")?;
            let entities = prompt_entities.clone();
            let ui = prompt_ui.clone();
            interact.set(
                "prompt",
                lua.create_function(move |_, text: String| {
                    entities.borrow_mut()?.interact_prompt = Some(text.clone());
                    ui.borrow_mut()?.texts_mut().insert(
                        "__interact_prompt".to_owned(),
                        rustlua_ui::ui_state::UiText {
                            text,
                            visible: true,
                            position: [16.0, 48.0],
                            scale: 1.0,
                            font_size: Some(18.0),
                        },
                    );
                    Ok(())
                })?,
            )?;
            let entities = prompt_entities.clone();
            let ui = prompt_ui_clear.clone();
            interact.set(
                "clear_prompt",
                lua.create_function(move |_, ()| {
                    entities.borrow_mut()?.interact_prompt = None;
                    ui.borrow_mut()?.texts_mut().remove("__interact_prompt");
                    Ok(())
                })?,
            )?;
            let entities = prompt_entities.clone();
            interact.set(
                "get_prompt",
                lua.create_function(move |_, ()| {
                    Ok(entities.borrow()?.interact_prompt.clone())
                })?,
            )?;
            Ok(())
        }))        .expect("interact.prompt 安装失败：表由 ECS 域建好，取不到说明装配顺序变了");

    // 切场景：UI 动态层与导航栈的实体 id 随世界作废（viewport/reference 保留）。
    let ui_transition = ui.clone();
    let nav_transition = nav.clone();
    script.add_scene_transition_hook(move || {
        if let Ok(mut state) = ui_transition.borrow_mut() {
            state.clear_for_scene_transition(&nav_transition);
        }
    });

    // ── UI 域的 15 类场景组件 ──────────────────────────────────────────────
    // 这些原来是 `scene_spawn::spawn_entity_components` 里写死的 if 链，每支直接构造一个
    // `rustlua_ui` 组件。组件类型住在 UI 域，装配知识因此归组合根；脚本层只认组件名。
    // 解析口径与编辑器导出保持一致（`from_component_str`），错误文案由外层 applier 循环
    // 统一加 `entity {name} has an invalid {component}` 前缀。
    macro_rules! ui_applier {
        ($component:literal, $type:path) => {
            script.add_component_applier($component, |world, id, value, _| {
                let component = <$type>::from_component_str(value)
                    .map_err(|error| error.to_string())?;
                world
                    .insert(id, component)
                    .map_err(|error| error.to_string())?;
                Ok(())
            });
        };
    }
    ui_applier!("Text", rustlua_ui::ui::UiText);
    ui_applier!("Image", rustlua_ui::ui::UiImage);
    ui_applier!("WorldText", rustlua_ui::world_text::WorldText);
    ui_applier!("UiBindings", rustlua_ui::ui_binding::UiBindings);
    ui_applier!("ScrollView", rustlua_ui::ui_scroll_view::ScrollView);
    ui_applier!("ButtonStates", rustlua_ui::ui_states::ButtonStateSet);
    ui_applier!("Mask", rustlua_ui::ui_mask::Mask);
    ui_applier!("CanvasGroup", rustlua_ui::ui_canvas_group::CanvasGroup);
    ui_applier!("Toggle", rustlua_ui::ui_toggle::Toggle);
    ui_applier!("Slider", rustlua_ui::ui_slider::Slider);
    ui_applier!("Progress", rustlua_ui::ui_progress::Progress);
    ui_applier!("Scrollbar", rustlua_ui::ui_scrollbar::ScrollBar);
    ui_applier!("UiVirtualList", rustlua_ui::ui_virtual_list::UiVirtualList);
    ui_applier!("UiLayout", rustlua_ui::ui_layout::UiRectLayout);
    // `UiEvents` 的解析入口叫 `parse`（不是 `from_component_str`），单列一支。
    script.add_component_applier("UiEvents", |world, id, value, _| {
        let events = rustlua_ui::ui_events::UiEvents::parse(value)
            .map_err(|error| error.to_string())?;
        world
            .insert(id, events)
            .map_err(|error| error.to_string())?;
        Ok(())
    });

    ScriptDomainHandles {
        camera,
        animation,
        physics,
        ui,
        nav,
    }
}
