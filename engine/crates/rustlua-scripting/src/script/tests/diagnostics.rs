use super::*;

#[test]
fn console_evaluates_values_and_updates_lua_state() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.execute_console("return 1 + 2").unwrap(), "3");
    assert_eq!(runtime.execute_console("console_value = 7").unwrap(), "nil");
    assert_eq!(
        runtime.execute_console("return console_value").unwrap(),
        "7"
    );
}

#[test]
fn console_errors_are_recorded_with_console_path() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
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
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
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
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
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
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&path).unwrap();
    let error = runtime.call_update().unwrap_err().to_string();
    assert!(error.contains("boom"));
    assert!(error.contains(&path.display().to_string()));
    let _ = fs::remove_file(path);
}
