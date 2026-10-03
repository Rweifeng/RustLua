//! 原 `mod sdf_tests` 里 `script_policy` 这一域（WP12 阶段 1 纯搬运；源行段 5354–5591）。
//!
//! - `example_project_runs_with_new_api`
//! - `script_update_time_is_measured_and_taken_once`
//! - `ui_test_scene_renders_ui_and_dispatches_clicks`
//! - `contained_script_error_does_not_abort_the_fixed_step`
//! - `strict_policy_still_fails_the_step_on_script_error`
//! - `repeated_contained_errors_disable_update`
//! - `contained_script_error_leaves_engine_systems_running`

use super::*;

#[test]
fn example_project_runs_with_new_api() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
    runtime
        .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
        .unwrap();
}

/// P4：`lua_ms` 指标真的被填了。此前 `begin_lua/end_lua` 只有单测在调，
/// 真实运行的 `[perf] lua_ms=` 恒为 `0.000`——一条看着在工作、其实永远为零的指标。
#[test]
fn script_update_time_is_measured_and_taken_once() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
    runtime
        .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
        .unwrap();
    let micros = runtime.take_script_micros();
    assert!(micros > 0, "Lua update 耗时必须被测量到，得到 {micros}µs");
    // 取走即清零：0 步帧不该重复计入上一帧的耗时。
    assert_eq!(runtime.take_script_micros(), 0);
}

/// ui_test 示例场景：渲染出 UI/世界 draw、UiEvents 目标存在，且点击
/// StartButton 会经注入的 on_click 分发（update 返回带实体名）。
#[test]
fn ui_test_scene_renders_ui_and_dispatches_clicks() {
    use crate::core::input::MouseButton;

    let root = crate::core::games_dir().join("examples").join("game_api");
    let path = root.join("scenes/ui_test.scene");
    assert!(path.is_file(), "ui_test 示例场景缺失: {}", path.display());
    let scene = Scene::load(&path).unwrap();
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
    let mesh = runtime.vertices(1280, 720).expect("vertices");
    assert!(!mesh.draws.is_empty(), "ui_test 应渲染出可见 draw");
    let targets = runtime.compute_click_targets();
    assert!(
        targets.len() >= 2,
        "ui_test 应有多个 UiEvents 目标: {targets:#?}"
    );
    let target = targets
        .iter()
        .find(|target| target.name == "StartButton")
        .expect("StartButton UiEvents 目标");
    let script_path =
        std::env::temp_dir().join(format!("rustlua-ui-test-click-{}.lua", std::process::id()));
    std::fs::write(
            &script_path,
            "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
        )
        .unwrap();
    runtime.script.execute_project_script(&script_path).unwrap();
    let cx = target.origin[0] + target.size[0] / 2.0;
    let cy = target.origin[1] + target.size[1] / 2.0;
    let mut input = InputSnapshot::default();
    input.cursor_moved(cx as f64, cy as f64);
    input.mouse_event(MouseButton::Left, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &input)
        .expect("script step");
    let result = runtime.script.call_update().unwrap();
    assert!(
        result.contains("StartButton"),
        "点击应分发 on_click('StartButton'): {result}"
    );
    let _ = std::fs::remove_file(script_path);
}

/// P0 回归：**容错档**下 `update()` 的 Lua 错误不得中断固定步，更不得终止进程。
///
/// 这就是本次修复针对的那条链路：错误 ⇒ `SystemError` ⇒ `SchedulerError::SystemFailed`
/// ⇒ 调度器放弃本步剩余系统 ⇒ 宿主 `fail()` 退出。容错后本步照跑，错误进诊断与计数。
#[test]
fn contained_script_error_does_not_abort_the_fixed_step() {
    let scene = crate::core::scene::Scene::new("guard-contain");
    let mut runtime = RuntimeScene::from_scene(None, scene).unwrap();
    runtime.set_script_error_policy(ScriptErrorPolicy::Contain);
    let path = write_guard_script("guard-contain", "function update() error('guard boom') end");
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("容错档下脚本错误不得让固定步返回 Err");

    assert_eq!(
        runtime.script_error_total(),
        1,
        "错误必须被记账（容错 ≠ 静默）"
    );
    let diagnostics = runtime.script_diagnostics();
    assert!(
        diagnostics
            .iter()
            .any(|item| item.message.contains("guard boom")),
        "被容错的错误必须进结构化诊断: {diagnostics:?}"
    );
    assert!(
        runtime.script_disabled_callbacks().is_empty(),
        "首次错误不该停用回调"
    );
    let _ = std::fs::remove_file(path);
}

/// 默认（严格）档的语义**一字未改**：脚本报错 ⇒ `run_script_steps` 返回 `Err`。
///
/// 26 个无头样例、soak 与性能基线都 `unwrap()` 这个结果，因此验收路径"脚本一错就判红"
/// 不受容错能力影响。
#[test]
fn strict_policy_still_fails_the_step_on_script_error() {
    let scene = crate::core::scene::Scene::new("guard-strict");
    let mut runtime = RuntimeScene::from_scene(None, scene).unwrap();
    assert_eq!(
        runtime.script_error_policy(),
        ScriptErrorPolicy::Strict,
        "默认必须是严格档"
    );
    let path = write_guard_script("guard-strict", "function update() error('strict boom') end");
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();

    let error = runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect_err("严格档必须把脚本错误上报给宿主");
    assert!(
        error.to_string().contains("strict boom"),
        "错误文本要保留 Lua 的原始信息: {error}"
    );
    assert_eq!(runtime.script_error_total(), 0, "严格档不经容错记账");
    let _ = std::fs::remove_file(path);
}

/// 每帧都炸的回调会被停用并**从此不再进 Lua**：既止损，也让日志不被同一个错误淹没。
#[test]
fn repeated_contained_errors_disable_update() {
    let scene = crate::core::scene::Scene::new("guard-disable");
    let mut runtime = RuntimeScene::from_scene(None, scene).unwrap();
    runtime.set_script_error_policy(ScriptErrorPolicy::Contain);
    runtime.with_script(|script| script.set_callback_error_limit(2));
    let path = write_guard_script(
        "guard-disable",
        "function update() error('always boom') end",
    );
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();

    let input = InputSnapshot::default();
    runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
    runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
    assert_eq!(runtime.script_error_total(), 2);
    assert_eq!(
        runtime.script_disabled_callbacks(),
        vec!["update()".to_owned()],
        "连续两次错误（上限 2）应停用 update()"
    );

    // 停用后不再进 Lua：计数不再增长，而固定步仍然是 Ok。
    runtime.run_script_steps(1, 1.0 / 60.0, &input).unwrap();
    assert_eq!(
        runtime.script_error_total(),
        2,
        "停用后不应再有新的错误记账"
    );
    let _ = std::fs::remove_file(path);
}

/// 容错是**按回调**的：工程级 `update()` 报错不该影响其它系统的推进。
/// 用粒子推进做观察点——它是与脚本无关、且每步都动的内建系统。
#[test]
fn contained_script_error_leaves_engine_systems_running() {
    let mut scene = Scene::new("guard-systems");
    scene
        .add_entity(crate::core::scene::SceneEntityId(1), "Tick", None)
        .unwrap();
    scene
        .entities
        .get_mut(&crate::core::scene::SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Transform".to_owned(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
        );
    let mut runtime = RuntimeScene::from_scene(None, scene).unwrap();
    runtime.set_script_error_policy(ScriptErrorPolicy::Contain);
    let path = write_guard_script(
        "guard-systems",
        "function update() error('systems boom') end",
    );
    runtime
        .with_script(|script| script.execute_project_script(&path))
        .unwrap();

    let input = InputSnapshot::default();
    for _ in 0..3 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("固定步应完整跑完");
    }
    // 步进统计是"本步真的跑过各阶段"的证据：脚本报错时它仍然有记录。
    let report = runtime
        .last_schedule_report()
        .expect("每一步都应留下调度统计");
    assert_eq!(report.steps, 1, "固定步不应被跳过");
    assert_eq!(runtime.script_error_total(), 3, "三次报错三次记账");
    let _ = std::fs::remove_file(path);
}
