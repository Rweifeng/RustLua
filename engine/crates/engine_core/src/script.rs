use std::{
    cell::RefCell,
    fmt, fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use mlua::{Function, Lua, Result as LuaResult};
use tracing::info;

use crate::input::{InputKey, InputSnapshot, MouseButton};

#[derive(Debug, Default)]
struct ScriptState {
    delta_seconds: f32,
    quit_requested: bool,
    input: InputSnapshot,
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

/// Main-thread Lua runtime for project scripts.
pub struct LuaRuntime {
    lua: Lua,
    state: Rc<RefCell<ScriptState>>,
    current_path: Option<PathBuf>,
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
        self.lua
            .load(&source)
            .set_name(path.to_string_lossy())
            .exec()
            .map_err(|error| LuaBridgeError::Load {
                path: path.to_path_buf(),
                source: error.to_string(),
            })?;
        self.current_path = Some(path.to_path_buf());
        Ok(())
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

    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.state.borrow().quit_requested
    }
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
    lua.globals().set("log", log)?;

    let time = lua.create_table()?;
    let time_state = Rc::clone(&state);
    time.set(
        "delta",
        lua.create_function(move |_, ()| Ok(time_state.borrow().delta_seconds))?,
    )?;
    lua.globals().set("time", time)?;

    let input = lua.create_table()?;
    let input_state = Rc::clone(&state);
    input.set(
        "is_down",
        lua.create_function(move |_, key: String| {
            Ok(input_state.borrow().input.is_down(parse_key(&key)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "just_pressed",
        lua.create_function(move |_, key: String| {
            Ok(input_state.borrow().input.just_pressed(parse_key(&key)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "just_released",
        lua.create_function(move |_, key: String| {
            Ok(input_state.borrow().input.just_released(parse_key(&key)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "axis",
        lua.create_function(move |_, axis: String| Ok(input_state.borrow().input.axis(&axis)))?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "mouse_is_down",
        lua.create_function(move |_, button: String| {
            Ok(input_state
                .borrow()
                .input
                .mouse_is_down(parse_mouse_button(&button)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "mouse_just_pressed",
        lua.create_function(move |_, button: String| {
            Ok(input_state
                .borrow()
                .input
                .mouse_just_pressed(parse_mouse_button(&button)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "mouse_just_released",
        lua.create_function(move |_, button: String| {
            Ok(input_state
                .borrow()
                .input
                .mouse_just_released(parse_mouse_button(&button)))
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "cursor_position",
        lua.create_function(move |lua, ()| {
            let (x, y) = input_state.borrow().input.cursor_position();
            let values = lua.create_table()?;
            values.set(1, x)?;
            values.set(2, y)?;
            Ok(values)
        })?,
    )?;
    let input_state = Rc::clone(&state);
    input.set(
        "cursor_delta",
        lua.create_function(move |lua, ()| {
            let (x, y) = input_state.borrow().input.cursor_delta();
            let values = lua.create_table()?;
            values.set(1, x)?;
            values.set(2, y)?;
            Ok(values)
        })?,
    )?;
    lua.globals().set("input", input)?;

    let app = lua.create_table()?;
    let app_state = state;
    app.set(
        "quit",
        lua.create_function(move |_, ()| {
            app_state.borrow_mut().quit_requested = true;
            Ok(())
        })?,
    )?;
    lua.globals().set("app", app)?;
    Ok(())
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
    fn rust_can_call_lua_function() {
        let path = script("function update() return 'lua-update-ok' end");
        let mut runtime = LuaRuntime::new().unwrap();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "lua-update-ok");
        let _ = fs::remove_file(path);
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
        assert!(error.contains("line") || error.contains(":1") || error.contains("<name>"));
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
