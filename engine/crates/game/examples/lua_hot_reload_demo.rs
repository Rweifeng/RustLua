use rustlua_scripting::script::{LuaRuntime, ReloadStatus};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "rustlua-hot-reload-{}.lua",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::write(&path, "function update() return 'v1' end")?;
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.execute_project_script(&path)?;
    assert_eq!(runtime.call_update()?, "v1");
    assert_eq!(runtime.reload_if_changed()?, ReloadStatus::Unchanged);
    fs::write(&path, "function update() return 'v2' end")?;
    assert_eq!(runtime.reload_if_changed()?, ReloadStatus::Reloaded);
    assert_eq!(runtime.call_update()?, "v2");
    fs::write(&path, "function update(")?;
    assert!(runtime.reload_if_changed().is_err());
    assert_eq!(runtime.call_update()?, "v2");
    println!("lua_hot_reload_demo_ok unchanged=true reloaded=true rollback=true update=v2");
    fs::remove_file(path)?;
    Ok(())
}
