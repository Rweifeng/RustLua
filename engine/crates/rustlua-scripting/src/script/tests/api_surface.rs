use super::*;

#[test]
fn registers_log_time_input_and_quit_api() {
    // `input` 绑定住在 `rustlua-input`（`docs/62` P1 阶段 3）：宿主句柄与注册表一起拿，
    // 本 crate 只在测试里经 dev-dependency 造快照。
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let mut runtime = LuaRuntime::with_installers(installers).unwrap();
    runtime.set_delta(0.25);
    let mut input = rustlua_input::input::InputSnapshot::default();
    input.key_event(rustlua_input::input::KeyCode::KeyD, true);
    handles.input.set_input(input);
    let path = script(
        "assert(time.delta() == 0.25)\nassert(input.is_down('d'))\nassert(input.axis('horizontal') == 1.0)\nfunction update() return 'ok' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    runtime.lua.load("app.quit()").exec().unwrap();
    assert!(runtime.quit_requested());
    let _ = fs::remove_file(path);
}

/// `app.project_root()`：工程根的绝对路径（正斜杠），未配置时 `""`。
///
/// 存在的理由：`io.open` 的相对路径按**进程工作目录**解析，而 `game --check` 并不
/// chdir 到工程根（在 `engine/` 下跑 `--check --project ../examples/game_jhyt` 时
/// `io.open("scenes/x.scene")` 返回 nil）⇒ 跨文件契约校验器会静默跳过检查。
#[test]
fn app_exposes_the_project_root_for_cwd_independent_file_checks() {
    let root = production_sample_root();
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.set_project_root(root.clone());
    let reported = runtime
        .execute_console("return app.project_root()")
        .unwrap();
    let reported = reported.trim();
    assert!(!reported.is_empty(), "配置了工程根就不该返回空串");
    assert!(
        !reported.contains('\\'),
        "路径分隔符必须归一成正斜杠（Lua 模式里反斜杠是转义符）：{reported}"
    );
    let expected = root.to_string_lossy().replace('\\', "/");
    // 盘符大小写在不同来源可能不同，逐段比对路径尾部即可。
    assert!(
        reported.ends_with(&expected) || expected.ends_with(reported),
        "project_root() = {reported:?}，与 set_project_root 的值对不上（{expected:?}）"
    );
    // 没配置时不报错，只是空串（Web 宿主没有文件系统）。
    let bare = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(
        bare.lua
            .load("return app.project_root()")
            .eval::<String>()
            .unwrap(),
        ""
    );
}

#[test]
fn call_update_without_script_idles_instead_of_erroring() {
    // Engine-only diagnostic mode has no project script; update must be a
    // no-op rather than a fatal "nil to function" error (which used to
    // crash double-clicked runs).
    let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "");
}

#[test]
fn invalid_entity_is_a_lua_error() {
    let path = script("transform.get(4294967297)");
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let error = runtime
        .execute_project_script(&path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid entity id"));
    let _ = fs::remove_file(path);
}
/// M19-03：`eval_json` 覆盖表/数组/标量/nil 的转换与非法表达式错误，
/// 且错误保留原始 Lua 消息（供编辑器原样显示）。
#[test]
fn eval_json_converts_tables_arrays_scalars_and_reports_errors() {
    let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    assert_eq!(runtime.eval_json("nil").unwrap(), serde_json::Value::Null);
    assert_eq!(runtime.eval_json("true").unwrap(), serde_json::json!(true));
    assert_eq!(runtime.eval_json("42").unwrap(), serde_json::json!(42));
    assert_eq!(runtime.eval_json("1.5").unwrap(), serde_json::json!(1.5));
    assert_eq!(
        runtime.eval_json("'text'").unwrap(),
        serde_json::json!("text")
    );
    // 纯数组表 → JSON 数组。
    assert_eq!(
        runtime.eval_json("return { 1, 2, 3 }").unwrap(),
        serde_json::json!([1, 2, 3])
    );
    // 字符串键表（含嵌套数组）→ JSON 对象。
    assert_eq!(
        runtime
            .eval_json("return { ok = true, issues = { 'a', 'b' } }")
            .unwrap(),
        serde_json::json!({ "ok": true, "issues": ["a", "b"] })
    );
    // 多语句块（编辑器域数据探针用 IIFE，但语句块本身也应可用）。
    assert_eq!(
        runtime
            .eval_json("local out = {}; for i = 1, 3 do out[i] = i * 2 end; return out")
            .unwrap(),
        serde_json::json!([2, 4, 6])
    );
    // 非法表达式：错误保留原始 Lua 语法消息。
    let error = runtime.eval_json("this is not lua").unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("eval_json"),
        "错误应带求值入口名: {message}"
    );
    assert!(
        message.contains("syntax error"),
        "错误应保留原始 Lua 消息: {message}"
    );
    // 运行期错误同样保留消息。
    let runtime_error = runtime.eval_json("error('probe boom')").unwrap_err();
    assert!(runtime_error.to_string().contains("probe boom"));
    // 不支持的 Lua 类型（函数）报错而不是 panic。
    assert!(runtime.eval_json("print").is_err());
}
