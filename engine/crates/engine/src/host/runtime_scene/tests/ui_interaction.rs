//! M76-02：交互路由接线门。
//!
//! 用 `ui_test.scene` 的真实实体验证三件事：
//! - hover / pressed / disabled 状态由交互树驱动（渲染侧按名字取状态贴图）；
//! - click 由抬起触发（按下/抬起同一命中且未拖拽），经 `process_ui_event` 派发到 Lua；
//! - 交互路由默认开启（M80-01），显式关闭后回到旧路径且不再派发 pointer_enter。

use super::*;
use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_ui::ui_states::ButtonVisualState;

fn ui_test_runtime() -> RuntimeScene {
    let root = game_root();
    let scene = Scene::load(root.join("scenes/ui_test.scene")).unwrap();
    RuntimeScene::from_scene(Some(&root), scene).unwrap()
}

fn target_center(runtime: &mut RuntimeScene, name: &str) -> (f64, f64) {
    let targets = runtime.compute_click_targets();
    let target = targets
        .iter()
        .find(|target| target.name == name)
        .unwrap_or_else(|| panic!("{name} 应在命中目标里: {targets:#?}"));
    (
        f64::from(target.origin[0] + target.size[0] / 2.0),
        f64::from(target.origin[1] + target.size[1] / 2.0),
    )
}

fn cursor_at(point: (f64, f64)) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(point.0, point.1);
    input
}

fn press_at(point: (f64, f64)) -> InputSnapshot {
    let mut input = cursor_at(point);
    input.mouse_event(MouseButton::Left, true);
    input
}

fn release_at(point: (f64, f64)) -> InputSnapshot {
    // 快照的边沿是"状态变化"：先按下、把按下边沿清掉，再抬起才有 just_released。
    let mut input = cursor_at(point);
    input.mouse_event(MouseButton::Left, true);
    input.finish_frame();
    input.mouse_event(MouseButton::Left, false);
    input
}

fn hit_center(runtime: &mut RuntimeScene, name: &str) -> (f64, f64) {
    let data = runtime.ui_hit_data();
    let item = data
        .iter()
        .find(|item| item.name == name)
        .unwrap_or_else(|| panic!("{name} 应在命中数据里"));
    (
        f64::from(item.rect.0[0] + item.rect.1[0] / 2.0),
        f64::from(item.rect.0[1] + item.rect.1[1] / 2.0),
    )
}

#[test]
fn interaction_routes_hover_press_and_click() {
    let mut runtime = ui_test_runtime();
    runtime.set_ui_interaction_enabled(true);
    let point = target_center(&mut runtime, "StartButton");
    let script = write_guard_script(
        "m76-hover",
        "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &cursor_at(point))
        .unwrap();
    assert_eq!(
        runtime.ui_visual_state("StartButton"),
        Some(ButtonVisualState::Hover),
        "指针进入按钮应进入 hover 态"
    );

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(point))
        .unwrap();
    assert_eq!(
        runtime.ui_visual_state("StartButton"),
        Some(ButtonVisualState::Pressed),
        "按下应进入 pressed 态"
    );
    assert!(
        !runtime
            .script
            .call_update()
            .unwrap()
            .contains("StartButton"),
        "按下阶段不得派发 click，必须等抬起"
    );

    runtime
        .run_script_steps(1, 1.0 / 60.0, &release_at(point))
        .unwrap();
    assert!(
        runtime
            .script
            .call_update()
            .unwrap()
            .contains("StartButton"),
        "抬起在同一目标上松手，on_click 必须收到实体名"
    );
    assert_eq!(
        runtime.ui_visual_state("StartButton"),
        Some(ButtonVisualState::Hover),
        "抬起后回到 hover 态"
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn press_then_drag_off_target_does_not_click() {
    let mut runtime = ui_test_runtime();
    runtime.set_ui_interaction_enabled(true);
    let point = target_center(&mut runtime, "StartButton");
    let script = write_guard_script(
        "m80-release-click",
        "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(point))
        .unwrap();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release_at((5.0, 5.0)))
        .unwrap();
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "nil",
        "按下后拖出目标再松手不得触发 click"
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn interaction_marks_disabled_buttons() {
    let mut runtime = ui_test_runtime();
    runtime.set_ui_interaction_enabled(true);
    let point = hit_center(&mut runtime, "DisabledButton");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &cursor_at(point))
        .unwrap();
    assert_eq!(
        runtime.ui_visual_state("DisabledButton"),
        Some(ButtonVisualState::Disabled),
        "ButtonStates.disabled_state=true 必须在运行时生效"
    );
}

#[test]
fn pointer_events_dispatch_by_default_and_opt_out_restores_legacy() {
    let mut runtime = ui_test_runtime();
    let point = target_center(&mut runtime, "PointerButton");
    let script = write_guard_script(
        "m76-optout",
        "function on_click(name) calls = (calls or 0) + 1 end\nfunction update() return tostring(calls or 0) end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &cursor_at(point))
        .unwrap();
    let calls: usize = runtime.script.call_update().unwrap().parse().unwrap();
    assert_eq!(calls, 1, "默认开启：hover 必须派发 pointer_enter");

    runtime.set_ui_interaction_enabled(false);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &cursor_at(point))
        .unwrap();
    assert_eq!(
        runtime.script.call_update().unwrap(),
        calls.to_string(),
        "显式回滚后旧路径不得再派发 pointer_enter"
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn direction_keys_and_gamepad_confirm_drive_focus_navigation() {
    let mut runtime = ui_test_runtime();
    let start = target_center(&mut runtime, "StartButton");
    let script = write_guard_script(
        "m80-nav",
        "function on_click(name) last = name end\nfunction update() return tostring(last or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(start))
        .unwrap();
    assert_eq!(runtime.ui_focused_name(), Some("StartButton"));

    let mut down = cursor_at(start);
    down.key_event(KeyCode::ArrowDown, true);
    runtime.run_script_steps(1, 1.0 / 60.0, &down).unwrap();
    assert_eq!(
        runtime.ui_focused_name(),
        Some("ResetButton"),
        "方向键沿同一列把焦点移到下一个按钮"
    );

    let mut pad = cursor_at(start);
    pad.gamepad_button_event(0, true);
    runtime.run_script_steps(1, 1.0 / 60.0, &pad).unwrap();
    assert!(
        runtime
            .script
            .call_update()
            .unwrap()
            .contains("ResetButton"),
        "手柄 confirm 应激活当前焦点按钮"
    );
    let _ = std::fs::remove_file(script);
}
