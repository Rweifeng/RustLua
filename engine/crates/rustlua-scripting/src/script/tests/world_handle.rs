//! P1 ⑤-7：`World` 能力单元的契约。
//!
//! 这一刀只换借用面、不搬绑定，所以这里锁的不是"某个 Lua 函数"，而是三条**句柄语义**：
//!
//! 1. 宿主（`with_world`/`with_world_mut`）与 Lua 绑定看到的是**同一个世界**——宿主改的
//!    组件，脚本立刻读得到；脚本 spawn 的实体，宿主的渲染遍历立刻数得到。
//! 2. 世界句柄有名字：重入借用报的是 `world is already borrowed …`，不是笼统的
//!    "RefCell already borrowed"。
//! 3. 整份世界可以**在主句柄上被替换**（场景切换走的就是这条路），替换后旧句柄全部失效。

use super::*;

/// 宿主与脚本共享同一份世界：两个方向都验一次。
#[test]
fn host_and_script_share_one_world_behind_the_handle() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "function update()\n\
         entity.spawn('Shared')\n\
         return 'spawned' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "spawned");
    let _ = fs::remove_file(&path);

    // 脚本 spawn 的实体，宿主看得见（同一个 `World`）。
    let spawned =
        runtime.with_world(|world| world.entities_with::<rustlua_ecs::transform::Transform>());
    assert_eq!(spawned.len(), 1, "宿主应看见脚本 spawn 的实体");

    // 宿主改的位置，脚本读得到。
    let handle = spawned[0];
    runtime.with_world_mut(|world| {
        let transform = world
            .get_mut::<rustlua_ecs::transform::Transform>(handle)
            .unwrap()
            .unwrap();
        transform.position.x = 42.0;
    });
    let path = script(
        "function update()\n\
         local id = entity.find('Shared')\n\
         return tostring(transform.get(id).x) end",
    );
    runtime.execute_project_script(&path).unwrap();
    let x = runtime.call_update().unwrap();
    let _ = fs::remove_file(&path);
    assert_eq!(
        x.parse::<f64>().unwrap(),
        42.0,
        "脚本应读到宿主写进同一份世界的位置"
    );
}

/// 句柄有名：重入借用给的是"世界正被借着"，能直接定位到是哪个能力单元。
#[test]
fn the_world_handle_is_named_in_reentrancy_errors() {
    let runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let message = runtime
        .with_world(|_| {
            // 宿主闭包里再借一次可写——正常路径永远不会这样做，这里刻意制造重入。
            runtime
                .state
                .borrow()
                .world
                .borrow_mut()
                .err()
                .map(|error| error.to_string())
        })
        .expect("世界已被读借用，再借写必须失败");
    assert!(
        message.contains("world") && message.contains("borrowed"),
        "报错必须点名 world：{message}"
    );
}

/// 场景切换（以及任何宿主）可以在主句柄上整份替换世界，旧句柄随之失效。
#[test]
fn replacing_the_world_through_the_handle_invalidates_old_entities() {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).unwrap();
    let path = script(
        "function update()\n\
         entity.spawn('Doomed')\n\
         return 'doomed' end",
    );
    runtime.execute_project_script(&path).unwrap();
    assert_eq!(runtime.call_update().unwrap(), "doomed");
    let _ = fs::remove_file(&path);
    assert_eq!(runtime.with_world(|world| world.entity_count()), 1);

    let handle =
        runtime.with_world(|world| world.entities_with::<rustlua_ecs::transform::Transform>()[0]);

    // 与 `transition.rs` 同一条路径：覆盖句柄里那份世界。
    {
        let state = runtime.state.borrow();
        let mut world = state.world.borrow_mut().unwrap();
        *world = rustlua_ecs::world::World::new();
    }
    assert_eq!(
        runtime.with_world(|world| world.entity_count()),
        0,
        "换世界后旧实体不应还在"
    );
    assert!(
        !runtime.with_world(|world| world.is_alive(handle)),
        "旧句柄在新世界里必须是死的"
    );
}
