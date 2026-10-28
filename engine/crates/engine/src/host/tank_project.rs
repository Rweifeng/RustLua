//! TankBattle 外部工程（默认 `E:\Qoder\TankBattle`）的活体验证。
//!
//! 这是"用引擎复刻一个完整小游戏"的工程级门：场景能实例化、Lua 能跑、
//! 行为自检全绿、真实输入快照能驱动玩家移动与开火。
//!
//! 只有设置 `RUSTLUA_TANK_PROJECT=<工程根>` 时才运行——工程不在仓库里，
//! CI 机器没有它时直接跳过，不制造假红。

use std::path::{Path, PathBuf};

use super::RuntimeScene;
use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scene::scene::Scene;

fn project_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("RUSTLUA_TANK_PROJECT").ok()?);
    assert!(
        root.join("project.toml").is_file(),
        "RUSTLUA_TANK_PROJECT 指向的目录没有 project.toml：{}",
        root.display()
    );
    Some(root)
}

fn boot(root: &Path) -> RuntimeScene {
    let scene_path = root.join("scenes").join("tank_battle.scene");
    let scene = Scene::load(&scene_path)
        .unwrap_or_else(|error| panic!("加载 {} 失败：{error}", scene_path.display()));
    let runtime = RuntimeScene::from_scene(Some(root), scene)
        .unwrap_or_else(|error| panic!("实例化 TankBattle 失败：{error}"));
    let (map, diagnostic) = rustlua_gameplay::settings::load_input_section(root);
    assert!(
        diagnostic.is_none(),
        "project.toml [input] 段有诊断：{diagnostic:?}"
    );
    runtime.input.set_action_map(map);
    runtime
}

fn debug_state(runtime: &RuntimeScene) -> serde_json::Value {
    runtime
        .script
        .eval_json("__tank_debug_state()")
        .expect("读取 __tank_debug_state() 失败")
}

fn step(runtime: &mut RuntimeScene, input: &InputSnapshot, frames: usize) {
    for _ in 0..frames {
        runtime
            .run_script_steps(1, 1.0 / 60.0, input)
            .expect("TankBattle 步进失败");
    }
}

#[test]
fn tank_project_self_test() {
    let Some(root) = project_root() else {
        return;
    };
    let mut runtime = boot(&root);
    step(&mut runtime, &InputSnapshot::default(), 1);

    let value = runtime
        .script
        .eval_json("__tank_self_test()")
        .expect("调用 __tank_self_test() 失败");
    let result = value
        .as_str()
        .unwrap_or_else(|| panic!("自检应返回字符串，实际 {value}"));
    assert!(result.starts_with("ok "), "TankBattle 行为自检失败：\n{result}");
    assert!(
        result.contains("failed=0"),
        "TankBattle 行为自检有失败项：\n{result}"
    );

    let mut checks = 0usize;
    for part in result.split_whitespace() {
        if let Some(value) = part.strip_prefix("checks=") {
            checks = value.parse().unwrap_or(0);
        }
    }
    assert!(checks >= 20, "自检只有 {checks} 条断言：{result}");
    assert_eq!(runtime.script_error_total(), 0, "自检期间出现脚本错误");
}

#[test]
fn tank_project_input_drives_player_and_combat() {
    let Some(root) = project_root() else {
        return;
    };
    let mut runtime = boot(&root);
    step(&mut runtime, &InputSnapshot::default(), 1);

    let before = debug_state(&runtime);
    assert_eq!(before["mode"], "playing", "开局应为 playing：{before}");
    let start_x = before["px"].as_f64().expect("px");

    // 按住 D 半秒：玩家向右移动（走真实的 input.action + tanks.move）。
    let mut right = InputSnapshot::default();
    right.key_event(KeyCode::KeyD, true);
    step(&mut runtime, &right, 30);
    let moved = debug_state(&runtime);
    let moved_x = moved["px"].as_f64().expect("px");
    assert!(
        moved_x > start_x + 8.0,
        "按住 D 后玩家没有右移：{start_x} -> {moved_x}"
    );
    assert!(
        moved_x <= 648.0 - 20.0,
        "玩家越过棋盘右缘：x={moved_x}"
    );

    // 归位后再按 J 一秒：出生点上方有砖墙，子弹必须真的把它打掉。
    runtime
        .script
        .execute_console("__tank_debug_restart()")
        .expect("重开失败");
    step(&mut runtime, &InputSnapshot::default(), 1);
    let walls_before = debug_state(&runtime)["walls"]
        .as_u64()
        .expect("walls");

    let mut fire = InputSnapshot::default();
    fire.key_event(KeyCode::KeyJ, true);
    step(&mut runtime, &fire, 60);
    let after_fire = debug_state(&runtime);
    let walls_after = after_fire["walls"].as_u64().expect("walls");
    assert!(
        walls_after < walls_before,
        "按住 J 一秒后砖墙没有被击毁：{walls_before} -> {walls_after}"
    );

    assert_eq!(
        runtime.script_error_total(),
        0,
        "输入驱动期间出现脚本错误"
    );
}
