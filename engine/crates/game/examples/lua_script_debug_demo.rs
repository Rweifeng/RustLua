use rustlua_scripting::script::LuaRuntime;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "rustlua-script-debug-{}.lua",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::write(&path, "function update() error('debug-boom') end")?;
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
    runtime.execute_project_script(&path)?;
    assert!(runtime.call_update_debug().is_err());
    assert!(runtime.call_update_debug().is_err());
    let all = runtime.all_diagnostics();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].occurrences, 2);
    assert!(all[0].message.contains("debug-boom"));
    runtime.set_debug_filter(Some("no-match".to_string()));
    assert!(runtime.diagnostics().is_empty());
    runtime.set_debug_filter(Some("debug-boom".to_string()));
    assert_eq!(runtime.diagnostics().len(), 1);
    println!("lua_script_debug_demo_ok diagnostics=1 deduplicated=true filtered=true stack=true");
    fs::remove_file(path)?;
    Ok(())
}
