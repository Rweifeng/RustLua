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
use std::collections::BTreeSet;

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
    rustlua_lua_api::Handle<rustlua_physics::physics_state::PhysicsState>,
    rustlua_lua_api::Handle<rustlua_ui::ui_state::UiState>,
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
    // 物理域：三张表 + 刚体装配。tilemap 绑定要实体账本，句柄从 runtime 取。
    let physics = rustlua_physics::physics_state::PhysicsState::handle();
    let entities = runtime.entity_state_handle();
    let physics_installer = physics.clone();
    let entities_installer = entities.clone();
    runtime
        .add_deferred_installer(Box::new(move |lua, world, _| {
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
        .expect("物理表安装");
    let applier = physics.clone();
    runtime.add_component_applier("Collider", move |world, id, value, _| {
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
    runtime.add_component_applier("Tilemap", move |world, id, _, _| {
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
    runtime.add_component_applier("Collider3d", move |world, id, value, _| {
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

    // UI 域的 15 类场景组件：与生产组合根 `engine::host::script_packs` 同一套装配，
    // 测试不能反向依赖 engine，所以在这里手写一遍。
    install_ui_appliers(&mut runtime);
    // UI 域能力单元由测试组合根创建（生产侧同款，见 `engine::host::script_packs`）：
    // 脚本层已不再持有 UI 状态，同一份句柄必须从这里传给绑定与断言两侧。
    let ui = rustlua_ui::ui_state::UiState::handle();
    let nav = rustlua_ui::lua_nav::NavState::handle();
    // UI 域的七处 Lua 绑定（ui.* / prefab / world_text）：同样镜像生产组合根。
    install_ui_bindings(&mut runtime, &ui, &nav);

    (runtime, camera, animation, physics, ui)
}

/// UI 域的七处 Lua 绑定（测试版，镜像 `engine::host::script_packs::install` 的 UI 段）。
///
/// 关键约定（上次回归的教训）：`ui` 表每次建 VM 都是一张新表，取表必须放在**回调内现场**做，
/// 绝不能在注册期捕获 `Table` 句柄——热重载重建 VM 后捕获的旧表会让脚本读到 nil。
fn install_ui_bindings(
    runtime: &mut LuaRuntime,
    ui: &rustlua_lua_api::Handle<rustlua_ui::ui_state::UiState>,
    nav: &rustlua_lua_api::Handle<rustlua_ui::lua_nav::NavState>,
) {
    ui.with_mut(|state| state.set_reference(rustlua_ui::ui::UI_REFERENCE_SIZE));
    // `prefab.set_text` / `prefab.set_rect` 的世界写入端口（镜像生产组合根的同一对闭包）。
    runtime.set_prefab_ui_helpers(
        |world, id, text| {
            let Ok(mut world) = world.borrow_mut() else {
                return false;
            };
            world
                .get_mut::<rustlua_ui::ui::UiText>(id)
                .ok()
                .flatten()
                .map(|component| component.text = text.to_owned())
                .is_some()
        },
        |world, id, raw, edges| {
            let Ok(mut world) = world.borrow_mut() else {
                return None;
            };
            let mut layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(raw).ok()?;
            layout.offsets = edges;
            let updated = layout.to_component_str();
            world.insert(id, layout).ok()?;
            Some(updated)
        },
    );
    let prefab_host = runtime.prefab_host();
    let entities_source = runtime.entity_state_handle();
    let ui_installer = ui.clone();
    let nav_installer = nav.clone();
    let entities_installer = entities_source.clone();
    runtime
        .add_deferred_installer(Box::new(move |lua, world, _registry| {
            // `world_text` 与 `prefab` 是独立全局表，不写进 ui 表。world 句柄由回调参数给出。
            rustlua_ui::world_text_lua::install(lua, world)?;
            rustlua_ui::prefab_lua::install(lua, Rc::clone(&prefab_host))?;
            let table: rustlua_lua_api::Table = lua.globals().get("ui")?;
            rustlua_ui::ui_lua::install(lua, table.clone(), &ui_installer)?;
            rustlua_ui::ui_panel_lua::install(lua, table.clone(), &ui_installer)?;
            rustlua_ui::ui_controls_lua::install(lua, table.clone(), world)?;
            rustlua_ui::lua_nav::install(lua, &table, &nav_installer)?;
            rustlua_ui::lua_advanced::install(lua, &table, &ui_installer)?;
            // interact.prompt / clear_prompt / get_prompt：跨 ECS+UI 的三个键，镜像生产组合根。
            let entities = entities_installer.clone();
            let ui = ui_installer.clone();
            let interact: rustlua_lua_api::Table = lua.globals().get("interact")?;
            let entities_for_clear = entities.clone();
            let ui_for_clear = ui.clone();
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
            interact.set(
                "clear_prompt",
                lua.create_function(move |_, ()| {
                    entities_for_clear.borrow_mut()?.interact_prompt = None;
                    ui_for_clear
                        .borrow_mut()?
                        .texts_mut()
                        .remove("__interact_prompt");
                    Ok(())
                })?,
            )?;
            let entities = entities_installer.clone();
            interact.set(
                "get_prompt",
                lua.create_function(move |_, ()| {
                    Ok(entities.borrow()?.interact_prompt.clone())
                })?,
            )?;
            Ok(())
        }))
        .expect("UI 表安装失败：VM 已建好却装不进绑定，脚本会拿不到 ui.* / prefab / world_text");
}

/// 15 类 UI 组件装配器（测试版，镜像 `engine::host::script_packs::install` 的 UI 段）。
fn install_ui_appliers(runtime: &mut LuaRuntime) {
    macro_rules! ui_applier {
        ($component:literal, $type:path) => {
            runtime.add_component_applier($component, |world, id, value, _| {
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
    runtime.add_component_applier("UiEvents", |world, id, value, _| {
        let events =
            rustlua_ui::ui_events::UiEvents::parse(value).map_err(|error| error.to_string())?;
        world
            .insert(id, events)
            .map_err(|error| error.to_string())?;
        Ok(())
    });
}

/// 只需要相机接线时的简写（相机测试与 API 面快照用它）。
pub(crate) fn runtime_with_camera() -> (
    LuaRuntime,
    rustlua_lua_api::Handle<rustlua_camera::camera_state::CameraState>,
) {
    let (runtime, camera, ..) = runtime_with_domains();
    (runtime, camera)
}

/// 需要 UI 域句柄做断言时的简写：返回的包装经 `Deref` 仍是 `LuaRuntime`，VM 调用一行不改；
/// `with_ui` 等宿主视图方法由本类型用**同一份**句柄直读（脚本层不再有这些入口）。
pub(crate) struct UiRuntime {
    runtime: LuaRuntime,
    pub(crate) ui: rustlua_lua_api::Handle<rustlua_ui::ui_state::UiState>,
}

impl std::ops::Deref for UiRuntime {
    type Target = LuaRuntime;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl std::ops::DerefMut for UiRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}

impl UiRuntime {
    pub(crate) fn with_ui<R>(
        &self,
        f: impl FnOnce(
            &BTreeMap<String, rustlua_ui::ui_state::UiText>,
            &BTreeMap<String, rustlua_ui::ui_state::UiBar>,
        ) -> R,
    ) -> R {
        self.ui.with(|ui| f(ui.texts(), ui.bars()))
    }

    pub(crate) fn with_dynamic_ui<R>(
        &self,
        f: impl FnOnce(
            &BTreeMap<String, rustlua_ui::ui::UiPanel>,
            &BTreeMap<String, rustlua_ui::ui::DynamicUiImage>,
            &BTreeMap<String, rustlua_ui::ui::UiGrid>,
        ) -> R,
    ) -> R {
        self.ui.with(|ui| f(ui.panels(), ui.images(), ui.grids()))
    }

    pub(crate) fn scene_text_hidden(&self) -> BTreeSet<String> {
        self.ui.with(|ui| ui.scene_text_hidden().clone())
    }

    pub(crate) fn set_ui_viewport(&mut self, width: u32, height: u32) {
        self.ui.with_mut(|ui| ui.set_viewport(width, height));
    }
}

pub(crate) fn runtime_with_ui() -> UiRuntime {
    let (runtime, _, _, _, ui) = runtime_with_domains();
    UiRuntime { runtime, ui }
}

/// 带宿主侧物理驱动的测试运行时。
///
/// 生产侧的驱动住在 `engine::host::runtime_scene::physics_driver`，本 crate 的测试不能反向
/// 依赖 `engine`，所以这里用同一份句柄手写一遍（`runtime_with_all` 完成接线）。`Deref` 到
/// `LuaRuntime` 让原有测试里的 VM 调用一字不改，物理方法则由本类型提供。
pub(crate) struct TestRuntime {
    runtime: LuaRuntime,
    pub(crate) physics: rustlua_lua_api::Handle<rustlua_physics::physics_state::PhysicsState>,
    pub(crate) ui: rustlua_lua_api::Handle<rustlua_ui::ui_state::UiState>,
}

impl std::ops::Deref for TestRuntime {
    type Target = LuaRuntime;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl std::ops::DerefMut for TestRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}

pub(crate) fn runtime_with_all() -> TestRuntime {
    let (runtime, _camera, _animation, physics, ui) = runtime_with_domains();
    TestRuntime { runtime, physics, ui }
}

impl TestRuntime {
    /// UI 视口登记（LuaRuntime 上的同名入口已随 UI 域外移删除，测试经句柄直写）。
    pub(crate) fn set_ui_viewport(&self, width: u32, height: u32) {
        self.ui.with_mut(|ui| ui.set_viewport(width, height));
    }

    pub(crate) fn step_physics(&self, delta_seconds: f32) {
        let entities = self.runtime.entity_state_handle();
        let pending = entities.with_mut(|e| e.take_physics_removals());
        self.physics.with_mut(|physics| {
            for entity in &pending {
                let _ = physics.world2d_mut().remove_body(*entity);
                let _ = physics.world3d_mut().remove_body(*entity);
            }
            let moved = physics.world2d_mut().step(delta_seconds);
            physics.set_pending_writeback_2d(moved);
        });
    }

    pub(crate) fn step_physics3d(&self, delta_seconds: f32) {
        self.physics.with_mut(|physics| {
            let moved = physics.world3d_mut().step(delta_seconds);
            physics.set_pending_writeback_3d(moved);
        });
    }

    pub(crate) fn writeback_physics(&mut self) -> usize {
        let moved = self
            .physics
            .with_mut(|physics| physics.take_pending_writeback_2d());
        let mut written = 0;
        for (entity, position) in moved {
            self.runtime.with_world_mut(|world| {
                if let Ok(Some(transform)) =
                    world.get_mut::<rustlua_ecs::transform::Transform>(entity)
                {
                    transform.position = position;
                    written += 1;
                }
            });
        }
        written
    }

    pub(crate) fn writeback_physics3d(&mut self) -> usize {
        let moved = self
            .physics
            .with_mut(|physics| physics.take_pending_writeback_3d());
        let mut written = 0;
        for (entity, position) in moved {
            self.runtime.with_world_mut(|world| {
                if let Ok(Some(transform)) =
                    world.get_mut::<rustlua_ecs::transform::Transform3D>(entity)
                {
                    transform.position = position;
                    written += 1;
                }
            });
        }
        written
    }

    pub(crate) fn discard_pending_writeback(&self) {
        self.physics
            .with_mut(|physics| physics.clear_pending_writebacks());
    }

    pub(crate) fn sync_kinematic_bodies(&self) -> usize {
        let physics = self.physics.clone();
        let mut synced = 0;
        self.runtime.with_world(|world| {
            let targets: Vec<_> = physics.with(|p| p.world2d().kinematic_positions(world));
            for (entity, position) in targets {
                if physics.with_mut(|p| p.world2d_mut().set_position(entity, position).is_ok()) {
                    synced += 1;
                }
            }
        });
        synced
    }

    pub(crate) fn sync_kinematic_bodies_3d(&self) -> usize {
        let physics = self.physics.clone();
        let mut synced = 0;
        self.runtime.with_world(|world| {
            let targets: Vec<_> = physics.with(|p| p.world3d().kinematic_positions(world));
            for (entity, position) in targets {
                if physics.with_mut(|p| p.world3d_mut().set_position(entity, position).is_ok()) {
                    synced += 1;
                }
            }
        });
        synced
    }

    pub(crate) fn take_collider_debug_request(&self) -> Option<bool> {
        self.physics.with_mut(|p| p.take_debug_request())
    }

    pub(crate) fn collider_debug_shapes(
        &self,
    ) -> Vec<rustlua_physics::physics::ColliderDebugShape> {
        self.physics.with(|p| p.debug_shapes())
    }

    pub(crate) fn collider_debug_shapes_3d(
        &self,
    ) -> Vec<rustlua_physics::physics3d::ColliderDebugShape3d> {
        self.physics.with(|p| p.debug_shapes_3d())
    }

    pub(crate) fn take_collider_debug_request_3d(&self) -> Option<bool> {
        self.physics.with_mut(|p| p.take_debug_request_3d())
    }

    pub(crate) fn dispatch_collision_contacts(&self) -> Result<usize, LuaBridgeError> {
        let contacts = self
            .physics
            .with_mut(|physics| physics.drain_event_contacts());
        let mut delivered = 0;
        for contact in contacts.into_iter().filter(|contact| contact.started) {
            if self.runtime.dispatch_entity_event(
                contact.a,
                EntityEvent::Collision { other: contact.b },
            )? {
                delivered += 1;
            }
            if contact.a != contact.b
                && self.runtime.dispatch_entity_event(
                    contact.b,
                    EntityEvent::Collision { other: contact.a },
                )?
            {
                delivered += 1;
            }
        }
        Ok(delivered)
    }
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
