//! P1 ⑤-9：物理域能力单元的契约。
//!
//! 这一刀把 `PhysicsState`（两个模拟世界 + 分阶段回写队列 + 调试请求）搬进
//! `rustlua-physics`，绑定随之搬走。这里锁三条**句柄语义**：
//!
//! 1. 宿主（`step_physics`/`take_collider_debug_request`）与 Lua（`physics.*`）看到的是
//!    **同一份**物理状态——脚本登记的调试请求，宿主下一次取得到。
//! 2. 句柄有名字：重入借用报的是 `physics is already borrowed …`，能直接定位能力单元。
//! 3. 场景切换把两个世界与两条回写队列一起换掉（旧实体句柄作废），这是 `transition.rs`
//!    走的那条路径。

use super::*;

/// 脚本登记的调试请求，宿主取得到；且是"取走即清空"。
#[test]
fn host_and_script_share_one_physics_state() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "function update()\n\
         physics.debug_draw(true)\n\
         return 'asked' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "asked");
    let _ = fs::remove_file(&path);

    assert_eq!(
        runtime.take_collider_debug_request(),
        Some(true),
        "宿主应取到脚本登记的请求"
    );
    assert_eq!(
        runtime.take_collider_debug_request(),
        None,
        "取走即清空，不该被消费两次"
    );
}

/// 句柄有名：重入借用给的是"物理正被借着"，能直接定位到是哪个能力单元。
#[test]
fn the_physics_handle_is_named_in_reentrancy_errors() {
    let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let message = runtime
        .state
        .borrow()
        .physics
        .borrow_mut()
        .and_then(|_physics| {
            // 已经持有可写借用，再借一次必须失败（正常路径不会这样，这里刻意制造重入）。
            runtime.state.borrow().physics.borrow_mut().map(|_| ())
        })
        .err()
        .map(|error| error.to_string())
        .expect("物理已被写借用，再借必须失败");
    assert!(
        message.contains("physics") && message.contains("borrowed"),
        "报错必须点名 physics：{message}"
    );
}

/// 切场景把两个世界一起换掉：宿主推进过的一步不该跨场景留到下一张图。
#[test]
fn scene_transition_replaces_both_physics_worlds() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    // 脚本登记请求 + 宿主推进一步，制造"有状态"的物理域。
    let path = script(
        "function update()\n\
         physics.debug_draw(true)\n\
         return 'stepped' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "stepped");
    let _ = fs::remove_file(&path);
    runtime.step_physics(1.0 / 60.0);
    assert_eq!(runtime.take_collider_debug_request(), Some(true));

    // 与 `transition.rs` 同一条路径：在句柄上清场。
    {
        let state = runtime.state.borrow();
        let mut physics = state.physics.borrow_mut().unwrap();
        physics.clear_for_scene_transition();
    }
    assert_eq!(
        runtime.take_collider_debug_request(),
        None,
        "切场景后旧的调试请求不该还在"
    );
    assert!(
        runtime.collider_debug_shapes().is_empty(),
        "新场景的 2D 世界应是空的"
    );
    assert!(
        runtime.collider_debug_shapes_3d().is_empty(),
        "新场景的 3D 世界应是空的"
    );
    // 回写队列也清了：切场景后第一次 writeback 不该写出上一张图的移动。
    assert_eq!(runtime.writeback_physics(), 0);
    assert_eq!(runtime.writeback_physics3d(), 0);
}
