//! 依赖倒置的接缝（`docs/62` P1）：领域 crate 提供的 Lua 模块由**注册表**装进 VM。
//!
//! 这条测试不碰任何具体领域：它证明的是机制本身——注册表里的安装器真的跑在 VM 上、
//! 能捕获能力句柄、能在热重载的新 VM 上重装、重名会当场报错。有了这条，"把 audio
//! 的绑定搬进 `rustlua-audio`"就只是搬家，而不是还要顺手发明一套装配协议。

use super::*;

/// 一个假的能力单元：假装是"领域状态"，验证句柄能从安装器一路传到 Lua。
#[derive(Default)]
struct Counter {
    calls: u32,
}

#[test]
fn registry_installs_domain_modules_into_the_vm() {
    use rustlua_lua_api::{Handle, Registry};

    let counter = Handle::new(Counter::default(), "counter");
    let mut registry = Registry::new();
    registry.add("counter", {
        let counter = counter.clone();
        move |lua| {
            let table = lua.create_table()?;
            let bump = counter.clone();
            table.set(
                "bump",
                lua.create_function(move |_, ()| {
                    let mut state = bump.borrow_mut()?;
                    state.calls += 1;
                    Ok(state.calls)
                })?,
            )?;
            let peek = counter.clone();
            table.set(
                "peek",
                lua.create_function(move |_, ()| Ok(peek.borrow()?.calls))?,
            )?;
            lua.globals().set("counter", table)?;
            Ok(())
        }
    });

    let mut runtime = LuaRuntime::with_installers(Rc::new(registry)).expect("运行时");
    // 1) 安装器真的跑了，而且闭包捕获的句柄与宿主是同一份状态
    runtime
        .execute_project_script(&script(
            "assert(counter.bump() == 1)\nassert(counter.bump() == 2)\nassert(counter.peek() == 2)",
        ))
        .expect("注册表里的模块必须能在脚本里用");
    assert_eq!(counter.borrow().expect("借用").calls, 2);

    // 2) 热重载重建 VM 之后模块还在（注册表跟着运行时走，不是一次性装完就丢）
    runtime
        .execute_project_script(&script("assert(counter.peek() == 2)\ncounter.bump()"))
        .expect("热重载后的新 VM 也要装同一套模块");
    assert_eq!(counter.borrow().expect("借用").calls, 3);

    // 3) 没有注册表的运行时**看不到**这些模块：证明它不是内建，而是外部装的
    let mut plain =
        LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry()).expect("运行时");
    plain
        .execute_project_script(&script("assert(counter == nil)"))
        .expect("空注册表不应有任何领域模块");
}

#[test]
fn duplicate_module_names_are_rejected_loudly() {
    use rustlua_lua_api::Registry;

    let mut registry = Registry::new();
    for _ in 0..2 {
        registry.add("twice", |lua| {
            lua.globals().set("twice", lua.create_table()?)?;
            Ok(())
        });
    }
    let error = LuaRuntime::with_installers(Rc::new(registry))
        .err()
        .expect("同名模块必须被拒绝");
    let text = error.to_string();
    assert!(text.contains("重复登记"), "{text}");
}

#[test]
fn reentrant_handle_borrow_becomes_a_lua_error_not_a_panic() {
    use rustlua_lua_api::Handle;

    let counter = Handle::new(Counter::default(), "counter");
    // 模拟"绑定里又回调了 Lua"：先借住，再从同一条调用链借第二次。
    let held = counter.borrow_mut().expect("第一次借用");
    let error = counter.borrow_mut().err().expect("重入必须报错");
    let text = error.to_string();
    assert!(text.contains("counter"), "报错要点名是哪个单元：{text}");
    assert!(text.contains("already borrowed"), "{text}");
    drop(held);
}
