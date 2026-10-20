use super::*;
use rustlua_input::input::InputSnapshot;
use rustlua_input::input::KeyCode;
use rustlua_input::input::MouseButton;

/// 示例工程的**入口菜单**能载入、跑一步，并且菜单卡片都是结构化的 `UiEvents` 实体。
///
/// 这三条（本测试与下面两条）原本对着旧工程的口号式返回值（`menu_ready` /
/// `production_ok`）断言；示例工程重建后改成**断言可观测状态**
/// （当前场景名、存档是否落盘、实体数量），这样演示模块的措辞再改也不会误伤。
#[test]
fn sample_project_index_scene_runs_the_project_script() {
    use rustlua_scene::scene::Scene;
    use rustlua_ui::ui_events::UiEvents;

    let root = production_sample_root();
    let path = root.join("scenes/index.scene");
    if !path.is_file() {
        return;
    }
    let scene = Scene::load(&path).expect("load index scene");
    let (installers, _host) = rustlua_gameplay::input_lua::registry_with_handles();
    let mut runtime = LuaRuntime::with_installers(installers).unwrap();
    runtime.set_project_root(root.clone());
    runtime.instantiate_scene(&scene).unwrap();
    runtime
        .execute_project_script(&root.join("scripts/main.lua"))
        .expect("load sample main.lua");

    let result = runtime.call_update().unwrap();
    assert!(
        result.starts_with("index_ok"),
        "入口菜单的演示模块应当接手 update：{result}"
    );
    let cards = runtime.with_world(|world| world.entities_with::<UiEvents>().len());
    assert!(
        cards >= 10,
        "入口菜单的每张卡片都应当是 UiEvents 实体，实际 {cards}"
    );
}

/// 点菜单卡片 → `scene.load` → 宿主处理过渡 → 目标场景的演示模块接手。
#[test]
fn sample_project_menu_click_loads_the_demo_scene() {
    let root = production_sample_root();
    let path = root.join("scenes/index.scene");
    if !path.is_file() {
        return;
    }
    let scene = Scene::load(&path).expect("load index scene");
    let (installers, _host) = rustlua_gameplay::input_lua::registry_with_handles();
    let mut runtime = LuaRuntime::with_installers(installers).unwrap();
    runtime.set_project_root(root.clone());
    runtime.instantiate_scene(&scene).unwrap();
    runtime
        .execute_project_script(&root.join("scripts/main.lua"))
        .expect("load sample main.lua");

    let mut input = InputSnapshot::default();
    input.cursor_moved(200.0, 160.0);
    input.mouse_event(MouseButton::Left, true);
    // 命中矩形由运行时预计算（`RuntimeScene::compute_click_targets`），
    // 这里直接给一个覆盖点击点的目标，实体名就是目标场景名。
    runtime
        .process_ui_clicks(
            Some(input.cursor_position()),
            &[UiClickTarget {
                name: "api_core".to_owned(),
                origin: [120.0, 148.0],
                size: [560.0, 50.0],
                functions: vec!["on_click".to_owned()],
            }],
        )
        .unwrap();
    runtime.process_scene_transition().unwrap();

    assert_eq!(
        runtime.scene_name().as_deref(),
        Some("api_core"),
        "点击卡片后应当切到该演示场景"
    );
    let result = runtime.call_update().unwrap();
    assert!(
        result.starts_with("api_core_ok"),
        "目标场景的演示模块应当接手 update：{result}"
    );
}

/// 演示里"按 S 存档、按 ESC 回菜单"这条真实链路：`save.commit` 真的落盘，
/// `scene.load("index")` 真的切回入口场景。
#[test]
fn sample_project_demo_saves_and_esc_returns_to_menu() {
    let root = production_sample_root();
    let path = root.join("scenes/api_core.scene");
    if !path.is_file() {
        return;
    }
    let dir = std::env::temp_dir().join(format!(
        "rustlua-sample-save-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let save_path = dir.join("save.sav");

    let scene = Scene::load(&path).expect("load api_core scene");
    let (installers, host) = rustlua_gameplay::input_lua::registry_with_handles();
    let mut runtime = LuaRuntime::with_installers(installers).unwrap();
    runtime.set_project_root(root.clone());
    runtime.set_save_path(save_path.clone());
    runtime.instantiate_scene(&scene).unwrap();
    runtime
        .execute_project_script(&root.join("scripts/main.lua"))
        .expect("load sample main.lua");

    // S = `save.slot("demo")` + `save.commit()`（演示里写明的落盘键）。
    let mut save_input = InputSnapshot::default();
    save_input.key_event(KeyCode::KeyS, true);
    host.input.set_input(save_input);
    runtime.call_update().unwrap();
    let slot_file = dir.join("save_demo.sav");
    assert!(
        slot_file.is_file(),
        "按 S 应当把演示槽写盘：{}",
        slot_file.display()
    );

    // ESC = 命名动作 `cancel` → 路由回到入口菜单。
    let mut menu_input = InputSnapshot::default();
    menu_input.key_event(KeyCode::Escape, true);
    host.input.set_input(menu_input);
    runtime.call_update().unwrap();
    runtime.process_scene_transition().unwrap();
    assert_eq!(
        runtime.scene_name().as_deref(),
        Some("index"),
        "ESC 应当回到入口菜单"
    );

    let _ = fs::remove_dir_all(dir);
}
