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
    entity::EntityId,
    input::{InputKey, InputSnapshot, MouseButton},
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
    assets: BTreeMap<String, String>,
    ui: BTreeMap<String, UiText>,
    save_data: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct UiText {
    text: String,
    visible: bool,
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
        let update: Function = self
            .lua
            .globals()
            .get("update")
            .map_err(|error| LuaBridgeError::Api(error.to_string()))?;
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
    lua.globals().set("physics", physics)?;

    let scene = lua.create_table()?;
    let s = Rc::clone(&state);
    scene.set(
        "load",
        lua.create_function(move |_, name: String| {
            s.borrow_mut().scene = Some(name);
            Ok(())
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
                },
            );
            Ok(())
        })?,
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
