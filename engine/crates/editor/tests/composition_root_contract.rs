//! 跨 crate 契约门：编辑器真的能驱动组合根交出的运行时。
//!
//! ## 为什么需要这条门
//!
//! `crates/editor` 有 792 条单元测试，但绝大多数守的是编辑器自身的 UI 逻辑（面板布局、
//! 命中测试、撤销栈）。它们与引擎之间靠的是 `engine::host::script_packs::runtime(...)`
//! 这一个入口——而"入口交出来的东西能不能真的用"没有门看着。
//!
//! 这类失败特别隐蔽：组合根改了签名、某个领域的绑定搬走、`ScriptDomainHandles` 少一个
//! 字段，编辑器侧的单元测试**全部照旧通过**（它们用桩件），只有真正按 Play 或打开
//! runtime 调试面板时才炸。这条门用**真实组合根 + 真实 VM** 走一遍编辑器的实际调用形态。
//!
//! ## 为什么不用 GPU
//!
//! 这里刻意只碰与渲染设备无关的部分（VM、绑定表、句柄、脚本执行），因此在 CI 的无 GPU
//! 环境也能跑——它要守的是**跨 crate 的类型与生命周期契约**，不是像素。

use engine::host::script_packs;

/// 编辑器的 `ai_host` / `domain_data` / `runtime_debug` 都用这一行建运行时。
///
/// 断言它交出的东西**可用**：VM 活着、领域句柄齐全、绑定表真的装好了——而不是只"返回
/// 了 Ok"。三件事分别对应三类真实故障：VM 构造失败、句柄缺失、绑定漏装。
#[test]
fn composition_root_runtime_is_usable_by_the_editor() {
    let (runtime, handles) = script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("组合根应能交出运行时（编辑器的 ai_host/domain_data/runtime_debug 全靠它）");

    // ① 领域句柄是**同一份**能力单元：编辑器把它们与 VM 一起持有时读写的是同一状态。
    //    句柄能 borrow 出来就说明没有被别的持有者卡住（这是编辑器最常见的一类死锁）。
    handles.camera.with(|_| ());
    handles.animation.with(|_| ());
    handles.physics.with(|_| ());
    handles.ui.with(|_| ());

    // ② 绑定表装好了：编辑器的 runtime 调试面板正是靠这些全局表做探测。
    //    只查"表存在"不够——子键也必须真的在（热重载路径曾漏装 `ui.image`）。
    let probes = [
        ("camera.set_active", "camera.set_active ~= nil"),
        ("camera.get_active", "camera.get_active ~= nil"),
        ("animation.play", "animation.play ~= nil"),
        ("ui.text", "ui.text ~= nil"),
        ("ui.text_get", "ui.text_get ~= nil"),
        ("ui.view_model_set", "ui.view_model_set ~= nil"),
        ("ui.scene_text_visible", "ui.scene_text_visible ~= nil"),
        ("world_text.attach", "world_text.attach ~= nil"),
        ("prefab.spawn", "prefab.spawn ~= nil"),
        ("interact.prompt", "interact.prompt ~= nil"),
    ];
    // 下面要按探测表达式逐个求值，需要可变的 VM 句柄。
    for (label, expression) in probes {
        let value = runtime
            .eval_json(&format!("return {expression}"))
            .unwrap_or_else(|error| panic!("探测 {label} 失败：{error}"));
        assert_eq!(
            value.as_bool(),
            Some(true),
            "组合根交出的运行时缺少 {label}（表达式 `{expression}`）——编辑器的探测面板会读到 nil"
        );
    }
}

/// 编辑器会在**同一个进程里建多个运行时**（Play 预览、AI 宿主、数据校验各一份）。
///
/// 这份契约要求两次调用互不干扰：各自的领域句柄独立、各自写的 UI 文字不串。
/// 组合根的句柄是每次 `install` 新建的，但"新建"这件事没人钉住——一旦有人把句柄改成
/// 进程级单例，两个预览窗口就会互相看到对方的 UI，而单元测试发现不了。
#[test]
fn two_editor_runtimes_do_not_share_domain_state() {
    let (mut first, first_handles) =
        script_packs::runtime(rustlua_gameplay::input_lua::registry()).expect("第一个运行时");
    let (mut second, second_handles) =
        script_packs::runtime(rustlua_gameplay::input_lua::registry()).expect("第二个运行时");

    first
        .execute_console("ui.text('probe', 'first-window')")
        .expect("第一个运行时应能写 UI 文字");
    second
        .execute_console("ui.text('probe', 'second-window')")
        .expect("第二个运行时应能写 UI 文字");

    let first_text = first_handles
        .ui
        .with(|ui| ui.texts().get("probe").map(|text| text.text.clone()));
    let second_text = second_handles
        .ui
        .with(|ui| ui.texts().get("probe").map(|text| text.text.clone()));

    assert_eq!(
        first_text.as_deref(),
        Some("first-window"),
        "第一个运行时读到了别人的 UI 文字：领域句柄被共享了"
    );
    assert_eq!(
        second_text.as_deref(),
        Some("second-window"),
        "第二个运行时读到了别人的 UI 文字：领域句柄被共享了"
    );
}
