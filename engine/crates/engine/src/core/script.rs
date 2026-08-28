use std::{
    cell::RefCell,
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::SystemTime,
};

use mlua::{Function, Lua, Result as LuaResult, Table, Value};
use tracing::{error, info, warn};

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::{Animation3D, Animation3dDescriptor},
    audio::AudioSystem,
    camera3d::Camera3D,
    entity::EntityId,
    input::{InputKey, InputSnapshot, MouseButton},
    light::Light,
    math3d::Vec3,
    mesh3d::MeshRenderer,
    particle::{Particle, ParticleSystem},
    physics::{ColliderDesc, ColliderShape, PhysicsWorld},
    physics3d::{ColliderDesc3d, PhysicsWorld3d},
    resource_db::AssetId,
    save::{decode as decode_save, encode as encode_save},
    scene::Scene,
    sprite::{SpriteAsset, SpriteComponent},
    tilemap::Tilemap,
    transform::{Transform, Vec2},
    transform3d::Transform3D,
    world::{World, WorldError},
};

/// `transform3d.set(id, x, y, z, [rx, ry, rz, rw, [sx, sy, sz]])` argument tuple.
type Transform3dSetArgs = (
    i64,
    f32,
    f32,
    f32,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
    Option<f32>,
);

#[derive(Debug, Default)]
struct ScriptState {
    delta_seconds: f32,
    quit_requested: bool,
    input: InputSnapshot,
    world: World,
    names: BTreeMap<String, EntityId>,
    health: BTreeMap<EntityId, f32>,
    scene: Option<String>,
    scene_pending_load: Option<String>,
    project_root: Option<PathBuf>,
    save_path: Option<PathBuf>,
    assets: BTreeMap<String, String>,
    ui: BTreeMap<String, UiText>,
    ui_bars: BTreeMap<String, UiBar>,
    save_data: BTreeMap<String, String>,
    physics: PhysicsWorld,
    physics3d: PhysicsWorld3d,
    particles: ParticleSystem,
    audio: AudioSystem,
    /// Entity whose `Camera3D` component the renderer uses this frame. Lua
    /// selects it with `camera.set_active`; the renderer falls back to the
    /// first camera entity when unset.
    active_camera: Option<EntityId>,
    /// Entity whose `Light` component the renderer uses this frame.
    active_light: Option<EntityId>,
    /// Persistent entities (name → authored snapshot) that survive scene loads.
    persistent: BTreeMap<String, PersistedEntity>,
}

#[derive(Debug, Clone)]
pub struct UiText {
    pub text: String,
    pub visible: bool,
    pub position: [f32; 2],
    /// 像素字号（SDF em 高度，px）。提供时优先于 `scale`；
    /// 换算见 [`crate::core::ui::ui_text_effective_scale`]。
    pub font_size: Option<f32>,
    /// 字形缩放系数（`1.0` = SDF 字体 em 12px / 位图 5x7 字形 7px）。
    pub scale: f32,
}

/// A screen-space filled bar (e.g. a health bar) rendered by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiBar {
    pub position: [f32; 2],
    pub size: [f32; 2],
    /// Fill ratio in `0..=1`.
    pub ratio: f32,
    pub color: [f32; 4],
}

/// Authored snapshot of an entity marked `metadata["persistent"]="true"`.
/// Keyed by entity name (the only handle stable across scene switches); kept in
/// `ScriptState::persistent` so `process_scene_transition` can re-spawn it.
#[derive(Clone, Debug, Default)]
struct PersistedEntity {
    name: String,
    components: BTreeMap<String, String>,
    resources: BTreeMap<String, AssetId>,
    /// 保留元数据（如 persistent 标志自身）；当前恢复只用 components。
    #[allow(dead_code)]
    metadata: BTreeMap<String, String>,
}

/// Errors produced while loading or running a project Lua script.
#[derive(Debug)]
pub enum LuaBridgeError {
    Init(String),
    Load { path: PathBuf, source: String },
    Runtime { path: PathBuf, source: String },
    Api(String),
}

impl fmt::Display for LuaBridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(source) => write!(f, "Lua VM initialization failed: {source}"),
            Self::Load { path, source } => write!(f, "failed to load {}: {source}", path.display()),
            Self::Runtime { path, source } => {
                write!(f, "script {} failed: {source}", path.display())
            }
            Self::Api(source) => write!(f, "Lua API error: {source}"),
        }
    }
}

impl std::error::Error for LuaBridgeError {}

/// Result of checking a loaded script for source changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadStatus {
    Unchanged,
    Reloaded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScriptFingerprint {
    modified: Option<SystemTime>,
    length: u64,
    hash: u64,
}

/// A structured diagnostic emitted by a Lua script load or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptDiagnostic {
    pub path: PathBuf,
    pub phase: String,
    pub message: String,
    pub line: Option<u32>,
    pub stack_trace: String,
    pub occurrences: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptDebugger {
    records: Vec<ScriptDiagnostic>,
    filter: Option<String>,
}

impl ScriptDebugger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, path: PathBuf, phase: impl Into<String>, source: impl Into<String>) {
        let phase = phase.into();
        let source = source.into();
        let line = find_line_number(&source);
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|item| item.path == path && item.phase == phase && item.message == source)
        {
            existing.occurrences = existing.occurrences.saturating_add(1);
            return;
        }
        self.records.push(ScriptDiagnostic {
            path,
            phase,
            message: source.clone(),
            line,
            stack_trace: source,
            occurrences: 1,
        });
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn set_filter(&mut self, filter: Option<String>) {
        self.filter = filter;
    }

    #[must_use]
    pub fn all(&self) -> &[ScriptDiagnostic] {
        &self.records
    }

    #[must_use]
    pub fn visible(&self) -> Vec<ScriptDiagnostic> {
        self.records
            .iter()
            .filter(|item| {
                self.filter.as_ref().is_none_or(|filter| {
                    item.message.contains(filter) || item.path.to_string_lossy().contains(filter)
                })
            })
            .cloned()
            .collect()
    }
}

/// Main-thread Lua runtime for project scripts.
pub struct LuaRuntime {
    lua: Lua,
    state: Rc<RefCell<ScriptState>>,
    current_path: Option<PathBuf>,
    fingerprint: Option<ScriptFingerprint>,
    debugger: ScriptDebugger,
}
impl LuaRuntime {
    pub fn new() -> Result<Self, LuaBridgeError> {
        let lua = Lua::new();
        let state = Rc::new(RefCell::new(ScriptState::default()));
        register_api(&lua, Rc::clone(&state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        Ok(Self {
            lua,
            state,
            current_path: None,
            fingerprint: None,
            debugger: ScriptDebugger::new(),
        })
    }

    pub fn set_delta(&mut self, delta_seconds: f32) {
        self.state.borrow_mut().delta_seconds = delta_seconds.max(0.0);
    }

    pub fn set_input(&mut self, input: InputSnapshot) {
        self.state.borrow_mut().input = input;
    }

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
        let mut count = 0;
        for entity in scene.entities.values() {
            if honor_editor_hidden
                && scene
                    .metadata
                    .get(&format!("editor.hidden.{}", entity.id.0))
                    .is_some_and(|value| value == "true")
            {
                continue;
            }
            Self::spawn_entity_components(
                &mut state,
                &entity.name,
                &entity.components,
                &entity.resources,
            )?;
            // 持久实体快照（权威、静态组件），供切场景后重生。
            if entity
                .metadata
                .get("persistent")
                .is_some_and(|value| value == "true")
                && !entity.name.is_empty()
            {
                state.persistent.insert(
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
        Ok(count)
    }

    /// 按下边沿的 Button 命中测试 → 调用全局 `on_click(name)`（若脚本定义了）。
    pub fn process_ui_clicks(
        &self,
        input: &InputSnapshot,
        viewport_width: u32,
        viewport_height: u32,
    ) -> Result<(), LuaBridgeError> {
        if !input.mouse_just_pressed(MouseButton::Left) {
            return Ok(());
        }
        let (mx, my) = input.cursor_position();
        let viewport = [viewport_width.max(1) as f32, viewport_height.max(1) as f32];
        let clicked: Vec<String> = {
            let state = self.state.borrow();
            state
                .world
                .entities_with::<crate::core::ui::UiButton>()
                .into_iter()
                .filter_map(|id| {
                    let Ok(Some(button)) = state.world.get::<crate::core::ui::UiButton>(id) else {
                        return None;
                    };
                    let (origin, size) = crate::core::ui::resolve_ui_rect(
                        button.position,
                        button.size,
                        button.space,
                        button.anchor,
                        viewport,
                    );
                    let inside = (mx as f32) >= origin[0]
                        && (mx as f32) <= origin[0] + size[0]
                        && (my as f32) >= origin[1]
                        && (my as f32) <= origin[1] + size[1];
                    if !inside {
                        return None;
                    }
                    state
                        .names
                        .iter()
                        .find(|(_, entity)| **entity == id)
                        .map(|(name, _)| name.clone())
                })
                .collect()
        };
        for name in clicked {
            let callback: Value =
                self.lua
                    .globals()
                    .get("on_click")
                    .map_err(|error| LuaBridgeError::Runtime {
                        path: self
                            .current_path
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("<project script>")),
                        source: error.to_string(),
                    })?;
            if let Value::Function(function) = callback {
                function
                    .call::<()>((name,))
                    .map_err(|error| LuaBridgeError::Runtime {
                        path: self
                            .current_path
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("<project script>")),
                        source: error.to_string(),
                    })?;
            }
        }
        Ok(())
    }

    /// Spawns a single entity from its component map into the runtime world
    /// (components, physics bodies, name registration). Single source of truth
    /// used by `instantiate_scene` and by the persistent-entity restore.
    fn spawn_entity_components(
        state: &mut ScriptState,
        name: &str,
        components: &BTreeMap<String, String>,
        _resources: &BTreeMap<String, AssetId>,
    ) -> Result<EntityId, LuaBridgeError> {
        let id = state.world.spawn();
        if let Some(value) = components.get("Transform") {
            let transform = Transform::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Transform: {error}"))
            })?;
            state
                .world
                .insert(id, transform)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
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
            state
                .world
                .insert(id, sprite)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, SpriteAsset(component.texture))
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            state
                .world
                .insert(id, component.sorting)
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
        if let Some(value) = components.get("Animation") {
            let animation = AnimationStateMachine::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Animation: {error}"))
            })?;
            state
                .world
                .insert(id, animation)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Transform3D") {
            let transform = Transform3D::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!(
                    "entity {name:?} has an invalid Transform3D: {error}"
                ))
            })?;
            state
                .world
                .insert(id, transform)
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
        if let Some(value) = components.get("Camera3D") {
            let camera = Camera3D::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Camera3D: {error}"))
            })?;
            state
                .world
                .insert(id, camera)
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
        // UI scene components (screen-space overlays).
        if let Some(value) = components.get("Text") {
            let text = crate::core::ui::UiText::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Text: {error}"))
            })?;
            state
                .world
                .insert(id, text)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Image") {
            let image = crate::core::ui::UiImage::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Image: {error}"))
            })?;
            state
                .world
                .insert(id, image)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if let Some(value) = components.get("Button") {
            let button = crate::core::ui::UiButton::from_component_str(value).map_err(|error| {
                LuaBridgeError::Api(format!("entity {name:?} has an invalid Button: {error}"))
            })?;
            state
                .world
                .insert(id, button)
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        if !name.is_empty() {
            state.names.insert(name.to_owned(), id);
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
                .add_body(id, position, true, vec![collider])
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
                .physics3d
                .add_body(id, position, collider.dynamic, vec![collider])
                .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        }
        Ok(id)
    }

    /// Steps the physics world and writes moved dynamic body positions back to
    /// their ECS Transforms so rendering and Lua see the simulated result.
    pub fn step_physics(&mut self, delta_seconds: f32) {
        let mut state = self.state.borrow_mut();
        let moved = state.physics.step(delta_seconds);
        for (entity, position) in moved {
            if let Ok(Some(transform)) = state.world.get_mut::<Transform>(entity) {
                transform.position = position;
            }
        }
    }

    /// Steps the 3D physics world and writes moved dynamic body positions back
    /// to their ECS `Transform3D` so rendering and Lua see the simulation.
    pub fn step_physics3d(&mut self, delta_seconds: f32) {
        let mut state = self.state.borrow_mut();
        let moved = state.physics3d.step(delta_seconds);
        for (entity, position) in moved {
            if let Ok(Some(transform)) = state.world.get_mut::<Transform3D>(entity) {
                transform.position = position;
            }
        }
    }

    /// Advances the particle system by `dt` seconds.
    pub fn step_particles(&mut self, delta_seconds: f32) {
        self.state.borrow_mut().particles.step(delta_seconds);
    }

    /// Sets the directory audio clips are loaded from.
    pub fn set_audio_asset_dir(&mut self, dir: PathBuf) {
        self.state.borrow_mut().audio.set_asset_dir(dir);
    }

    /// Sets the project root, used to resolve `scenes/<name>.scene` on load.
    pub fn set_project_root(&mut self, root: PathBuf) {
        self.state.borrow_mut().project_root = Some(root);
    }

    /// Sets the file path save data is committed to / loaded from.
    pub fn set_save_path(&mut self, path: PathBuf) {
        self.state.borrow_mut().save_path = Some(path);
    }

    /// Applies a pending `scene.load(name)` requested by Lua: clears the ECS
    /// world, physics bodies, particles and UI, instantiates the new scene,
    /// and re-runs the project script's `on_create` so the new level is
    /// initialized with fresh entity handles. Saves and audio persist.
    pub fn process_scene_transition(&mut self) -> Result<(), LuaBridgeError> {
        let pending = self.state.borrow().scene_pending_load.clone();
        let Some(name) = pending else {
            return Ok(());
        };
        let root = self
            .state
            .borrow()
            .project_root
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project root configured".to_owned()))?;
        let path = root.join("scenes").join(format!("{name}.scene"));
        let scene = Scene::load(&path).map_err(|error| {
            LuaBridgeError::Api(format!("scene {name} failed to load: {error}"))
        })?;
        {
            let mut state = self.state.borrow_mut();
            state.world = World::new();
            state.physics = PhysicsWorld::new();
            state.physics3d = PhysicsWorld3d::new();
            state.particles = ParticleSystem::new();
            state.names.clear();
            state.health.clear();
            state.ui.clear();
            state.ui_bars.clear();
            state.scene_pending_load = None;
            state.scene = Some(name.clone());
            state.active_camera = None;
            state.active_light = None;
        }
        let instantiated = self.instantiate_scene(&scene)?;
        // 恢复持久实体：新场景未定义的名字用持久快照重生（新场景定义胜出）。
        {
            let mut state = self.state.borrow_mut();
            let persisted: Vec<PersistedEntity> = state.persistent.values().cloned().collect();
            for entity in persisted {
                if state.names.contains_key(&entity.name) {
                    continue;
                }
                Self::spawn_entity_components(
                    &mut state,
                    &entity.name,
                    &entity.components,
                    &entity.resources,
                )
                .map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "restore persistent entity {}: {error}",
                        entity.name
                    ))
                })?;
            }
        }
        call_optional(&self.lua, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: self
                .current_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("<project script>")),
            source: error.to_string(),
        })?;
        tracing::info!(target: "scene", name = %name, entities = instantiated, "[scene] switched");
        Ok(())
    }

    /// Gives the host a read view of the UI text and bar elements.
    pub fn with_ui<R>(
        &self,
        f: impl FnOnce(&BTreeMap<String, UiText>, &BTreeMap<String, UiBar>) -> R,
    ) -> R {
        let state = self.state.borrow();
        f(&state.ui, &state.ui_bars)
    }

    /// Gives the host a read view of the active particles.
    pub fn with_particles<R>(&self, f: impl FnOnce(&[Particle]) -> R) -> R {
        f(self.state.borrow().particles.particles())
    }

    /// Gives the host a shared read view of the runtime entity world, so the
    /// renderer draws the exact entities the Lua script is mutating.
    pub fn with_world<R>(&self, f: impl FnOnce(&World) -> R) -> R {
        f(&self.state.borrow().world)
    }

    /// Gives the host a shared write view of the runtime entity world.
    pub fn with_world_mut<R>(&mut self, f: impl FnOnce(&mut World) -> R) -> R {
        f(&mut self.state.borrow_mut().world)
    }

    /// Returns the authored name registered for an entity, if any. Entities
    /// spawned from a serialized scene keep their scene name; entities spawned
    /// by Lua via `entity.spawn` are registered with the name given at spawn.
    /// Entities never named (or dropped from `names` after despawn) return `None`.
    #[must_use]
    pub fn entity_name(&self, id: EntityId) -> Option<String> {
        self.state
            .borrow()
            .names
            .iter()
            .find(|(_, registered)| **registered == id)
            .map(|(name, _)| name.clone())
    }

    /// Returns the entity whose `Camera3D` the renderer should use, if Lua
    /// selected one with `camera.set_active`.
    #[must_use]
    pub fn active_camera(&self) -> Option<EntityId> {
        self.state.borrow().active_camera
    }

    /// Returns the entity whose `Light` the renderer should use, if Lua
    /// selected one with `light.set_active`.
    #[must_use]
    pub fn active_light(&self) -> Option<EntityId> {
        self.state.borrow().active_light
    }

    pub fn execute_project_script(&mut self, path: &Path) -> Result<(), LuaBridgeError> {
        let source = read_project_script(path)?;
        let candidate = match self.load_candidate(path, &source) {
            Ok(candidate) => candidate,
            Err(error) => {
                self.record_bridge_error(&error);
                return Err(error);
            }
        };
        if let Err(error) = call_optional(&candidate, "on_create") {
            let error = LuaBridgeError::Runtime {
                path: path.to_path_buf(),
                source: error.to_string(),
            };
            self.record_bridge_error(&error);
            return Err(error);
        }
        self.lua = candidate;
        self.current_path = Some(path.to_path_buf());
        self.fingerprint = Some(file_fingerprint(path, &source)?);
        Ok(())
    }

    pub fn reload_if_changed(&mut self) -> Result<ReloadStatus, LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let fingerprint = file_fingerprint(&path, &source)?;
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            return Ok(ReloadStatus::Unchanged);
        }
        let candidate = self.load_candidate(&path, &source)?;
        call_optional(&self.lua, "on_destroy").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        call_optional(&candidate, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        self.lua = candidate;
        self.fingerprint = Some(fingerprint);
        Ok(ReloadStatus::Reloaded)
    }

    pub fn reload(&mut self) -> Result<(), LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .ok_or_else(|| LuaBridgeError::Api("no project script is loaded".to_string()))?;
        let source = read_project_script(&path)?;
        let candidate = self.load_candidate(&path, &source)?;
        call_optional(&self.lua, "on_destroy").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        call_optional(&candidate, "on_create").map_err(|error| LuaBridgeError::Runtime {
            path: path.clone(),
            source: error.to_string(),
        })?;
        self.lua = candidate;
        self.fingerprint = Some(file_fingerprint(&path, &source)?);
        Ok(())
    }

    fn load_candidate(&self, path: &Path, source: &str) -> Result<Lua, LuaBridgeError> {
        let candidate = Lua::new();
        register_api(&candidate, Rc::clone(&self.state))
            .map_err(|error| LuaBridgeError::Init(error.to_string()))?;
        // Let Lua `require` resolve sibling modules relative to the project
        // script directory (e.g. `require("player")` -> `scripts/player.lua`).
        if let Ok(package) = candidate.globals().get::<Table>("package")
            && let Ok(existing) = package.get::<String>("path")
            && let Some(script_dir) = path.parent()
        {
            let module_path = format!("{};{}", script_dir.join("?.lua").display(), existing);
            let _ = package.set("path", module_path);
        }
        // 拦截 require：支持压缩/加密的模块文件（明文模块同样可用）。
        if let Some(script_dir) = path.parent() {
            let _ = install_encoded_require_searcher(&candidate, script_dir.to_path_buf());
        }
        candidate
            .load(source)
            .set_name("scripts/main.lua")
            .exec()
            .map_err(|error| LuaBridgeError::Load {
                path: path.to_path_buf(),
                source: error.to_string(),
            })?;
        Ok(candidate)
    }
    pub fn call_update(&self) -> Result<String, LuaBridgeError> {
        let path = self
            .current_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<project script>"));
        let update: Option<Function> = self
            .lua
            .globals()
            .get("update")
            .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
        // No script loaded (engine-only diagnostic mode) or the script has no
        // `update` function: the game simply idles instead of erroring out.
        let Some(update) = update else {
            return Ok(String::new());
        };
        update.call(()).map_err(|error| LuaBridgeError::Runtime {
            path,
            source: error.to_string(),
        })
    }

    pub fn call_update_debug(&mut self) -> Result<String, LuaBridgeError> {
        match self.call_update() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.record_bridge_error(&error);
                Err(error)
            }
        }
    }

    /// Executes a short expression or statement in the currently loaded Lua VM.
    /// Console failures are routed through the same structured debugger as script errors.
    pub fn execute_console(&mut self, source: &str) -> Result<String, LuaBridgeError> {
        let path = PathBuf::from("<console>");
        let result = self
            .lua
            .load(source)
            .set_name("<console>")
            .eval::<Value>()
            .map(format_lua_value)
            .map_err(|error| LuaBridgeError::Runtime {
                path: path.clone(),
                source: error.to_string(),
            });
        if let Err(error) = &result {
            self.record_bridge_error(error);
        }
        result
    }

    fn record_bridge_error(&mut self, error: &LuaBridgeError) {
        match error {
            LuaBridgeError::Load { path, source } => {
                self.debugger.record(path.clone(), "load", source.clone())
            }
            LuaBridgeError::Runtime { path, source } => {
                self.debugger
                    .record(path.clone(), "runtime", source.clone())
            }
            LuaBridgeError::Api(source) | LuaBridgeError::Init(source) => {
                self.debugger
                    .record(PathBuf::from("<engine>"), "api", source.clone())
            }
        }
    }

    #[must_use]
    pub fn diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.debugger.visible()
    }

    #[must_use]
    pub fn all_diagnostics(&self) -> &[ScriptDiagnostic] {
        self.debugger.all()
    }

    pub fn clear_diagnostics(&mut self) {
        self.debugger.clear();
    }

    pub fn set_debug_filter(&mut self, filter: Option<String>) {
        self.debugger.set_filter(filter);
    }

    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.state.borrow().quit_requested
    }
}

fn find_line_number(source: &str) -> Option<u32> {
    let bytes = source.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b':' {
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start
            && (end == bytes.len() || bytes[end] == 58 || bytes[end] == 32)
            && let Ok(line) = source[start..end].parse()
        {
            return Some(line);
        }
    }
    None
}

fn file_fingerprint(path: &Path, source: &str) -> Result<ScriptFingerprint, LuaBridgeError> {
    use std::hash::{Hash, Hasher};
    let metadata = fs::metadata(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error.to_string(),
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    Ok(ScriptFingerprint {
        modified: metadata.modified().ok(),
        length: metadata.len(),
        hash: hasher.finish(),
    })
}

fn format_lua_value(value: Value) -> String {
    match value {
        Value::Nil => "nil".to_owned(),
        Value::Boolean(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.to_string_lossy(),
        Value::Table(_) => "<table>".to_owned(),
        Value::Function(_) => "<function>".to_owned(),
        Value::Thread(_) => "<thread>".to_owned(),
        Value::UserData(_) => "<userdata>".to_owned(),
        Value::LightUserData(_) => "<lightuserdata>".to_owned(),
        Value::Error(error) => error.to_string(),
        Value::Other(_) => "<other>".to_owned(),
    }
}

fn call_optional(lua: &Lua, name: &str) -> LuaResult<()> {
    let callback: Value = lua.globals().get(name)?;
    if let Value::Function(function) = callback {
        function.call::<()>(())?;
    }
    Ok(())
}
fn api_error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}
fn entity_from_lua(value: i64) -> Result<EntityId, mlua::Error> {
    if value < 0 {
        return Err(api_error("invalid entity id: negative value"));
    }
    let raw = value as u64;
    let index = (raw >> 32) as u32;
    let generation = raw as u32;
    if generation == 0 {
        return Err(api_error(format!("invalid entity id: {value}")));
    }
    Ok(EntityId::from_raw(index, generation))
}
fn entity_to_lua(entity: EntityId) -> i64 {
    ((entity.index() as u64) << 32 | entity.generation() as u64) as i64
}
fn require_alive(state: &ScriptState, entity: EntityId) -> Result<(), mlua::Error> {
    if state.world.is_alive(entity) {
        Ok(())
    } else {
        Err(api_error(format!("invalid entity id: {entity}")))
    }
}
fn table_vec(lua: &Lua, x: f32, y: f32) -> LuaResult<Table> {
    let table = lua.create_table()?;
    table.set(1, x)?;
    table.set(2, y)?;
    Ok(table)
}

/// 读取项目脚本源码，支持发布包的压缩/加密（明文原样返回）。
fn read_project_script(path: &Path) -> Result<String, LuaBridgeError> {
    let bytes = fs::read(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error.to_string(),
    })?;
    let decoded = crate::core::pack::decode(&bytes).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        source: error,
    })?;
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

/// 在 Lua VM 上安装支持压缩/加密的 require 搜索器：
/// 读取模块文件字节 → 解码 → 编译，供 `require` 加载发布包中的模块。
fn install_encoded_require_searcher(lua: &Lua, script_dir: PathBuf) -> mlua::Result<()> {
    let searcher = lua.create_function(move |lua, module: String| {
        let relative = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        let file_path = script_dir.join(format!("{relative}.lua"));
        if !file_path.is_file() {
            return Ok(Value::Nil);
        }
        let bytes = fs::read(&file_path).map_err(mlua::Error::external)?;
        let decoded = crate::core::pack::decode(&bytes).map_err(mlua::Error::external)?;
        let source = String::from_utf8_lossy(&decoded).into_owned();
        let chunk_name = format!("scripts/{module}.lua");
        let chunk = lua.load(&source).set_name(chunk_name).into_function()?;
        Ok(Value::Function(chunk))
    })?;
    let package: Table = lua.globals().get("package")?;
    let searchers: Table = package.get("searchers")?;
    searchers.raw_insert(1, searcher)?;
    Ok(())
}

/// Writes `text` to `path` atomically (temp file + rename), like scene saves.
fn atomic_write(path: &Path, text: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

/// Converts a tile map's solid cells into static cuboid colliders attached to
/// the tile map entity's fixed body, one per solid cell.
fn solid_cell_colliders(tilemap: &Tilemap) -> Vec<ColliderDesc> {
    let half = Vec2::new(tilemap.tile_size.x / 2.0, tilemap.tile_size.y / 2.0);
    let mut colliders = Vec::new();
    for y in 0..tilemap.height {
        for x in 0..tilemap.width {
            if tilemap.solid[y * tilemap.width + x] {
                colliders.push(ColliderDesc {
                    shape: ColliderShape::Cuboid(half),
                    offset: Vec2::new(
                        x as f32 * tilemap.tile_size.x + half.x,
                        y as f32 * tilemap.tile_size.y + half.y,
                    ),
                    sensor: false,
                    layer: 0xFF,
                    mask: 0xFF,
                });
            }
        }
    }
    colliders
}

fn register_api(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let log = lua.create_table()?;
    log.set(
        "info",
        lua.create_function(|_, message: String| {
            info!(target: "script", message = %message, "Lua log.info");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_, message: String| {
            warn!(target: "script", message = %message, "Lua log.warn");
            Ok(())
        })?,
    )?;
    log.set(
        "error",
        lua.create_function(|_, message: String| {
            error!(target: "script", message = %message, "Lua log.error");
            Ok(())
        })?,
    )?;
    lua.globals().set("log", log)?;

    let time = lua.create_table()?;
    let s = Rc::clone(&state);
    time.set(
        "delta",
        lua.create_function(move |_, ()| Ok(s.borrow().delta_seconds))?,
    )?;
    lua.globals().set("time", time)?;

    let input = lua.create_table()?;
    for (name, mode) in [("is_down", 0_u8), ("just_pressed", 1), ("just_released", 2)] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, key: String| {
                let input = &s.borrow().input;
                let key = parse_key(&key);
                Ok(match mode {
                    0 => input.is_down(key),
                    1 => input.just_pressed(key),
                    _ => input.just_released(key),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "action",
        lua.create_function(move |_, key: String| Ok(s.borrow().input.is_down(parse_key(&key))))?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "pressed",
        lua.create_function(move |_, key: String| {
            Ok(s.borrow().input.just_pressed(parse_key(&key)))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "released",
        lua.create_function(move |_, key: String| {
            Ok(s.borrow().input.just_released(parse_key(&key)))
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "axis",
        lua.create_function(move |_, axis: String| Ok(s.borrow().input.axis(&axis)))?,
    )?;
    for (name, mode) in [
        ("mouse_is_down", 0_u8),
        ("mouse_just_pressed", 1),
        ("mouse_just_released", 2),
    ] {
        let s = Rc::clone(&state);
        input.set(
            name,
            lua.create_function(move |_, button: String| {
                let input = &s.borrow().input;
                let button = parse_mouse_button(&button);
                Ok(match mode {
                    0 => input.mouse_is_down(button),
                    1 => input.mouse_just_pressed(button),
                    _ => input.mouse_just_released(button),
                })
            })?,
        )?;
    }
    let s = Rc::clone(&state);
    input.set(
        "cursor_position",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.cursor_position();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "cursor_delta",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.cursor_delta();
            table_vec(lua, p.0 as f32, p.1 as f32)
        })?,
    )?;
    let s = Rc::clone(&state);
    input.set(
        "scroll_delta",
        lua.create_function(move |lua, ()| {
            let p = s.borrow().input.scroll_delta();
            table_vec(lua, p.0, p.1)
        })?,
    )?;
    lua.globals().set("input", input)?;

    let entity = lua.create_table()?;
    let s = Rc::clone(&state);
    entity.set(
        "spawn",
        lua.create_function(move |_, name: Option<String>| {
            let mut state = s.borrow_mut();
            let id = state.world.spawn();
            state
                .world
                .insert(id, Transform::default())
                .map_err(world_error)?;
            if let Some(name) = name {
                state.names.insert(name, id);
            }
            Ok(entity_to_lua(id))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "find",
        lua.create_function(move |_, name: String| {
            Ok(s.borrow().names.get(&name).copied().map(entity_to_lua))
        })?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "is_alive",
        lua.create_function(move |_, id: i64| Ok(s.borrow().world.is_alive(entity_from_lua(id)?)))?,
    )?;
    let s = Rc::clone(&state);
    entity.set(
        "destroy",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.despawn(id).map_err(world_error)?;
            state.names.retain(|_, value| *value != id);
            state.health.remove(&id);
            let _ = state.physics.remove_body(id);
            let _ = state.physics3d.remove_body(id);
            Ok(true)
        })?,
    )?;
    lua.globals().set("entity", entity)?;
    let sprite = lua.create_table()?;
    let s = Rc::clone(&state);
    sprite.set(
        "set",
        lua.create_function(move |_, (id, component): (i64, String)| {
            let id = entity_from_lua(id)?;
            let component = SpriteComponent::from_component_str(&component)
                .map_err(|error| api_error(error.to_string()))?;
            let sprite_value = component
                .sprite()
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.insert(id, sprite_value).map_err(world_error)?;
            state.world.insert(id, SpriteAsset(component.texture)).map_err(world_error)?;
            Ok(())
        })?,
    )?;
    lua.globals().set("sprite", sprite)?;

    let transform = lua.create_table()?;
    let s = Rc::clone(&state);
    transform.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<Transform>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform"))?;
            let t = lua.create_table()?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("rotation", value.rotation)?;
            t.set("scale_x", value.scale.x)?;
            t.set("scale_y", value.scale.y)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, rotation, scale_x, scale_y): (
                i64,
                f32,
                f32,
                Option<f32>,
                Option<f32>,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                let t = state
                    .world
                    .get_mut::<Transform>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform"))?;
                t.position = Vec2::new(x, y);
                if let Some(v) = rotation {
                    t.rotation = v;
                }
                if let Some(v) = scale_x {
                    t.scale.x = v;
                }
                if let Some(v) = scale_y {
                    t.scale.y = v;
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform.set(
        "move",
        lua.create_function(move |_, (id, dx, dy): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let t = state
                .world
                .get_mut::<Transform>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform"))?;
            t.position.x += dx;
            t.position.y += dy;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform", transform)?;

    let transform3d = lua.create_table()?;
    let s = Rc::clone(&state);
    transform3d.set(
        "attach",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .world
                .insert(id, Transform3D::default())
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let value = state
                .world
                .get::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            let t = lua.create_table()?;
            t.set("x", value.position.x)?;
            t.set("y", value.position.y)?;
            t.set("z", value.position.z)?;
            t.set("rx", value.rotation.x)?;
            t.set("ry", value.rotation.y)?;
            t.set("rz", value.rotation.z)?;
            t.set("rw", value.rotation.w)?;
            t.set("sx", value.scale.x)?;
            t.set("sy", value.scale.y)?;
            t.set("sz", value.scale.z)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "set",
        lua.create_function(
            move |_, (id, x, y, z, rx, ry, rz, rw, sx, sy, sz): Transform3dSetArgs| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return Err(api_error("transform3d.set position must be finite"));
                }
                let t = state
                    .world
                    .get_mut::<Transform3D>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no transform3d"))?;
                t.position = Vec3::new(x, y, z);
                if let (Some(rx), Some(ry), Some(rz), Some(rw)) = (rx, ry, rz, rw) {
                    if !rx.is_finite() || !ry.is_finite() || !rz.is_finite() || !rw.is_finite() {
                        return Err(api_error("transform3d.set rotation must be finite"));
                    }
                    t.rotation = crate::core::math3d::Quat {
                        x: rx,
                        y: ry,
                        z: rz,
                        w: rw,
                    };
                }
                if let (Some(sx), Some(sy), Some(sz)) = (sx, sy, sz) {
                    if sx <= 0.0 || sy <= 0.0 || sz <= 0.0 {
                        return Err(api_error("transform3d.set scale must be positive"));
                    }
                    t.scale = Vec3::new(sx, sy, sz);
                }
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    transform3d.set(
        "move",
        lua.create_function(move |_, (id, dx, dy, dz): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let t = state
                .world
                .get_mut::<Transform3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no transform3d"))?;
            t.position.x += dx;
            t.position.y += dy;
            t.position.z += dz;
            Ok(())
        })?,
    )?;
    lua.globals().set("transform3d", transform3d)?;

    let mesh = lua.create_table()?;
    let s = Rc::clone(&state);
    mesh.set(
        "set",
        lua.create_function(move |_, (id, path): (i64, String)| {
            let id = entity_from_lua(id)?;
            let renderer = MeshRenderer::from_component_str(&format!("mesh={path}"))
                .map_err(|error| api_error(error.to_string()))?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state.world.insert(id, renderer).map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    mesh.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(renderer) = state.world.get::<MeshRenderer>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(lua.create_string(&renderer.mesh)?))
        })?,
    )?;
    let s = Rc::clone(&state);
    mesh.set(
        "clear",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .world
                .remove::<MeshRenderer>(id)
                .map_err(world_error)?;
            Ok(true)
        })?,
    )?;
    lua.globals().set("mesh", mesh)?;

    let camera = lua.create_table()?;
    let s = Rc::clone(&state);
    camera.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if state
                .world
                .get::<Camera3D>(id)
                .map_err(world_error)?
                .is_none()
            {
                return Err(api_error("entity has no camera3d"));
            }
            state.active_camera = Some(id);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get_active",
        lua.create_function(move |_, ()| Ok(s.borrow().active_camera.map(entity_to_lua)))?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(camera) = state.world.get::<Camera3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no camera3d"));
            };
            let t = lua.create_table()?;
            t.set("x", camera.position.x)?;
            t.set("y", camera.position.y)?;
            t.set("z", camera.position.z)?;
            t.set("tx", camera.target.x)?;
            t.set("ty", camera.target.y)?;
            t.set("tz", camera.target.z)?;
            t.set("fov", camera.fov_y.to_degrees())?;
            t.set("near", camera.near)?;
            t.set("far", camera.far)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    camera.set(
        "set",
        lua.create_function(
            move |_,
                  (id, x, y, z, tx, ty, tz, fov): (
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                Option<f32>,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !Vec3::new(x, y, z).is_finite() || !Vec3::new(tx, ty, tz).is_finite() {
                    return Err(api_error("camera.set position/target must be finite"));
                }
                if let Some(fov) = fov
                    && (!fov.is_finite() || fov <= 0.0 || fov >= 180.0)
                {
                    return Err(api_error("camera.set fov must be in (0, 180) degrees"));
                }
                let Some(camera) = state.world.get_mut::<Camera3D>(id).map_err(world_error)? else {
                    return Err(api_error("entity has no camera3d"));
                };
                camera.position = Vec3::new(x, y, z);
                camera.target = Vec3::new(tx, ty, tz);
                if let Some(fov) = fov {
                    camera.fov_y = fov.to_radians();
                }
                Ok(true)
            },
        )?,
    )?;
    lua.globals().set("camera", camera)?;

    let light = lua.create_table()?;
    let s = Rc::clone(&state);
    light.set(
        "set_active",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if state.world.get::<Light>(id).map_err(world_error)?.is_none() {
                return Err(api_error("entity has no light"));
            }
            state.active_light = Some(id);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    light.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(light) = state.world.get::<Light>(id).map_err(world_error)? else {
                return Err(api_error("entity has no light"));
            };
            let t = lua.create_table()?;
            t.set("dx", light.direction.x)?;
            t.set("dy", light.direction.y)?;
            t.set("dz", light.direction.z)?;
            t.set("r", light.color[0])?;
            t.set("g", light.color[1])?;
            t.set("b", light.color[2])?;
            t.set("intensity", light.intensity)?;
            Ok(t)
        })?,
    )?;
    let s = Rc::clone(&state);
    light.set(
        "set",
        lua.create_function(
            move |_,
                  (id, dx, dy, dz, r, g, b, intensity): (
                i64,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                let id = entity_from_lua(id)?;
                let mut state = s.borrow_mut();
                require_alive(&state, id)?;
                if !intensity.is_finite() || intensity < 0.0 {
                    return Err(api_error("light intensity must be finite and non-negative"));
                }
                let values = [dx, dy, dz, r, g, b];
                if values.iter().any(|value| !value.is_finite()) {
                    return Err(api_error("light parameters must be finite"));
                }
                let light = state
                    .world
                    .get_mut::<Light>(id)
                    .map_err(world_error)?
                    .ok_or_else(|| api_error("entity has no light"))?;
                light.direction = Vec3::new(dx, dy, dz);
                light.color = [r, g, b];
                light.intensity = intensity;
                Ok(())
            },
        )?,
    )?;
    lua.globals().set("light", light)?;

    let health = lua.create_table()?;
    let s = Rc::clone(&state);
    health.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            Ok(state.health.get(&id).copied().unwrap_or(0.0))
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "set",
        lua.create_function(move |_, (id, value): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if !value.is_finite() || value < 0.0 {
                return Err(api_error("health must be finite and non-negative"));
            }
            state.health.insert(id, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    health.set(
        "damage",
        lua.create_function(move |_, (id, amount): (i64, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            if !amount.is_finite() || amount < 0.0 {
                return Err(api_error("damage must be finite and non-negative"));
            }
            let value = state.health.entry(id).or_insert(0.0);
            *value = (*value - amount).max(0.0);
            Ok(*value)
        })?,
    )?;
    lua.globals().set("health", health)?;

    let tilemap = lua.create_table()?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            let t = lua.create_table()?;
            t.set("tile_size", map.tile_size.x)?;
            t.set("width", map.width)?;
            t.set("height", map.height)?;
            Ok(mlua::Value::Table(t))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "is_solid",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.is_solid(x, y).unwrap_or(false))
        })?,
    )?;
    let s = Rc::clone(&state);
    tilemap.set(
        "get_cell",
        lua.create_function(move |_, (id, x, y): (i64, usize, usize)| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(map) = state.world.get::<Tilemap>(id).map_err(world_error)? else {
                return Err(api_error("entity has no tilemap"));
            };
            Ok(map.cell(x, y))
        })?,
    )?;
    lua.globals().set("tilemap", tilemap)?;

    let animation = lua.create_table()?;
    let s = Rc::clone(&state);
    animation.set(
        "get",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_state().to_owned())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            machine
                .set_state(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "frame",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(machine) = state
                .world
                .get::<AnimationStateMachine>(id)
                .map_err(world_error)?
            else {
                return Err(api_error("entity has no animation"));
            };
            Ok(machine.current_frame())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation.set(
        "ended",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let machine = state
                .world
                .get_mut::<AnimationStateMachine>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation"))?;
            Ok(machine.take_ended())
        })?,
    )?;
    lua.globals().set("animation", animation)?;

    let animation3d = lua.create_table()?;
    let s = Rc::clone(&state);
    animation3d.set(
        "get",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Ok(mlua::Value::Nil);
            };
            Ok(mlua::Value::String(
                lua.create_string(animation.current_name())?,
            ))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set",
        lua.create_function(move |_, (id, name): (i64, String)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation
                .set_clip(&name)
                .map_err(|error| api_error(error.to_string()))
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "list",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            let result = lua.create_table()?;
            for (index, name) in animation.clip_names().into_iter().enumerate() {
                result.set(index + 1, name)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "paused",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.is_paused())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "set_paused",
        lua.create_function(move |_, (id, paused): (i64, bool)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            let animation = state
                .world
                .get_mut::<Animation3D>(id)
                .map_err(world_error)?
                .ok_or_else(|| api_error("entity has no animation3d"))?;
            animation.set_paused(paused);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    animation3d.set(
        "time",
        lua.create_function(move |_, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let Some(animation) = state.world.get::<Animation3D>(id).map_err(world_error)? else {
                return Err(api_error("entity has no animation3d"));
            };
            Ok(animation.time())
        })?,
    )?;
    lua.globals().set("animation3d", animation3d)?;

    let physics = lua.create_table()?;
    let s = Rc::clone(&state);
    physics.set(
        "overlap",
        lua.create_function(move |lua, (x, y, radius): (f32, f32, f32)| {
            if !radius.is_finite() || radius < 0.0 {
                return Err(api_error("overlap radius must be finite and non-negative"));
            }
            let state = s.borrow();
            let ids = state.world.entities_with::<Transform>();
            let result = lua.create_table()?;
            let mut n = 1;
            for id in ids {
                if let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? {
                    let dx = t.position.x - x;
                    let dy = t.position.y - y;
                    if dx * dx + dy * dy <= radius * radius {
                        result.set(n, entity_to_lua(id))?;
                        n += 1;
                    }
                }
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "raycast",
        lua.create_function(
            move |_, (x, y, dx, dy, max_distance): (f32, f32, f32, f32, f32)| {
                if !max_distance.is_finite() || max_distance < 0.0 {
                    return Err(api_error(
                        "raycast distance must be finite and non-negative",
                    ));
                }
                let state = s.borrow();
                let mut hit = None;
                let mut best = max_distance;
                for id in state.world.entities_with::<Transform>() {
                    if let Some(t) = state.world.get::<Transform>(id).map_err(world_error)? {
                        let px = t.position.x - x;
                        let py = t.position.y - y;
                        let length = (dx * dx + dy * dy).sqrt();
                        if length > 0.0 {
                            let along = (px * dx + py * dy) / length;
                            let perp = (px * dy - py * dx).abs() / length;
                            if along >= 0.0 && along <= best && perp <= 0.5 {
                                best = along;
                                hit = Some(entity_to_lua(id));
                            }
                        }
                    }
                }
                Ok(hit)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "set_velocity",
        lua.create_function(move |_, (id, x, y): (i64, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics
                .set_velocity(id, x, y)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let velocity = state
                .physics
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            table_vec(lua, velocity.x, velocity.y)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics.set(
        "events",
        lua.create_function(move |lua, ()| {
            let mut state = s.borrow_mut();
            let events = state.physics.drain_contacts();
            let result = lua.create_table()?;
            for (index, contact) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("a", entity_to_lua(contact.a))?;
                entry.set("b", entity_to_lua(contact.b))?;
                entry.set("started", contact.started)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("physics", physics)?;

    // 3D physics (M9-02): same shape as `physics`, but bodies are created from
    // scene `Collider3d` components and positions sync to `Transform3D`.
    let physics3d = lua.create_table()?;
    let s = Rc::clone(&state);
    physics3d.set(
        "set_velocity",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics3d
                .set_velocity(id, x, y, z)
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "get_velocity",
        lua.create_function(move |lua, id: i64| {
            let id = entity_from_lua(id)?;
            let state = s.borrow();
            require_alive(&state, id)?;
            let velocity = state
                .physics3d
                .velocity(id)
                .map_err(|error| api_error(error.to_string()))?;
            let table = lua.create_table()?;
            table.set("x", velocity.x)?;
            table.set("y", velocity.y)?;
            table.set("z", velocity.z)?;
            Ok(table)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "set_position",
        lua.create_function(move |_, (id, x, y, z): (i64, f32, f32, f32)| {
            let id = entity_from_lua(id)?;
            let mut state = s.borrow_mut();
            require_alive(&state, id)?;
            state
                .physics3d
                .set_position(id, Vec3::new(x, y, z))
                .map_err(|error| api_error(error.to_string()))?;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "events",
        lua.create_function(move |lua, ()| {
            let mut state = s.borrow_mut();
            let events = state.physics3d.drain_contacts();
            let result = lua.create_table()?;
            for (index, contact) in events.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("a", entity_to_lua(contact.a))?;
                entry.set("b", entity_to_lua(contact.b))?;
                entry.set("started", contact.started)?;
                result.set(index + 1, entry)?;
            }
            Ok(result)
        })?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "raycast",
        lua.create_function(
            move |_, (ox, oy, oz, dx, dy, dz, max_distance): (f32, f32, f32, f32, f32, f32, f32)| {
                let state = s.borrow();
                let hit = state
                    .physics3d
                    .raycast(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz), max_distance)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(hit.map(entity_to_lua))
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    physics3d.set(
        "overlap",
        lua.create_function(move |lua, (cx, cy, cz, radius): (f32, f32, f32, f32)| {
            let state = s.borrow();
            let hits = state.physics3d.overlap(Vec3::new(cx, cy, cz), radius);
            let result = lua.create_table()?;
            for (index, id) in hits.into_iter().enumerate() {
                result.set(index + 1, entity_to_lua(id))?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("physics3d", physics3d)?;

    let particles = lua.create_table()?;
    let s = Rc::clone(&state);
    particles.set(
        "burst",
        lua.create_function(move |_, (x, y, count): (f32, f32, u32)| {
            s.borrow_mut().particles.burst(
                Vec2::new(x, y),
                count as usize,
                120.0,
                0.6,
                [1.0, 0.6, 0.2, 1.0],
            );
            Ok(())
        })?,
    )?;
    lua.globals().set("particles", particles)?;

    let audio = lua.create_table()?;
    let s = Rc::clone(&state);
    audio.set(
        "play",
        lua.create_function(move |_, name: String| {
            let mut state = s.borrow_mut();
            if let Err(error) = state.audio.play(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] play failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "volume",
        lua.create_function(move |_, level: f32| {
            s.borrow_mut().audio.set_volume(level);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "music",
        lua.create_function(move |_, name: String| {
            let mut state = s.borrow_mut();
            if let Err(error) = state.audio.play_music(&name) {
                tracing::warn!(target: "audio", name = %name, error = %error, "[audio] music start failed");
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    audio.set(
        "stop_music",
        lua.create_function(move |_, ()| {
            s.borrow_mut().audio.stop_music();
            Ok(())
        })?,
    )?;
    lua.globals().set("audio", audio)?;

    let scene = lua.create_table()?;
    let s = Rc::clone(&state);
    scene.set(
        "load",
        lua.create_function(move |_, name: String| {
            let state = s.borrow();
            let exists = state.project_root.as_ref().is_some_and(|root| {
                root.join("scenes").join(format!("{name}.scene")).is_file()
            });
            if exists {
                drop(state);
                s.borrow_mut().scene_pending_load = Some(name);
            } else {
                tracing::warn!(target: "scene", name = %name, "[scene] scene file not found; load ignored");
            }
            Ok(exists)
        })?,
    )?;
    let s = Rc::clone(&state);
    scene.set(
        "reload",
        lua.create_function(move |_, ()| Ok(s.borrow().scene.clone()))?,
    )?;
    lua.globals().set("scene", scene)?;
    let asset = lua.create_table()?;
    let s = Rc::clone(&state);
    asset.set(
        "load",
        lua.create_function(move |_, path: String| {
            let mut state = s.borrow_mut();
            let token = format!("asset:{}", path);
            state.assets.insert(path, token.clone());
            Ok(token)
        })?,
    )?;
    let s = Rc::clone(&state);
    asset.set(
        "release",
        lua.create_function(move |_, token: String| {
            s.borrow_mut().assets.retain(|_, v| v != &token);
            Ok(())
        })?,
    )?;
    lua.globals().set("asset", asset)?;
    let ui = lua.create_table()?;
    let s = Rc::clone(&state);
    ui.set(
        "text",
        lua.create_function(move |_, (id, text): (String, String)| {
            s.borrow_mut().ui.insert(
                id,
                UiText {
                    text,
                    visible: true,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_at",
        lua.create_function(
            move |_, (id, x, y, text, scale, font_size): (
                String,
                f32,
                f32,
                String,
                Option<f32>,
                Option<f32>,
            )| {
                s.borrow_mut().ui.insert(
                    id,
                    UiText {
                        text,
                        visible: true,
                        position: [x, y],
                        scale: scale.unwrap_or(1.0),
                        font_size,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "bar",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, ratio, r, g, b): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                s.borrow_mut().ui_bars.insert(
                    id,
                    UiBar {
                        position: [x, y],
                        size: [width, height],
                        ratio: ratio.clamp(0.0, 1.0),
                        color: [r, g, b, 1.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "show",
        lua.create_function(move |_, id: String| {
            s.borrow_mut()
                .ui
                .entry(id)
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = true;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "hide",
        lua.create_function(move |_, id: String| {
            s.borrow_mut()
                .ui
                .entry(id)
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = false;
            Ok(())
        })?,
    )?;
    lua.globals().set("ui", &ui)?;
    let s = Rc::clone(&state);
    ui.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow();
            let Some(value) = state.ui.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", value.text.clone())?;
            result.set("visible", value.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let save = lua.create_table()?;
    let s = Rc::clone(&state);
    save.set(
        "write",
        lua.create_function(move |_, (key, value): (String, String)| {
            s.borrow_mut().save_data.insert(key, value);
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "read",
        lua.create_function(move |_, key: String| Ok(s.borrow().save_data.get(&key).cloned()))?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "commit",
        lua.create_function(move |_, ()| {
            let state = s.borrow();
            let Some(path) = state.save_path.clone() else {
                tracing::warn!(target: "save", "[save] no save path configured; commit ignored");
                return Ok(false);
            };
            let text = encode_save(&state.save_data);
            let result = atomic_write(&path, &text);
            match &result {
                Ok(()) => tracing::info!(target: "save", path = %path.display(), "[save] committed"),
                Err(error) => {
                    tracing::error!(target: "save", path = %path.display(), error = %error, "[save] commit failed")
                }
            }
            Ok(result.is_ok())
        })?,
    )?;
    let s = Rc::clone(&state);
    save.set(
        "load",
        lua.create_function(move |_, ()| {
            let mut state = s.borrow_mut();
            let Some(path) = state.save_path.clone() else {
                tracing::warn!(target: "save", "[save] no save path configured; load ignored");
                return Ok(false);
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return Ok(false); // no save yet is not an error
            };
            match decode_save(&text) {
                Ok(data) => {
                    state.save_data = data;
                    tracing::info!(target: "save", path = %path.display(), "[save] loaded");
                    Ok(true)
                }
                Err(error) => {
                    tracing::warn!(target: "save", path = %path.display(), error = %error, "[save] corrupt save ignored");
                    Ok(false)
                }
            }
        })?,
    )?;
    lua.globals().set("save", save)?;

    let app = lua.create_table()?;
    let s = state;
    app.set(
        "quit",
        lua.create_function(move |_, ()| {
            s.borrow_mut().quit_requested = true;
            Ok(())
        })?,
    )?;
    lua.globals().set("app", app)?;
    Ok(())
}

fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}
fn parse_key(key: &str) -> InputKey {
    match key.to_ascii_lowercase().as_str() {
        "w" => InputKey::W,
        "a" => InputKey::A,
        "s" => InputKey::S,
        "d" => InputKey::D,
        "space" => InputKey::Space,
        "escape" => InputKey::Escape,
        "f1" => InputKey::Other(1),
        "f2" => InputKey::Other(2),
        "f4" => InputKey::Other(4),
        "r" => InputKey::Other(99), // 重玩一局
        _ => InputKey::Other(0),
    }
}
fn parse_mouse_button(button: &str) -> MouseButton {
    match button.to_ascii_lowercase().as_str() {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => MouseButton::Other(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::sprite::Sprite;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn script(contents: &str) -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("p2-lua-{id}.lua"));
        fs::write(&path, contents).unwrap();
        path
    }

    fn production_sample_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
    }

    #[test]
    fn registers_log_time_input_and_quit_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_delta(0.25);
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        runtime.set_input(input);
        let path = script(
            "assert(time.delta() == 0.25)\nassert(input.is_down('d'))\nassert(input.axis('horizontal') == 1.0)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        runtime.lua.load("app.quit()").exec().unwrap();
        assert!(runtime.quit_requested());
        let _ = fs::remove_file(path);
    }
    #[test]
    fn lua_gameplay_api_moves_damages_and_destroys() {
        let path = script(
            "player=entity.spawn('player')\ntransform.set(player,0,0)\ntransform.move(player,2,3)\nlocal p=transform.get(player)\nassert(p.x==2 and p.y==3)\nhealth.set(player,10)\nassert(health.damage(player,4)==6)\nassert(entity.find('player')==player)\nentity.destroy(player)\nassert(not entity.is_alive(player))\nfunction update() return 'ok' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn instantiated_scene_entities_are_shared_with_lua_movement() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".into(), "assets/textures/red-1x1.png".into());

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\nlocal p = entity.find('Player')\nassert(p ~= nil)\ntransform.move(p, 10, 5)\nreturn 'moved' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "moved");

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 1);
        let player = player[0];
        assert!(runtime.with_world(|world| world.has::<Sprite>(player).unwrap()));
        assert!(runtime.with_world(|world| world.has::<SpriteAsset>(player).unwrap()));
        let asset =
            runtime.with_world(|world| world.get::<SpriteAsset>(player).unwrap().unwrap().clone());
        assert_eq!(asset.path(), "assets/textures/red-1x1.png");
        let moved = runtime.with_world(|world| *world.get::<Transform>(player).unwrap().unwrap());
        assert_eq!(moved.position, Vec2::new(10.0, 5.0));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wasd_input_axis_moves_the_instantiated_player() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();
        let path = script(
            "local speed = 10\nfunction update()\nlocal p = entity.find('Player')\nlocal h = input.axis('horizontal')\nlocal v = input.axis('vertical')\ntransform.move(p, h * speed, v * speed)\nreturn 'moved' end",
        );
        runtime.execute_project_script(&path).unwrap();

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 1);
        let player = player[0];

        // Hold D -> horizontal axis = +1 -> player moves right.
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        runtime.set_input(input);
        runtime.call_update().unwrap();
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert_eq!(position.x, 10.0);
        assert_eq!(position.y, 0.0);

        // Release D, hold W -> vertical axis = +1 -> player moves up only.
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::W, true);
        runtime.set_input(input);
        runtime.call_update().unwrap();
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert_eq!(position.x, 10.0);
        assert_eq!(position.y, 10.0);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn instantiated_tilemap_is_queryable_from_lua() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Map", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Tilemap".into(),
                "tile_size=32x32;width=4;height=3;tiles=100101000100;solid=100000000000".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function on_create()\nlocal m = entity.find('Map')\nlocal d = tilemap.get(m)\nassert(d ~= nil)\nassert(d.width == 4 and d.height == 3 and d.tile_size == 32)\nassert(tilemap.is_solid(m, 0, 0))\nassert(not tilemap.is_solid(m, 1, 1))\nassert(not tilemap.is_solid(m, 9, 9))\nassert(tilemap.get_cell(m, 3, 0) == 1)\nassert(tilemap.get_cell(m, 1, 0) == 0)\nreturn 'ok' end\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        let maps = runtime.with_world(|world| world.entities_with::<Tilemap>());
        assert_eq!(maps.len(), 1);
        let map =
            runtime.with_world(|world| world.get::<Tilemap>(maps[0]).unwrap().unwrap().clone());
        assert_eq!(map.width, 4);
        assert_eq!(map.is_solid(0, 0), Some(true));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn instantiated_animation_is_switchable_from_lua() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        scene.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
            "Animation".into(),
            "default=idle;clip:idle:assets/idle-sheet.png:4:4:true;clip:attack:assets/attack-sheet.png:2:2:false".into(),
        );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let first = script(
            "function on_create()\nlocal h = entity.find('Hero')\nassert(animation.get(h) == 'idle')\nassert(animation.set(h, 'attack'))\nassert(animation.get(h) == 'attack')\nassert(not animation.set(h, 'attack'))\nreturn 'ok' end\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&first).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");

        // advance past the 1.0s non-looping attack clip
        let machines = runtime.with_world(|world| world.entities_with::<AnimationStateMachine>());
        assert_eq!(machines.len(), 1);
        runtime.with_world_mut(|world| {
            world
                .get_mut::<AnimationStateMachine>(machines[0])
                .unwrap()
                .unwrap()
                .advance(1.1);
        });

        let second = script(
            "function update()\nlocal h = entity.find('Hero')\nassert(animation.frame(h) == 1)\nassert(animation.ended(h))\nassert(not animation.ended(h))\nreturn 'done' end",
        );
        runtime.execute_project_script(&second).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "done");
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
    }

    #[test]
    fn physics_blocks_player_on_tilemap_wall_and_emits_events() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("test");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Collider".into(), "shape=ball;radius=10".into());
        scene.add_entity(SceneEntityId(2), "Map", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        // 2x2 tilemap, right column solid -> a wall spanning x=20..40
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Tilemap".into(),
                "tile_size=20x20;width=2;height=2;tiles=0000;solid=0101".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let path = script(
            "function update()\nlocal p = entity.find('Player')\nphysics.set_velocity(p, 40, 0)\nreturn 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        for _ in 0..60 {
            runtime.step_physics(1.0 / 60.0);
        }

        let player = runtime.with_world(|world| world.entities_with::<Transform>());
        assert_eq!(player.len(), 2);
        let player = player[0];
        let position =
            runtime.with_world(|world| world.get::<Transform>(player).unwrap().unwrap().position);
        assert!(
            position.x < 15.0,
            "player was not blocked by the tilemap wall: x={}",
            position.x
        );

        let events = script(
            "function update()\nlocal events = physics.events()\nreturn tostring(#events) end",
        );
        runtime.execute_project_script(&events).unwrap();
        let count = runtime.call_update().unwrap();
        let count: u32 = count.parse().unwrap();
        assert!(count > 0, "no collision events reached Lua");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(events);
    }

    #[test]
    fn physics3d_body_falls_onto_static_ground_and_syncs_transform3d() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("3d");
        scene.add_entity(SceneEntityId(1), "Ball", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Collider3d".into(), "shape=ball;radius=1".into());
        scene.add_entity(SceneEntityId(2), "Ground", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        // Static ground: cuboid spanning y=-2..0 (top at y=0), centered at (0,-1,0).
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Collider3d".into(),
                "shape=cuboid;half_width=10;half_height=1;half_z=10;offset=0,-1,0;dynamic=false"
                    .into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.instantiate_scene(&scene).unwrap();

        let path = script(
            "function update()\nlocal b = entity.find('Ball')\nphysics3d.set_velocity(b, 0, 0, 2)\nreturn 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        for _ in 0..180 {
            runtime.step_physics3d(1.0 / 60.0);
        }

        let positions: Vec<_> = runtime.with_world(|world| {
            world
                .entities_with::<Transform3D>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<Transform3D>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.position)
                })
                .collect()
        });
        assert_eq!(positions.len(), 2);
        let ball_pos = positions.iter().find(|p| p.y > 0.5).copied().unwrap();
        // Ball radius 1 resting on the ground top (y=0) => y ≈ 1; it also moved +z.
        assert!(
            (ball_pos.y - 1.0).abs() < 0.1,
            "ball should rest at y≈1, got y={}",
            ball_pos.y
        );
        assert!(
            ball_pos.z > 2.0,
            "ball should have moved in +z, got z={}",
            ball_pos.z
        );

        // Collision events reached Lua via the physics3d table.
        let events = script(
            "function update()\nlocal events = physics3d.events()\nreturn tostring(#events) end",
        );
        runtime.execute_project_script(&events).unwrap();
        let count = runtime.call_update().unwrap();
        let count: u32 = count.parse().unwrap();
        assert!(count > 0, "no 3D collision events reached Lua");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(events);
    }

    #[test]
    fn ui_particles_and_audio_lua_api() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "particles.burst(5, -3, 10)\nui.bar('hp', 20, 30, 100, 10, 0.5, 1, 0, 0)\nui.text_at('objective', 20, 20, 'Reach the door')\naudio.volume(0.5)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        runtime.step_particles(0.1);

        let count = runtime.with_particles(|particles| particles.len());
        assert!(count >= 10, "particles.burst should spawn at least 10");
        let (texts, bars) = runtime.with_ui(|texts, bars| (texts.clone(), bars.clone()));
        let text = texts.get("objective").expect("objective text stored");
        assert_eq!(text.text, "Reach the door");
        let bar = bars.get("hp").expect("hp bar stored");
        assert_eq!(bar.ratio, 0.5);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ui_text_at_accepts_optional_scale_and_font_size() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "ui.text_at('small', 10, 10, 'a')\nui.text_at('big', 10, 30, 'b', 3)\nui.text_at('huge', 10, 50, 'c', nil, 24)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        let texts = runtime.with_ui(|texts, _| texts.clone());
        assert_eq!(
            texts.get("small").expect("small text stored").scale,
            1.0,
            "no scale argument defaults to 1.0"
        );
        assert_eq!(
            texts.get("small").expect("small text stored").font_size,
            None,
            "no font_size argument stays None"
        );
        assert_eq!(
            texts.get("big").expect("big text stored").scale,
            3.0,
            "optional scale argument is honored"
        );
        let huge = texts.get("huge").expect("huge text stored");
        assert_eq!(huge.scale, 1.0, "scale stays default when font_size given");
        assert_eq!(
            huge.font_size,
            Some(24.0),
            "optional font_size argument is honored"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn project_script_can_require_sibling_lua_modules() {
        let root = std::env::temp_dir().join(format!("p2-module-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("helper.lua"),
            "local M = {}\nfunction M.answer() return 42 end\nreturn M\n",
        )
        .unwrap();
        let main = root.join("main.lua");
        fs::write(
            &main,
            "local h = require('helper')\nassert(h.answer() == 42)\nfunction update() return 'ok' end\n",
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&main).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn encrypted_project_and_modules_load_via_decode_and_require() {
        let root = std::env::temp_dir().join(format!("p2-enc-module-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        // 用发布包格式（压缩+加密）写模块与主脚本。
        let helper_src = "local M = {}\nfunction M.answer() return 7 end\nreturn M\n".as_bytes();
        fs::write(
            root.join("helper.lua"),
            crate::core::pack::encode(helper_src, true, true),
        )
        .unwrap();
        let main = root.join("main.lua");
        let main_src = "local h = require('helper')\nassert(h.answer() == 7)\nfunction update() return 'enc-ok' end\n";
        fs::write(
            &main,
            crate::core::pack::encode(main_src.as_bytes(), true, true),
        )
        .unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&main).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "enc-ok");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn save_commit_load_and_scene_transition() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-save-scene-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();
        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
            );
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".into(), "assets/player.png".into());
        arena.save(scenes.join("arena.scene")).unwrap();

        // save + commit to disk
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(root.join(".p2").join("save.sav"));
        let first = script(
            "save.write('hp', '75')\nsave.write('scene', 'arena')\nassert(save.commit())\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&first).unwrap();
        runtime.call_update().unwrap();
        assert!(root.join(".p2").join("save.sav").is_file());

        // a fresh runtime loads the save back
        let mut fresh = LuaRuntime::new().unwrap();
        fresh.set_save_path(root.join(".p2").join("save.sav"));
        let load_script = script(
            "assert(save.load())\nassert(save.read('hp') == '75')\nfunction update() return 'ok' end",
        );
        fresh.execute_project_script(&load_script).unwrap();
        fresh.call_update().unwrap();

        // scene.load queues a transition; process_scene_transition applies it
        let switch = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&switch).unwrap();
        runtime.process_scene_transition().unwrap();
        let players = runtime.with_world(|world| world.entities_with::<Sprite>());
        assert_eq!(players.len(), 1, "arena player instantiated after switch");
        assert_eq!(runtime.with_world(|world| world.entity_count()), 1);
        // Lua can find the freshly-instantiated player after the transition
        let check = script(
            "local p = entity.find('Player')\nassert(p ~= nil)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&check).unwrap();
        runtime.call_update().unwrap();

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(load_script);
        let _ = fs::remove_file(switch);
        let _ = fs::remove_file(check);
    }

    #[test]
    fn persistent_entities_survive_scene_transition() {
        use crate::core::scene::{Scene, SceneEntityId};
        use crate::core::ui::UiText;

        let root = std::env::temp_dir().join(format!("p2-persist-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();

        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Player", None).unwrap();
        arena
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".into(), "assets/player.png".into());
        arena.add_entity(SceneEntityId(2), "Hud", None).unwrap();
        let hud = arena.entities.get_mut(&SceneEntityId(2)).unwrap();
        hud.components.insert(
            "Text".into(),
            "text=Score:0;x=10;y=10;scale=2;color=1,1,1".into(),
        );
        hud.metadata.insert("persistent".into(), "true".into());
        arena.save(scenes.join("arena.scene")).unwrap();

        let mut hub = Scene::new("hub");
        hub.add_entity(SceneEntityId(1), "Boss", None).unwrap();
        hub.entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".into(), "assets/boss.png".into());
        hub.save(scenes.join("hub.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load_arena = script("assert(scene.load('arena'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_arena).unwrap();
        runtime.process_scene_transition().unwrap();
        let load_hub = script("assert(scene.load('hub'))\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_hub).unwrap();
        runtime.process_scene_transition().unwrap();

        // Hud (persistent, UI text) survives; arena Player gone; hub Boss present.
        let ui_texts: Vec<String> = runtime.with_world(|world| {
            world
                .entities_with::<UiText>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<UiText>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.text.clone())
                })
                .collect()
        });
        assert_eq!(ui_texts.len(), 1, "persistent Hud text survives");
        assert_eq!(ui_texts[0], "Score:0");
        let sprites = runtime.with_world(|world| world.entities_with::<Sprite>());
        assert_eq!(sprites.len(), 1, "only hub Boss sprite remains");
        assert_eq!(runtime.with_world(|world| world.entity_count()), 2);
        let check = script(
            "local hud = entity.find('Hud')\nassert(hud ~= nil)\nlocal boss = entity.find('Boss')\nassert(boss ~= nil)\nassert(entity.find('Player') == nil)\nfunction update() return 'ok' end",
        );
        runtime.execute_project_script(&check).unwrap();
        runtime.call_update().unwrap();

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load_arena);
        let _ = fs::remove_file(load_hub);
        let _ = fs::remove_file(check);
    }

    #[test]
    fn persistent_duplicate_name_new_scene_wins() {
        use crate::core::scene::{Scene, SceneEntityId};
        use crate::core::ui::UiText;

        let root = std::env::temp_dir().join(format!("p2-persist-name-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();

        let mut arena = Scene::new("arena");
        arena.add_entity(SceneEntityId(1), "Hud", None).unwrap();
        let hud = arena.entities.get_mut(&SceneEntityId(1)).unwrap();
        hud.components
            .insert("Text".into(), "text=Score:0;x=10;y=10".into());
        hud.metadata.insert("persistent".into(), "true".into());
        arena.save(scenes.join("arena.scene")).unwrap();

        let mut hub = Scene::new("hub");
        hub.add_entity(SceneEntityId(1), "Hud", None).unwrap();
        let hud = hub.entities.get_mut(&SceneEntityId(1)).unwrap();
        hud.components
            .insert("Text".into(), "text=Score:99;x=10;y=10".into());
        hud.metadata.insert("persistent".into(), "true".into());
        hub.save(scenes.join("hub.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load_arena = script("scene.load('arena')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_arena).unwrap();
        runtime.process_scene_transition().unwrap();
        let load_hub = script("scene.load('hub')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load_hub).unwrap();
        runtime.process_scene_transition().unwrap();

        let ui_texts: Vec<String> = runtime.with_world(|world| {
            world
                .entities_with::<UiText>()
                .into_iter()
                .filter_map(|id| {
                    world
                        .get::<UiText>(id)
                        .ok()
                        .flatten()
                        .map(|t| t.text.clone())
                })
                .collect()
        });
        assert_eq!(ui_texts.len(), 1, "exactly one Hud");
        assert_eq!(ui_texts[0], "Score:99", "new scene's persistent Hud wins");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load_arena);
        let _ = fs::remove_file(load_hub);
    }

    #[test]
    fn button_click_calls_lua_on_click() {
        use crate::core::input::MouseButton;
        use crate::core::scene::{Scene, SceneEntityId};

        let root = std::env::temp_dir().join(format!("p2-button-{}", std::process::id()));
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).unwrap();
        let mut menu = Scene::new("menu");
        menu.add_entity(SceneEntityId(1), "Start", None).unwrap();
        menu.entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Button".into(),
                "label=Start;x=10;y=10;width=100;height=32;bg=0.2,0.45,0.8;text_color=1,1,1".into(),
            );
        menu.save(scenes.join("menu.scene")).unwrap();

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        let load = script("scene.load('menu')\nfunction update() return 'ok' end");
        runtime.execute_project_script(&load).unwrap();
        runtime.process_scene_transition().unwrap();

        // 定义全局 on_click + 检查用 update（同一脚本，VM 不替换）。
        let click = script(
            "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
        );
        runtime.execute_project_script(&click).unwrap();
        let mut input = InputSnapshot::default();
        input.cursor_moved(50.0, 20.0);
        input.mouse_event(MouseButton::Left, true);
        runtime.process_ui_clicks(&input, 1280, 720).unwrap();

        let result = runtime.call_update().unwrap();
        assert_eq!(
            result, "Start",
            "on_click('Start') fired for the clicked button"
        );

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_file(load);
        let _ = fs::remove_file(click);
    }

    #[test]
    fn production_sample_main_script_runs_menu_update() {
        use crate::core::scene::Scene;
        use crate::core::ui::UiButton;

        let root = production_sample_root();
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 7);
        runtime
            .execute_project_script(&root.join("scripts/main.lua"))
            .expect("load sample main.lua");

        assert_eq!(runtime.call_update().unwrap(), "menu_ready");
        let buttons = runtime.with_world(|world| world.entities_with::<UiButton>().len());
        assert_eq!(buttons, 2, "menu scene should expose two buttons");
    }

    #[test]
    fn production_sample_start_button_loads_production_scene() {
        use crate::core::{
            animation::AnimationStateMachine, input::MouseButton, scene::Scene, tilemap::Tilemap,
        };

        let root = production_sample_root();
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.instantiate_scene(&scene).unwrap();
        runtime
            .execute_project_script(&root.join("scripts/main.lua"))
            .expect("load sample main.lua");

        let mut input = InputSnapshot::default();
        input.cursor_moved(520.0, 400.0);
        input.mouse_event(MouseButton::Left, true);
        runtime.process_ui_clicks(&input, 1280, 720).unwrap();
        runtime.process_scene_transition().unwrap();

        let result = runtime.call_update().unwrap();
        assert!(
            result.starts_with("production_ok"),
            "production gameplay update should run after transition: {result}"
        );
        let counts = runtime.with_world(|world| {
            (
                world.entities_with::<Tilemap>().len(),
                world.entities_with::<AnimationStateMachine>().len(),
            )
        });
        assert_eq!(counts.0, 1, "production scene should load one tilemap");
        assert_eq!(
            counts.1, 11,
            "production scene should load player plus ten animated enemies"
        );
    }

    #[test]
    fn production_sample_can_save_and_return_to_menu() {
        use crate::core::{input::InputKey, scene::Scene, ui::UiButton};

        let root = production_sample_root();
        let save_path = std::env::temp_dir().join(format!(
            "p2-production-sample-save-{}.sav",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        ));
        let _ = fs::remove_file(&save_path);
        let scene = Scene::load(root.join("scenes/menu.scene")).expect("load sample menu");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        runtime.set_save_path(save_path.clone());
        runtime.instantiate_scene(&scene).unwrap();
        runtime
            .execute_project_script(&root.join("scripts/main.lua"))
            .expect("load sample main.lua");

        let mut click = InputSnapshot::default();
        click.cursor_moved(520.0, 400.0);
        click.mouse_event(crate::core::input::MouseButton::Left, true);
        runtime.process_ui_clicks(&click, 1280, 720).unwrap();
        runtime.process_scene_transition().unwrap();

        let mut save_input = InputSnapshot::default();
        save_input.key_event(InputKey::Other(1), true);
        runtime.set_input(save_input);
        let save_result = runtime.call_update().unwrap();
        assert!(
            save_result.starts_with("production_ok"),
            "save frame should continue gameplay update: {save_result}"
        );
        assert!(save_path.is_file(), "F1 should commit a save file");

        let mut menu_input = InputSnapshot::default();
        menu_input.key_event(InputKey::Other(4), true);
        runtime.set_input(menu_input);
        let switch_result = runtime.call_update().unwrap();
        assert_eq!(switch_result, "return_menu");
        runtime.process_scene_transition().unwrap();
        runtime.set_input(InputSnapshot::default());
        assert_eq!(runtime.call_update().unwrap(), "menu_ready");
        let buttons = runtime.with_world(|world| world.entities_with::<UiButton>().len());
        assert_eq!(buttons, 2, "menu scene should be restored after F4");

        let _ = fs::remove_file(save_path);
    }

    #[test]
    fn lua_3d_api_moves_mesh_and_switches_camera() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("3d");
        scene.add_entity(SceneEntityId(1), "Camera", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
            );
        scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Light".into(),
                "direction=0.4,0.8,0.5;color=1,1,1;intensity=1".into(),
            );
        scene.add_entity(SceneEntityId(3), "Hero", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(3))
            .unwrap()
            .components
            .insert(
                "Transform3D".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(3))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/hero.glb".into());

        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 3);

        let path = script(
            "function update()\n\
             local h = entity.find('Hero')\n\
             assert(h ~= nil)\n\
             assert(mesh.get(h) == 'assets/models/hero.glb')\n\
             transform3d.move(h, 2, 3, 4)\n\
             local t = transform3d.get(h)\n\
             assert(t.x == 2 and t.y == 3 and t.z == 4)\n\
             local cam = entity.find('Camera')\n\
             assert(camera.get_active() == nil)\n\
             assert(camera.set_active(cam))\n\
             assert(camera.get_active() == cam)\n\
             local c = camera.get(cam)\n\
             assert(math.abs(c.fov - 90) < 0.001 and math.abs(c.near - 0.1) < 0.001 and c.far == 100)\n\
             local sun = entity.find('Sun')\n\
             light.set_active(sun)\n\
             light.set(sun, 0, 1, 0, 1, 0.5, 0.25, 2)\n\
             return '3d-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "3d-ok");

        assert_eq!(
            runtime.active_camera(),
            Some(runtime.with_world(|world| world.entities_with::<Camera3D>()[0]))
        );
        assert_eq!(
            runtime.active_light(),
            Some(runtime.with_world(|world| world.entities_with::<Light>()[0]))
        );
        let moved = runtime.with_world(|world| {
            let hero = world.entities_with::<MeshRenderer>()[0];
            *world.get::<Transform3D>(hero).unwrap().unwrap()
        });
        assert_eq!(moved.position, Vec3::new(2.0, 3.0, 4.0));
        let light = runtime.with_world(|world| {
            let sun = world.entities_with::<Light>()[0];
            *world.get::<Light>(sun).unwrap().unwrap()
        });
        assert_eq!(light.direction, Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(light.intensity, 2.0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn camera_set_moves_position_target_and_fov() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("cam");
        scene.add_entity(SceneEntityId(1), "Cam", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Camera3D".into(),
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100".into(),
            );
        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\n\
             local cam = entity.find('Cam')\n\
             assert(camera.set(cam, 1, 2, 3, 4, 5, 6, 45))\n\
             local c = camera.get(cam)\n\
             assert(c.x == 1 and c.y == 2 and c.z == 3)\n\
             assert(c.tx == 4 and c.ty == 5 and c.tz == 6)\n\
             assert(math.abs(c.fov - 45) < 0.001)\n\
             local bad = pcall(function() camera.set(cam, 0, 0, 0, 0, 0, 0, 0) end)\n\
             assert(not bad, 'fov=0 must be rejected')\n\
             return 'cam-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "cam-ok");
        let cam = runtime.with_world(|world| {
            let id = world.entities_with::<Camera3D>()[0];
            *world.get::<Camera3D>(id).unwrap().unwrap()
        });
        assert_eq!(cam.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(cam.target, Vec3::new(4.0, 5.0, 6.0));
        assert!((cam.fov_y.to_degrees() - 45.0).abs() < 0.001);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn lua_3d_attach_and_validation_errors() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "local e = entity.spawn('Cube')\n\
             transform3d.attach(e)\n\
             mesh.set(e, 'assets/models/cube.glb')\n\
             transform3d.set(e, 1, 2, 3)\n\
             local t = transform3d.get(e)\n\
             assert(t.x == 1 and t.y == 2 and t.z == 3)\n\
             assert(t.rw == 1 and t.sx == 1)\n\
             local ok = pcall(function() transform3d.set(e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0) end)\n\
             assert(not ok, 'zero-scale must be rejected')\n\
             local bad = pcall(function() mesh.set(e, 'assets/models/cube.obj') end)\n\
             assert(not bad, 'non-glb mesh must be rejected')\n\
             function update() return 'ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "ok");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn lua_3d_animation_queries_and_switches_clips() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test");
        let mut scene = Scene::new("3d-anim");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Animation3D".into(),
                "mesh=assets/models/m8-skin-quad.glb;clip=Bend;looped=true".into(),
            );

        let mut runtime = LuaRuntime::new().unwrap();
        runtime.set_project_root(root.clone());
        assert_eq!(runtime.instantiate_scene(&scene).unwrap(), 1);

        let path = script(
            "function update()\n\
             local h = entity.find('Hero')\n\
             assert(animation3d.get(h) == 'Bend')\n\
             local list = animation3d.list(h)\n\
             assert(#list == 1 and list[1] == 'Bend')\n\
             assert(not animation3d.set(h, 'Bend'))\n\
             assert(animation3d.time(h) >= 0)\n\
             assert(not animation3d.paused(h))\n\
             animation3d.set_paused(h, true)\n\
             assert(animation3d.paused(h))\n\
             return 'anim3d-ok' end",
        );
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "anim3d-ok");

        // The Animation3D component is present in the world and advancing works.
        let animations = runtime.with_world(|world| world.entities_with::<Animation3D>());
        assert_eq!(animations.len(), 1);
        runtime.with_world_mut(|world| {
            world
                .get_mut::<Animation3D>(animations[0])
                .unwrap()
                .unwrap()
                .advance(0.5);
        });
        assert!(
            runtime.with_world(|world| world
                .get::<Animation3D>(animations[0])
                .unwrap()
                .unwrap()
                .time())
                == 0.0,
            "paused animation must not advance"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn call_update_without_script_idles_instead_of_erroring() {
        // Engine-only diagnostic mode has no project script; update must be a
        // no-op rather than a fatal "nil to function" error (which used to
        // crash double-clicked runs).
        let runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.call_update().unwrap(), "");
    }

    #[test]
    fn invalid_entity_is_a_lua_error() {
        let path = script("transform.get(4294967297)");
        let mut runtime = LuaRuntime::new().unwrap();
        let error = runtime
            .execute_project_script(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid entity id"));
        let _ = fs::remove_file(path);
    }
    #[test]
    fn hot_reload_detects_changes_and_keeps_old_vm_on_failure() {
        let path = script("function update() return 'v1' end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "v1");
        assert_eq!(
            runtime.reload_if_changed().unwrap(),
            ReloadStatus::Unchanged
        );
        fs::write(&path, "function update() return 'v2' end").unwrap();
        assert_eq!(runtime.reload_if_changed().unwrap(), ReloadStatus::Reloaded);
        assert_eq!(runtime.call_update().unwrap(), "v2");
        fs::write(&path, "function update(").unwrap();
        assert!(runtime.reload_if_changed().is_err());
        assert_eq!(runtime.call_update().unwrap(), "v2");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn hot_reload_calls_lifecycle_callbacks() {
        let path = script(
            "function on_create() end; function on_destroy() end; function update() return 'v1' end",
        );
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        fs::write(&path, "function on_create() end; function on_destroy() end; function update() return 'v2' end").unwrap();
        runtime.reload_if_changed().unwrap();
        assert_eq!(runtime.call_update().unwrap(), "v2");
        let _ = fs::remove_file(path);
    }
    #[test]
    fn console_evaluates_values_and_updates_lua_state() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert_eq!(runtime.execute_console("return 1 + 2").unwrap(), "3");
        assert_eq!(runtime.execute_console("console_value = 7").unwrap(), "nil");
        assert_eq!(
            runtime.execute_console("return console_value").unwrap(),
            "7"
        );
    }

    #[test]
    fn console_errors_are_recorded_with_console_path() {
        let mut runtime = LuaRuntime::new().unwrap();
        assert!(runtime.execute_console("error('console boom')").is_err());
        let diagnostics = runtime.all_diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, PathBuf::from("<console>"));
        assert_eq!(diagnostics[0].phase, "runtime");
        assert!(diagnostics[0].message.contains("console boom"));
    }

    #[test]
    fn debugger_extracts_location_deduplicates_and_filters() {
        let path = script("function update() error('boom') end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert!(runtime.call_update_debug().is_err());
        assert!(runtime.call_update_debug().is_err());
        assert_eq!(runtime.all_diagnostics().len(), 1);
        assert_eq!(runtime.all_diagnostics()[0].occurrences, 2);
        assert_eq!(runtime.all_diagnostics()[0].phase, "runtime");
        runtime.set_debug_filter(Some("does-not-match".to_string()));
        assert!(runtime.diagnostics().is_empty());
        runtime.set_debug_filter(Some("boom".to_string()));
        assert_eq!(runtime.diagnostics().len(), 1);
        runtime.clear_diagnostics();
        assert!(runtime.all_diagnostics().is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn debugger_extracts_line_from_lua_message() {
        let mut debugger = ScriptDebugger::new();
        debugger.record(PathBuf::from("game.lua"), "runtime", "game.lua:42: boom");
        assert_eq!(debugger.all()[0].line, Some(42));
        assert_eq!(debugger.all()[0].stack_trace, "game.lua:42: boom");
    }

    #[test]
    fn syntax_error_contains_path_and_line() {
        let path = script("function update(\n");
        let mut runtime = LuaRuntime::new().unwrap();
        let error = runtime
            .execute_project_script(&path)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&path.display().to_string()));
        let _ = fs::remove_file(path);
    }
    #[test]
    fn runtime_error_is_returned_without_panic() {
        let path = script("function update() error('boom') end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        let error = runtime.call_update().unwrap_err().to_string();
        assert!(error.contains("boom"));
        assert!(error.contains(&path.display().to_string()));
        let _ = fs::remove_file(path);
    }
}
