//! M80-04：值控件（`Toggle` / `Slider` / `Progress` / `Scrollbar`）的宿主管道门。
//!
//! 四条路径必须共用同一份状态与同一套换算：
//! - 布局：按组件值改写子实体矩形（勾选/填充/滑块/手柄），只认控件自己的布局子树；
//! - 命中：三个可操作控件进交互树（无 `UiEvents` 也可聚焦），`Progress` 只读不进树；
//! - 指针：按下改值、按住拖拽跟随、松开结束；Toggle 走单选组语义并派发 `value_changed`；
//! - 键盘：焦点控件消费自己方向的导航轴（改值优先），Enter/Space 切 Toggle。

use super::*;
use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_ui::ui_progress::Progress;
use rustlua_ui::ui_scrollbar::ScrollBar;
use rustlua_ui::ui_slider::Slider;
use rustlua_ui::ui_toggle::Toggle;

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{what}: 期望 {expected}，实际 {actual}"
    );
}

fn assert_rect(rect: UiRect, expected: [f32; 4], what: &str) {
    assert_close(rect.x, expected[0], &format!("{what} x"));
    assert_close(rect.y, expected[1], &format!("{what} y"));
    assert_close(rect.w, expected[2], &format!("{what} w"));
    assert_close(rect.h, expected[3], &format!("{what} h"));
}

fn add(scene: &mut Scene, id: u64, name: &str, parent: Option<u64>, components: &[(&str, &str)]) {
    scene
        .add_entity(SceneEntityId(id), name, parent.map(SceneEntityId))
        .unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
    for (key, value) in components {
        entity.components.insert((*key).into(), (*value).into());
    }
}

fn cursor_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input
}

fn press_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = cursor_at(x, y);
    input.mouse_event(MouseButton::Left, true);
    input
}

fn release_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = cursor_at(x, y);
    input.mouse_event(MouseButton::Left, true);
    input.finish_frame();
    input.mouse_event(MouseButton::Left, false);
    input
}

/// 按住左键移动：`just_pressed` 已在本帧之前消费掉，只剩 `is_down`。
fn drag_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = cursor_at(x, y);
    input.mouse_event(MouseButton::Left, true);
    input.finish_frame();
    input
}

fn key_at(x: f64, y: f64, key: KeyCode) -> InputSnapshot {
    let mut input = cursor_at(x, y);
    input.key_event(key, true);
    input
}

/// 舞台左侧一列控件：
/// - `Toggle` (100,100)-(200,140)，关；同组 `OnRadio` (220,100)-(320,140)，开；
/// - `Slider` (100,200)-(300,240)，值 0.25，滑块 20×20；
/// - `Progress` (100,300)-(300,320)，值 0.5；
/// - `Scrollbar` (100,400)-(300,420)，值 0.3、size 0.25；
/// - `RightButton` (400,200)-(500,240)：只有点击绑定，用来证明滑块吃掉了右方向键。
fn controls_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_controls");
    add(
        &mut scene,
        1,
        "Toggle",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=100,100,200,140"),
            ("Toggle", "on=false;group=radio;checkmark=ToggleCheck"),
            ("UiEvents", "event=value_changed;function=on_value"),
        ],
    );
    add(
        &mut scene,
        2,
        "ToggleCheck",
        Some(1),
        &[("UiLayout", "anchors=0,0,0,0;offsets=150,110,190,130")],
    );
    add(
        &mut scene,
        3,
        "OnRadio",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=220,100,320,140"),
            ("Toggle", "on=true;group=radio;checkmark=RadioCheck"),
            ("UiEvents", "event=value_changed;function=on_value"),
        ],
    );
    add(
        &mut scene,
        4,
        "RadioCheck",
        Some(3),
        &[("UiLayout", "anchors=0,0,0,0;offsets=270,110,310,130")],
    );
    add(
        &mut scene,
        5,
        "Slider",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=100,200,300,240"),
            (
                "Slider",
                "min=0;max=1;value=0.25;fill=SliderFill;handle=SliderHandle",
            ),
            ("UiEvents", "event=value_changed;function=on_value"),
        ],
    );
    // 作者矩形故意放在别处：运行期必须被控件布局改写。
    add(
        &mut scene,
        6,
        "SliderFill",
        Some(5),
        &[("UiLayout", "anchors=0,0,0,0;offsets=10,10,60,30")],
    );
    add(
        &mut scene,
        7,
        "SliderHandle",
        Some(5),
        &[("UiLayout", "anchors=0,0,0,0;offsets=0,0,20,20")],
    );
    add(
        &mut scene,
        8,
        "Progress",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=100,300,300,320"),
            ("Progress", "min=0;max=1;value=0.5;fill=ProgressFill"),
        ],
    );
    add(
        &mut scene,
        9,
        "ProgressFill",
        Some(8),
        &[("UiLayout", "anchors=0,0,0,0;offsets=0,0,20,10")],
    );
    add(
        &mut scene,
        10,
        "Scrollbar",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=100,400,300,420"),
            ("Scrollbar", "value=0.3;size=0.25;handle=ScrollHandle"),
            ("UiEvents", "event=value_changed;function=on_value"),
        ],
    );
    add(
        &mut scene,
        11,
        "ScrollHandle",
        Some(10),
        &[("UiLayout", "anchors=0,0,0,0;offsets=0,0,30,20")],
    );
    add(
        &mut scene,
        12,
        "RightButton",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=400,200,500,240"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

fn toggle_on(runtime: &RuntimeScene, checkmark: &str) -> bool {
    runtime.script.with_world(|world| {
        let mut found = None;
        world.for_each::<Toggle>(|_, toggle| {
            if toggle.checkmark == checkmark {
                found = Some(toggle.is_on);
            }
        });
        found.unwrap_or_else(|| panic!("找不到 checkmark={checkmark} 的 Toggle"))
    })
}

fn slider_value(runtime: &RuntimeScene) -> f32 {
    runtime.script.with_world(|world| {
        let mut found = None;
        world.for_each::<Slider>(|_, slider| found = Some(slider.value));
        found.expect("场景里应有一个 Slider")
    })
}

fn scrollbar_value(runtime: &RuntimeScene) -> f32 {
    runtime.script.with_world(|world| {
        let mut found = None;
        world.for_each::<ScrollBar>(|_, scrollbar| found = Some(scrollbar.value));
        found.expect("场景里应有一个 Scrollbar")
    })
}

fn progress_value(runtime: &RuntimeScene) -> f32 {
    runtime.script.with_world(|world| {
        let mut found = None;
        world.for_each::<Progress>(|_, progress| found = Some(progress.value));
        found.expect("场景里应有一个 Progress")
    })
}

#[test]
fn control_layout_rewrites_children_from_component_values() {
    let mut runtime = controls_runtime();
    let rects = runtime.ui_rects_by_name([1280.0, 720.0]);

    assert_rect(
        rects["ToggleCheck"],
        [100.0, 100.0, 0.0, 0.0],
        "关掉的勾选图收成控件原点上的零尺寸",
    );
    assert_rect(
        rects["RadioCheck"],
        [220.0, 100.0, 100.0, 40.0],
        "打开的勾选图铺满控件矩形",
    );
    // 0.25 × 200 = 50，填充贴最小端；交叉轴铺满轨道。
    assert_rect(
        rects["SliderFill"],
        [100.0, 200.0, 50.0, 40.0],
        "滑块填充条",
    );
    // 行程 = 200 - 20 = 180；0.25 × 180 = 45；交叉轴居中 (40-20)/2 = 10。
    assert_rect(
        rects["SliderHandle"],
        [145.0, 210.0, 20.0, 20.0],
        "滑块保留作者尺寸、沿剩余行程滑动",
    );
    assert_rect(
        rects["ProgressFill"],
        [100.0, 300.0, 100.0, 20.0],
        "进度填充条",
    );
    // 长度 = 200 × 0.25 = 50；位置 = 0.3 × (200 - 50) = 45。
    assert_rect(
        rects["ScrollHandle"],
        [145.0, 400.0, 50.0, 20.0],
        "滚动条滑块长度由 size 决定",
    );
}

#[test]
fn toggles_join_the_hit_tree_but_progress_stays_read_only() {
    let mut runtime = controls_runtime();
    let data = runtime.ui_hit_data();
    let item = |name: &str| {
        data.iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("{name} 应在命中数据里"))
    };
    assert!(item("Toggle").control, "Toggle 没有事件也要进树");
    assert!(item("Slider").control, "Slider 没有事件也要进树");
    assert!(item("Scrollbar").control, "Scrollbar 没有事件也要进树");
    assert!(
        !item("RightButton").control,
        "普通按钮不是值控件，control 标记必须为假"
    );
    let names: Vec<&str> = data.iter().map(|item| item.name.as_str()).collect();
    assert!(
        !names.contains(&"Progress") && !names.contains(&"ProgressFill"),
        "只读进度条不得进命中树：{names:?}"
    );

    // 点击进度条的矩形：命中不到任何节点，焦点也不动。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(150.0, 310.0))
        .unwrap();
    assert_eq!(runtime.ui_focused_name(), None, "只读控件不该抢焦点");
    assert_close(progress_value(&runtime), 0.5, "只读控件不会被点击改值");
}

#[test]
fn pointer_press_toggles_and_dispatches_value_changed_for_the_group() {
    let mut runtime = controls_runtime();
    let script = write_guard_script(
        "m80-controls-toggle",
        "function on_value(name) values = (values or '') .. name .. ';' end\n\
         function on_click(name) clicked = name end\n\
         function update() return (values or '') .. '|' .. tostring(clicked or '-') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(150.0, 120.0))
        .unwrap();
    assert!(toggle_on(&runtime, "ToggleCheck"), "按下把 Toggle 打开");
    assert!(
        !toggle_on(&runtime, "RadioCheck"),
        "同组兄弟必须被关掉（单选组语义）"
    );
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "Toggle;OnRadio;|-",
        "自己与兄弟各派发一次 value_changed，按变化顺序到达脚本"
    );
    assert_eq!(
        runtime.ui_focused_name(),
        Some("Toggle"),
        "值控件没有 UiEvents 也要能拿焦点"
    );

    // 再点一次：Toggle 关闭；它本来就是组里唯一打开的项，兄弟不产生事件。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(150.0, 120.0))
        .unwrap();
    assert!(!toggle_on(&runtime, "ToggleCheck"));
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "Toggle;OnRadio;Toggle;|-",
        "关掉自己只派发自己一次"
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn slider_press_and_drag_follow_the_pointer() {
    let mut runtime = controls_runtime();
    let script = write_guard_script(
        "m80-controls-drag",
        "function on_value(name) last = name end\nfunction update() return tostring(last or '-') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    // 按在轨道 75% 处：值 = 点下去的位置。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(250.0, 220.0))
        .unwrap();
    assert_close(slider_value(&runtime), 0.75, "按下把值设到指针位置");
    assert_eq!(runtime.script.call_update().unwrap(), "Slider");
    assert_eq!(runtime.ui_focused_name(), Some("Slider"));

    // 按住拖动：不看命中树，指针移到哪值到哪（可以拖出控件矩形）。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &drag_at(150.0, 260.0))
        .unwrap();
    assert_close(slider_value(&runtime), 0.25, "拖拽跟随指针");

    // 松开后移动不再改值。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release_at(150.0, 220.0))
        .unwrap();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &cursor_at(250.0, 220.0))
        .unwrap();
    assert_close(slider_value(&runtime), 0.25, "松开即结束拖拽");

    // 点击滚动条同样落到指针位置（长度与位置都改）。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(250.0, 410.0))
        .unwrap();
    assert_close(scrollbar_value(&runtime), 0.75, "滚动条按指针位置改值");
    let rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_rect(
        rects["ScrollHandle"],
        [212.5, 400.0, 50.0, 20.0],
        "0.75 × (200-50) = 112.5 的滑块位置",
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn focused_slider_consumes_the_arrow_axis_and_steps_its_value() {
    let mut runtime = controls_runtime();
    // 先点一下滑块把焦点放上去（同时把值设到 0）。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(100.0, 220.0))
        .unwrap();
    assert_close(slider_value(&runtime), 0.0, "点在最左端");
    assert_eq!(runtime.ui_focused_name(), Some("Slider"));

    runtime
        .run_script_steps(1, 1.0 / 60.0, &key_at(100.0, 220.0, KeyCode::ArrowRight))
        .unwrap();
    assert_close(slider_value(&runtime), 0.1, "右方向键按 10% 行程加值");
    assert_eq!(
        runtime.ui_focused_name(),
        Some("Slider"),
        "方向键被滑块消费，焦点不得移到右侧按钮"
    );

    runtime
        .run_script_steps(1, 1.0 / 60.0, &key_at(100.0, 220.0, KeyCode::ArrowLeft))
        .unwrap();
    assert_close(slider_value(&runtime), 0.0, "左方向键按同一档减回去");

    // 对照：焦点在普通按钮上时，右方向键仍由交互树移动焦点。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(450.0, 220.0))
        .unwrap();
    assert_eq!(runtime.ui_focused_name(), Some("RightButton"));
}

#[test]
fn space_flips_the_focused_toggle() {
    let mut runtime = controls_runtime();
    let script = write_guard_script(
        "m80-controls-space",
        "function on_value(name) last = name end\nfunction update() return tostring(last or '-') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(150.0, 120.0))
        .unwrap();
    assert!(toggle_on(&runtime, "ToggleCheck"));

    runtime
        .run_script_steps(1, 1.0 / 60.0, &key_at(150.0, 120.0, KeyCode::Space))
        .unwrap();
    assert!(
        !toggle_on(&runtime, "ToggleCheck"),
        "Space 在焦点 Toggle 上切换状态"
    );
    assert_eq!(runtime.ui_focused_name(), Some("Toggle"), "切换不改焦点");
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "Toggle",
        "键盘切换与指针走同一条 value_changed 派发"
    );
    let _ = std::fs::remove_file(script);
}
