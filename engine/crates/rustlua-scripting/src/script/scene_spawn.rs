//! `LuaRuntime` 的场景装配：把 `.scene` 里的实体声明变成 ECS 实体与组件。
//!
//! `instantiate_scene*` 负责清场 + 逐个 spawn + 父子关系，`spawn_entity_components` 是
//! 「组件名 → 组件类型」的唯一映射点，`process_ui_clicks` 把宿主结构化点击派发给 Lua 绑定。

// WP11-USE:BEGIN
use crate::script::LuaRuntime;
use crate::script::PersistedEntity;
use crate::script::ScriptState;
use crate::script::error::LuaBridgeError;
use crate::script::guard::{BudgetSlot, ScriptErrorPolicy};
use crate::script::solid_cell_colliders;
use mlua::Value;
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_animation::animation3d::Animation3D;
use rustlua_animation::animation3d::Animation3dDescriptor;
use rustlua_assets::resource_db::AssetId;
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::particle::ParticleEmitter;
use rustlua_ecs::particle::SceneParticleEmitter;
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::Transform;
use rustlua_ecs::transform::Transform3D;
use rustlua_input::input::InputSnapshot;
use rustlua_input::input::MouseButton;
use rustlua_math::math3d::Vec3;
use rustlua_physics::physics::ColliderDesc;
use rustlua_physics::physics3d::ColliderDesc3d;
use rustlua_render::atlas::resolve_atlas_reference;
use rustlua_render::light::Light;
use rustlua_render::mesh3d::MeshRenderer;
use rustlua_render::sprite::SpriteAsset;
use rustlua_render::sprite::SpriteComponent;
use rustlua_scene::scene::Scene;
use rustlua_ui::ui_events::UiClickTarget;
use rustlua_ui::ui_events::UiEvents;
use rustlua_ui::ui_layout::UiRectLayout;
use std::collections::BTreeMap;
use std::path::PathBuf;
// WP11-USE:END

impl LuaRuntime {
    /// Spawns every scene entity into the runtime world, parsing the editor's
    /// `Transform` and `Sprite` component strings and registering names so Lua
    /// can look entities up with `entity.find`. The host renderer reads the very
    /// same world through [`LuaRuntime::with_world`], so script-driven movement
    /// is immediately visible.
    pub fn instantiate_scene(&mut self, scene: &Scene) -> Result<usize, LuaBridgeError> {
        self.instantiate_scene_with_editor_visibility(scene, false)
    }

    pub fn instantiate_scene_with_editor_visibility(
        &mut self,
        scene: &Scene,
        honor_editor_hidden: bool,
    ) -> Result<usize, LuaBridgeError> {
        let mut state = self.state.borrow_mut();
        state.scene = Some(scene.name.clone());
        state.entity_state.scene_entities.clear();
        // 坏 parent 边（悬空/自环）同样在这里点名：运行时会把它们当成"没有父"，静默下去就成了
        // "子树莫名其妙少了一层位移"。
        state.scene_parent_issues = rustlua_scene::scene_space::parent_link_issues(scene)
            .into_iter()
            .map(|issue| match issue {
                rustlua_scene::scene_space::ParentLinkIssue::Dangling { entity, parent } => {
                    let name = scene
                        .entities
                        .get(&entity)
                        .filter(|node| !node.name.is_empty())
                        .map_or_else(|| format!("entity {}", entity.0), |node| node.name.clone());
                    format!("{name} → 不存在的父 {}", parent.0)
                }
                rustlua_scene::scene_space::ParentLinkIssue::SelfParent(entity) => {
                    let name = scene
                        .entities
                        .get(&entity)
                        .filter(|node| !node.name.is_empty())
                        .map_or_else(|| format!("entity {}", entity.0), |node| node.name.clone());
                    format!("{name} 的父指向自己")
                }
            })
            .collect();
        // docs/54 §4.3：一个实体只能有一种可绘制表达。冲突在这里点名（与"两份局部真源"同一套
        // 做法：数据问题要在载入时可见，而不是等到画面上出现两遍或只出现一遍）。
        state.scene_renderable_conflicts = scene
            .entities
            .values()
            .filter_map(|entity| {
                let renderable = rustlua_render::renderable::Renderable::classify(
                    entity.components.keys().map(String::as_str),
                );
                renderable.describe().map(|message| {
                    let name = if entity.name.is_empty() {
                        format!("entity {}", entity.id.0)
                    } else {
                        entity.name.clone()
                    };
                    format!("{name}: {message}")
                })
            })
            .collect();
        for issue in &state.scene_renderable_conflicts {
            tracing::warn!(
                target: "render",
                issue = %issue,
                "[render] 实体的可绘制表达不唯一（docs/54 §4.3）"
            );
        }
        for issue in &state.scene_parent_issues {
            tracing::warn!(
                target: "scene",
                issue = %issue,
                "[scene] 场景里的 parent 边坏了（docs/54 §8-②）"
            );
        }
        let mut count = 0;
        for entity in scene.entities.values() {
            if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
                continue;
            }
            let handle = Self::spawn_entity_components(
                &mut state,
                &entity.name,
                &entity.components,
                &entity.resources,
            )?;
            state.entity_state.scene_entities.insert(entity.id, handle);

            // 持久实体快照（权威、静态组件），供切场景后重生。
            if entity
                .metadata
                .get("persistent")
                .is_some_and(|value| value == "true")
                && !entity.name.is_empty()
            {
                state.entity_state.persistent.insert(
                    entity.name.clone(),
                    PersistedEntity {
                        name: entity.name.clone(),
                        components: entity.components.clone(),
                        resources: entity.resources.clone(),
                        metadata: entity.metadata.clone(),
                    },
                );
            }
            count += 1;
        }
        // WP2：把场景的 `parent` 落成运行时的 `Parent` 组件。**第二趟**做：子实体可能先于
        // 父实体出现，第一趟里父的运行时句柄还不存在。父被跳过（编辑器隐藏）时不建立关系——
        // 宁可不继承，也不要指到一个不存在或错误的实体上。
        for entity in scene.entities.values() {
            let Some(&child) = state.entity_state.scene_entities.get(&entity.id) else {
                continue;
            };
            let Some(parent) = entity
                .parent
                .and_then(|parent| state.entity_state.scene_entities.get(&parent).copied())
            else {
                continue;
            };
            if parent == child {
                continue;
            }
            state
                .world
                .insert(child, rustlua_ecs::hierarchy::Parent(parent))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        drop(state);
        // docs/54 §4.1：每实体脚本在这里落地——场景加载时就把声明了 `Script` 的脚本读进来并调用
        // `on_spawn(id)`。放在实例化末尾（而不是第一次 update）是为了让"脚本文件不存在/编译失败"
        // 在**加载期**就报出来；没有脚本实体的场景一次哈希都不用碰。
        self.sync_entity_scripts()?;
        Ok(count)
    }

    /// Dispatches click bindings from structured `UiEvents`. Hit targets are
    /// precomputed by `RuntimeScene::compute_click_targets`; each target may
    /// declare one or more Lua callbacks.
    ///
    /// 容错档（[`ScriptErrorPolicy::Contain`]）下某个绑定的回调报错只跳过**它自己**：
    /// 一个按钮的函数写错，不该连带后面排队的按钮一起失效。检测到已停用的分发槽
    /// 时整体跳过——那时所有点击回调都已被停用。
    pub fn process_ui_clicks(
        &self,
        input: &InputSnapshot,
        targets: &[UiClickTarget],
    ) -> Result<(), LuaBridgeError> {
        if !input.mouse_just_pressed(MouseButton::Left) {
            return Ok(());
        }
        if self.guard.borrow().is_disabled(BudgetSlot::UiClicks) {
            return Ok(());
        }
        let (mx, my) = input.cursor_position();
        let clicked: Vec<&UiClickTarget> = targets
            .iter()
            .filter(|target| {
                target.origin[0] <= mx as f32
                    && (mx as f32) < target.origin[0] + target.size[0]
                    && target.origin[1] <= my as f32
                    && (my as f32) < target.origin[1] + target.size[1]
            })
            .collect();
        for target in clicked {
            for function_name in &target.functions {
                if let Err(error) = self.call_ui_callback(function_name, target.name.as_str()) {
                    if self.guard.borrow().policy() == ScriptErrorPolicy::Strict {
                        return Err(error);
                    }
                    self.contain_script_error(BudgetSlot::UiClicks, &error);
                }
            }
        }
        Ok(())
    }

    /// 取全局回调并调用，把两次可能失败的动作（取函数、调用）包成一条错误链。
    fn call_ui_callback(
        &self,
        function_name: &str,
        target_name: &str,
    ) -> Result<(), LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<project script>"));
        let callback: Value =
            self.lua
                .globals()
                .get(function_name)
                .map_err(|error| LuaBridgeError::Runtime {
                    path: path.clone(),
                    reason: error.to_string(),
                })?;
        if let Value::Function(function) = callback {
            function
                .call::<()>((target_name,))
                .map_err(|error| LuaBridgeError::Runtime {
                    path,
                    reason: format!("ui click {function_name}: {error}"),
                })?;
        }
        Ok(())
    }

    /// Spawns a single entity from its component map into the runtime world
    /// (components, physics bodies, name registration). Single source of truth
    /// used by `instantiate_scene` and by the persistent-entity restore.
    pub(super) fn spawn_entity_components(
        state: &mut ScriptState,
        name: &str,
        components: &BTreeMap<String, String>,
        _resources: &BTreeMap<String, AssetId>,
    ) -> Result<EntityId, LuaBridgeError> {
        let id = state.world.spawn();
        // **唯一的 `Transform` 组件**：载荷只有一个形态（`position/rotation/scale`），
        // 2D 还是 3D 由实体携带的**其它组件**推断（`spatial_transform::entity_space`），
        // 推断结果决定插进运行时世界的哪一份表示：2D 快路径（平面 f32 弧度，10k 精灵档实测过）
        // 还是统一 3D 表示。2D 消费方（SpriteBatch / 2D 物理 / Lua `transform.get`）读前者，
        // 3D 消费方读后者——两边都拿得到同一个组件里的同一份数据。
        let raw_transform = components.get("Transform").cloned();
        if let Some(value) = raw_transform.as_deref() {
            let local =
                rustlua_ecs::spatial_transform::LocalTransform::from_unified_component_str(value)
                    .map_err(|reason| {
                    LuaBridgeError::Api(format!(
                        "entity {name:?} has an invalid Transform: {reason}"
                    ))
                })?;
            let space = rustlua_ecs::spatial_transform::entity_space(components);
            match space {
                rustlua_ecs::spatial_transform::SpatialKind::TwoD => {
                    let flat = local.to_2d();
                    state
                        .world
                        .insert(id, flat)
                        .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
                }
                rustlua_ecs::spatial_transform::SpatialKind::ThreeD => {
                    state
                        .world
                        .insert(
                            id,
                            Transform3D {
                                position: local.translation,
                                rotation: local.rotation,
                                scale: local.scale,
                            },
                        )
                        .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
                }
            }
        }
        if let Some(raw) = components.get("Sprite") {
            let component = SpriteComponent::from_component_str(raw).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Sprite: {error}"))
            })?;
            let sprite = component.sprite().map_err(|error| {
                LuaBridgeError::Api(format!(
                    "failed to build sprite for entity {name:?}: {error}"
                ))
            })?;
            // 图集引用语法 `图集路径#区域名`：装配前解析成纹理路径 + 区域 UV，
            // 运行时仍然只认识 SpriteAsset + Sprite.uv，抽取/合批路径零改动。
            // 路径与渲染器同源：按项目根解析，返回的纹理路径保持工程根相对写法。
            let (texture_path, sprite) =
                match resolve_atlas_reference(state.project_root.as_deref(), &component.texture) {
                    Ok(Some((texture_path, region))) => (
                        texture_path,
                        region.apply(sprite).map_err(|error| {
                            LuaBridgeError::Api(format!(
                                "failed to apply atlas region for entity {name:?}: {error}"
                            ))
                        })?,
                    ),
                    Ok(None) => (component.texture.clone(), sprite),
                    Err(error) => {
                        return Err(LuaBridgeError::Api(format!(
                            "entity {name:?} has an invalid atlas texture reference: {error}"
                        )));
                    }
                };
            state
                .world
                .insert(id, sprite)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, SpriteAsset(texture_path))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, component.sorting)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §4.3：一等 Quad（几何 + 材质，**贴图可选**）。它插入自己的组件而不是被降级成
        // `Sprite`，因为"这个实体是一个矩形"要在运行时可查、可改（`quad.get/set`），而且
        // Quad 不参与帧动画——那是 `Sprite` 的纹理区域才有的语义。
        if let Some(value) = components.get("Quad") {
            let quad = rustlua_render::quad::Quad::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Quad: {error}"))
            })?;
            state
                .world
                .insert(id, quad)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Tilemap") {
            let tilemap = Tilemap::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Tilemap: {error}"))
            })?;
            state
                .world
                .insert(id, tilemap)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §8-①：场景可以声明自带的相机。它作为**初始**相机生效；之后 Lua
        // `camera2d.*`/`camera3d.*` 与相机控制器照旧可以移动它（不是第二份姿态真源）。
        //
        // **唯一的 `Camera` 组件**：投影由判别字段给出，落进世界的就是**这一个**组件类型，
        // 消费方按投影分支取值（`as_orthographic` / `as_perspective`）——这正是"运行时也只
        // 有一个相机组件"的落地：世界存储、选帧遍历与 Lua 的 `camera.*` 都只看这一个类型。
        if let Some(value) = components.get("Camera") {
            let camera =
                rustlua_camera::camera::Camera::from_component_str(value).map_err(|error| {
                    LuaBridgeError::Api(format!("entity {name:?} has an invalid Camera: {error}"))
                })?;
            state
                .world
                .insert(id, camera)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §8-①：场景可以**声明**粒子发射器（连续发射由 `rate` 表达）。这里插入的是
        // 带累计器的运行时形态；发射本身由 `scene_emitters_step` 每步按速率推进。
        if let Some(value) = components.get("ParticleEmitter") {
            let emitter = ParticleEmitter::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid ParticleEmitter: {error}"
                ))
            })?;
            state
                .world
                .insert(id, SceneParticleEmitter::new(emitter))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Animation") {
            let animation = AnimationStateMachine::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Animation: {error}"))
            })?;
            state
                .world
                .insert(id, animation)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Animation3D") {
            let descriptor = Animation3dDescriptor::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Animation3D: {error}"
                ))
            })?;
            let root = state.project_root.clone().ok_or_else(|| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} needs a project root to load its Animation3D mesh"
                ))
            })?;
            let animation = Animation3D::from_gltf_path(&root, &descriptor).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} Animation3D failed to load: {error}"
                ))
            })?;
            state
                .world
                .insert(id, animation)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("MeshRenderer") {
            let renderer = MeshRenderer::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid MeshRenderer: {error}"
                ))
            })?;
            state
                .world
                .insert(id, renderer)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Light") {
            let light = Light::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Light: {error}"))
            })?;
            state
                .world
                .insert(id, light)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §4.2：可见层遮罩（`RenderView::visibility` 的实体侧）。缺组件 = 全可见，
        // 因此既有场景一字不改。
        if let Some(value) = components.get("Visibility") {
            let visibility = rustlua_render::visibility::Visibility::from_component_str(value)
                .map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "entity {name:?} has an invalid Visibility: {error}"
                    ))
                })?;
            state
                .world
                .insert(id, visibility)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §4.1：每实体脚本。这里只把组件写进世界；真正的加载与 `on_spawn(id)`
        // 在场景实例化的末尾统一做一次（见 `sync_entity_scripts`），因为脚本需要工程根。
        if let Some(value) = components.get(rustlua_ecs::entity_script::SCRIPT_COMPONENT) {
            let script = rustlua_ecs::entity_script::EntityScript::from_component_str(value)
                .map_err(|error| {
                    LuaBridgeError::Api(format!("entity {name:?} has an invalid Script: {error}"))
                })?;
            state
                .world
                .insert(id, script)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // docs/54 §4.3：显式 2D 显示模式（world/overlay/screen）。运行时不插入 = 跟随全局开关，
        // 因此既有场景一字不改；声明了的实体在提取时被路由到对应通道。
        if let Some(value) = components.get("RenderMode") {
            let mode = rustlua_render::render_mode::RenderMode::from_component_str(value).map_err(
                |error| {
                    LuaBridgeError::Api(format!(
                        "entity {name:?} has an invalid RenderMode: {error}"
                    ))
                },
            )?;
            state
                .world
                .insert(id, mode)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // UI scene components (screen-space overlays).
        if let Some(value) = components.get("Text") {
            let text = rustlua_ui::ui::UiText::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Text: {error}"))
            })?;
            state
                .world
                .insert(id, text)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Image") {
            let image = rustlua_ui::ui::UiImage::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Image: {error}"))
            })?;
            state
                .world
                .insert(id, image)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // WP4：世界空间文字（锚在世界位置，与屏幕 `Text` 是两套组件）。
        if let Some(value) = components.get("WorldText") {
            let text = rustlua_scene::world_text::WorldText::from_component_str(value).map_err(
                |error| {
                    LuaBridgeError::Api(format!(
                        "entity {name:?} has an invalid WorldText: {error}"
                    ))
                },
            )?;
            state
                .world
                .insert(id, text)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(raw) = components.get("UiEvents") {
            let events = UiEvents::parse(raw).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has invalid UiEvents: {error}"))
            })?;
            state
                .world
                .insert(id, events)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        // E16：可选 `UiLayout` 组件——四边 Anchor+Offset 布局，存在时优先于
        // Text/Image 的内联 x/y/width/height/anchor（编辑器与运行时同一 resolve）。
        if let Some(value) = components.get("UiLayout") {
            let layout = UiRectLayout::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid UiLayout: {error}"))
            })?;
            state
                .world
                .insert(id, layout)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if !name.is_empty() {
            state.entity_state.names.insert(name.to_owned(), id);
        }
        let position = state
            .world
            .get::<Transform>(id)
            .ok()
            .flatten()
            .map(|transform| transform.position)
            .unwrap_or_default();
        if let Some(value) = components.get("Collider") {
            let collider = ColliderDesc::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Collider: {error}"))
            })?;
            state
                .physics
                .world2d
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if components.contains_key("Tilemap") {
            let tilemap = state.world.get::<Tilemap>(id).ok().flatten();
            if let Some(tilemap) = tilemap
                && tilemap.solid.iter().any(|solid| *solid)
            {
                let colliders = solid_cell_colliders(tilemap);
                state
                    .physics
                    .world2d
                    .add_body(id, position, false, colliders)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
        }
        if let Some(value) = components.get("Collider3d") {
            let collider = ColliderDesc3d::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Collider3d: {error}"
                ))
            })?;
            let position = state
                .world
                .get::<Transform3D>(id)
                .ok()
                .flatten()
                .map(|transform| transform.position)
                .unwrap_or(Vec3::ZERO);
            state
                .physics
                .world3d
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        Ok(id)
    }
}

/// 一次 `prefab.spawn` 在活世界里的落点：根实体句柄 + 全部实例句柄 + 名字→句柄。
///
/// 刻意**不**进 `entity_state.scene_entities`：那张表是"当前场景声明的实体"，随场景加载
/// 清空并参与切图重生；prefab 实例是运行期动态生成的，混进去会让切图把 UI 实例当场景
/// 实体重生一遍。名字照旧进 `entity_state.names`（`entity.find` 同源），despawn 时按
/// 句柄精确摘除——同名实体被实例覆盖时不误删场景那份。
#[derive(Clone, Debug)]
pub struct PrefabSpawn {
    pub root: EntityId,
    pub handles: Vec<EntityId>,
    pub(crate) names: BTreeMap<String, EntityId>,
}

impl LuaRuntime {
    /// 把一份 `.prefab` 的实体树插进**活世界**（不清场、不动场景簿记）。
    ///
    /// 与 `instantiate_scene` 共用 `spawn_entity_components` 这唯一映射点，组件字符串的
    /// 解析口径不会分叉；parent 边同样第二趟落 `Parent` 组件（子可能先于父 spawn）。
    pub fn instantiate_prefab(
        &mut self,
        prefab: &rustlua_scene::prefab::Prefab,
    ) -> Result<PrefabSpawn, LuaBridgeError> {
        let mut state = self.state.borrow_mut();
        Self::instantiate_prefab_state(&mut state, prefab)
    }

    pub(crate) fn instantiate_prefab_state(
        state: &mut ScriptState,
        prefab: &rustlua_scene::prefab::Prefab,
    ) -> Result<PrefabSpawn, LuaBridgeError> {
        let mut map = BTreeMap::new();
        let mut handles = Vec::with_capacity(prefab.entities.len());
        let mut names = BTreeMap::new();
        for (source_id, entity) in &prefab.entities {
            let handle = Self::spawn_entity_components(
                state,
                &entity.name,
                &entity.components,
                &entity.resources,
            )?;
            map.insert(*source_id, handle);
            handles.push(handle);
            if !entity.name.is_empty() {
                names.insert(entity.name.clone(), handle);
            }
        }
        for (source_id, entity) in &prefab.entities {
            let Some(&child) = map.get(source_id) else {
                continue;
            };
            let Some(parent) = entity.parent.and_then(|parent| map.get(&parent).copied()) else {
                continue;
            };
            if parent == child {
                continue;
            }
            state
                .world
                .insert(child, rustlua_ecs::hierarchy::Parent(parent))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        let root = map[&prefab.root];
        // 布局节点登记：宿主画场景 UI 要先按"活实体槽位 → `UiSceneNode`"解析矩形，而这张表
        // 原本**只在场景加载时**从场景文档建一次。运行期 spawn 的实体不在里头，表现就是
        // "spawn 返回了句柄、实体真的活着、屏上一个像素都没多"。建节点的规则与场景那份
        // 同源（同一个 `build_ui_scene_nodes`），只有 key 的来源不同：这里直接用落进世界的
        // 槽位号，因此父链与宿主的按槽位查找对得上。
        let nodes = rustlua_ui::ui_scene_layout::build_ui_scene_nodes(
            prefab
                .entities
                .values()
                .map(|entity| rustlua_ui::ui_scene_layout::UiSceneNodeSource {
                    key: map[&entity.id].index() as u64,
                    parent: entity
                        .parent
                        .and_then(|parent| map.get(&parent))
                        .map(|handle| handle.index() as u64),
                    components: &entity.components,
                }),
        );
        state.prefab_ui_nodes.extend(nodes);
        Ok(PrefabSpawn {
            root,
            handles,
            names,
        })
    }

    /// 拆除一次 prefab 实例：名字按句柄精确摘除，布局节点按槽位摘除，实体逐个 despawn。
    pub(crate) fn despawn_prefab_state(
        state: &mut ScriptState,
        spawn: &PrefabSpawn,
    ) -> Result<(), LuaBridgeError> {
        for (name, handle) in &spawn.names {
            if state.entity_state.names.get(name) == Some(handle) {
                state.entity_state.names.remove(name);
            }
        }
        for handle in &spawn.handles {
            state.prefab_ui_nodes.remove(&(handle.index() as u64));
            state
                .world
                .despawn(*handle)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        Ok(())
    }
}
