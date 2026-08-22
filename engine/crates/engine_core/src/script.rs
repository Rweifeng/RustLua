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

use crate::{
    animation::AnimationStateMachine,
    audio::AudioSystem,
    entity::EntityId,
    input::{InputKey, InputSnapshot, MouseButton},
    particle::{Particle, ParticleSystem},
    physics::{ColliderDesc, ColliderShape, PhysicsWorld},
    save::{decode as decode_save, encode as encode_save},
    scene::Scene,
    sprite::{Sprite, SpriteAsset},
    tilemap::Tilemap,
    transform::{Transform, Vec2},
    world::{World, WorldError},
};

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
    particles: ParticleSystem,
    audio: AudioSystem,
}

#[derive(Debug, Clone)]
pub struct UiText {
    pub text: String,
    pub visible: bool,
    pub position: [f32; 2],
    /// Screen-space scale of the built-in glyph atlas (1.0 = 5x7 px glyphs).
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
        let mut state = self.state.borrow_mut();
        let mut count = 0;
        for entity in scene.entities.values() {
            let id = state.world.spawn();
            if let Some(value) = entity.components.get("Transform") {
                let transform = Transform::from_component_str(value).map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "entity {:?} has an invalid Transform: {error}",
                        entity.name
                    ))
                })?;
                state
                    .world
                    .insert(id, transform)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
            if let Some(raw) = entity.components.get("Sprite") {
                let (path, size) = parse_sprite_ref(raw);
                let sprite =
                    Sprite::new(size.unwrap_or(Vec2::new(128.0, 128.0))).map_err(|error| {
                        LuaBridgeError::Api(format!("failed to build default Sprite: {error}"))
                    })?;
                state
                    .world
                    .insert(id, sprite)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
                state
                    .world
                    .insert(id, SpriteAsset(path))
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
            if let Some(value) = entity.components.get("Tilemap") {
                let tilemap = Tilemap::from_component_str(value).map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "entity {:?} has an invalid Tilemap: {error}",
                        entity.name
                    ))
                })?;
                state
                    .world
                    .insert(id, tilemap)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
            if let Some(value) = entity.components.get("Animation") {
                let animation =
                    AnimationStateMachine::from_component_str(value).map_err(|error| {
                        LuaBridgeError::Api(format!(
                            "entity {:?} has an invalid Animation: {error}",
                            entity.name
                        ))
                    })?;
                state
                    .world
                    .insert(id, animation)
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
            if !entity.name.is_empty() {
                state.names.insert(entity.name.clone(), id);
            }
            let position = state
                .world
                .get::<Transform>(id)
                .ok()
                .flatten()
                .map(|transform| transform.position)
                .unwrap_or_default();
            if let Some(value) = entity.components.get("Collider") {
                let collider = ColliderDesc::from_component_str(value).map_err(|error| {
                    LuaBridgeError::Api(format!(
                        "entity {:?} has an invalid Collider: {error}",
                        entity.name
                    ))
                })?;
                state
                    .physics
                    .add_body(id, position, true, vec![collider])
                    .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
            }
            if entity.components.contains_key("Tilemap") {
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
            count += 1;
        }
        Ok(count)
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
            state.particles = ParticleSystem::new();
            state.names.clear();
            state.health.clear();
            state.ui.clear();
            state.ui_bars.clear();
            state.scene_pending_load = None;
            state.scene = Some(name.clone());
        }
        let instantiated = self.instantiate_scene(&scene)?;
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

    pub fn execute_project_script(&mut self, path: &Path) -> Result<(), LuaBridgeError> {
        let source = fs::read_to_string(path).map_err(|error| LuaBridgeError::Load {
            path: path.to_path_buf(),
            source: error.to_string(),
        })?;
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
        let source = fs::read_to_string(&path).map_err(|error| LuaBridgeError::Load {
            path: path.clone(),
            source: error.to_string(),
        })?;
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
        let source = fs::read_to_string(&path).map_err(|error| LuaBridgeError::Load {
            path: path.clone(),
            source: error.to_string(),
        })?;
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
        candidate
            .load(source)
            .set_name(path.to_string_lossy())
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

/// Splits a `Sprite` component value into its texture path and an optional
/// `;size=<n>` or `;size=<w>x<h>` override. Defaults to the engine's 128px
/// sprite when no size is given.
fn parse_sprite_ref(value: &str) -> (String, Option<Vec2>) {
    let (path, params) = match value.split_once(';') {
        Some((path, params)) => (path, Some(params)),
        None => (value, None),
    };
    let size = params
        .and_then(|params| {
            params
                .split(';')
                .find_map(|field| field.strip_prefix("size="))
        })
        .and_then(|raw| {
            if let Some((w, h)) = raw.split_once('x') {
                Some((w.parse().ok()?, h.parse().ok()?))
            } else {
                let value: f32 = raw.parse().ok()?;
                Some((value, value))
            }
        })
        .map(|(width, height)| Vec2::new(width, height));
    (path.to_owned(), size)
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
            Ok(true)
        })?,
    )?;
    lua.globals().set("entity", entity)?;

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
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_at",
        lua.create_function(
            move |_, (id, x, y, text, scale): (String, f32, f32, String, Option<f32>)| {
                s.borrow_mut().ui.insert(
                    id,
                    UiText {
                        text,
                        visible: true,
                        position: [x, y],
                        scale: scale.unwrap_or(1.0),
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
        use crate::scene::{Scene, SceneEntityId};

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
            .insert("Sprite".into(), "assets/red-1x1.png".into());

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
        assert_eq!(asset.path(), "assets/red-1x1.png");
        let moved = runtime.with_world(|world| *world.get::<Transform>(player).unwrap().unwrap());
        assert_eq!(moved.position, Vec2::new(10.0, 5.0));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wasd_input_axis_moves_the_instantiated_player() {
        use crate::scene::{Scene, SceneEntityId};

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
        use crate::scene::{Scene, SceneEntityId};

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
        use crate::scene::{Scene, SceneEntityId};

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
        use crate::scene::{Scene, SceneEntityId};

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
    fn ui_text_at_accepts_optional_scale() {
        let mut runtime = LuaRuntime::new().unwrap();
        let path = script(
            "ui.text_at('small', 10, 10, 'a')\nui.text_at('big', 10, 30, 'b', 3)\nfunction update() return 'ok' end",
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
            texts.get("big").expect("big text stored").scale,
            3.0,
            "optional scale argument is honored"
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
    fn save_commit_load_and_scene_transition() {
        use crate::scene::{Scene, SceneEntityId};

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
