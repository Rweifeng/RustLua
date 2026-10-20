use super::*;

/// `docs/60` §4.3：**重入借用必须从 RefCell panic 降级为 Lua 可捕获错误**。
///
/// 这条不是洁癖：wasm32 上 `panic = abort`，重入触发的是**硬 abort**（浏览器/微信里表现为裸
/// `unreachable`，连行号都拿不到）；而这条路径是**已知可达**的——机制本身就是因为 mlua 会经
/// `__index` 等元方法重入 Rust 才建的。
///
/// 造法：Rust 侧先持住 `ScriptState` 的可变借用（重入的充分条件），再从 Lua 调一个需要借用的
/// API。改写前这句 `s.borrow()` 会 panic；改写后 `lua_borrow()` 的 `try_borrow` 失败，脚本收到
/// 一条带原因的 error。
///
/// **为什么直接调 API、而不是走 `call_update`**：`call_update` 自己会 `self.state.borrow_mut()`
/// 取 `last_update_micros`，持着借用调它只会红在那一层——那样测的是别的东西。这里拿 Lua 侧的函数
/// 直接 call，命中率 100% 落在 API 闭包的借用上。
#[test]
fn reentrant_lua_api_call_surfaces_as_lua_error_instead_of_panic() {
    let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let health: mlua::Table = runtime.lua.globals().get("health").unwrap();
    let get: mlua::Function = health.get("get").unwrap();

    // 持住可变借用：此后任何 `lua_borrow(_mut)` 都必须失败，而不是 panic。
    let held = runtime.state.borrow_mut();
    let outcome = get.call::<mlua::Value>(1_i64);
    drop(held);

    let error = outcome.expect_err("重入必须报错；拿到值说明这条路径根本没在借用 state");
    let message = error.to_string();
    assert!(
        message.contains("already borrowed"),
        "错误必须说清是重入借用（脚本据此才报得出可读原因），实际是：{message}"
    );
}

/// 同一条契约的另一半：**没有重入时必须照常工作**。
/// 少了这一半，"把所有 API 都改成永远报错"也能让上面那条测试变绿。
#[test]
fn non_reentrant_lua_api_call_still_works() {
    let path = script(
        "local e = entity.spawn('probe')\n\
         health.set(e, 7)\n\
         assert(health.get(e) == 7, 'health 往返必须正常')\n\
         assert(entity.find('probe') == e, '非重入路径的名字查找也必须正常')\n\
         function update() return 'ok' end",
    );
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "ok");
    let _ = fs::remove_file(path);
}
