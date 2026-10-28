//! M80-02：`Mask` 矩形裁剪与 `CanvasGroup` 透明度/交互开关的宿主门。
//!
//! 三件事必须同源（同一份 `ui_scene_nodes` + 同一次 `apply_scroll_layout`）：
//! - 绘制按遮罩矩形裁 UV，命中框按同一矩形收窄，完全在外的条目既不画也不命中；
//! - `CanvasGroup.alpha` 沿父链相乘后写进图像/文字顶点颜色；
//! - `interactable=false` 让整棵子树不响应命中，`blocks_raycasts=false` 让命中穿过子树。

use super::*;
use rustlua_input::input::{InputSnapshot, MouseButton};
use rustlua_scene::scene::{Scene, SceneEntityId};

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{what}: 期望 {expected}，实际 {actual}"
    );
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

fn press_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.mouse_event(MouseButton::Left, true);
    input
}

/// 快照的边沿是"状态变化"：先按下并清掉按下边沿，再抬起才有 just_released。
fn release_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.mouse_event(MouseButton::Left, true);
    input.finish_frame();
    input.mouse_event(MouseButton::Left, false);
    input
}

/// click 由**抬起**派发（M80-07）：一次按下 + 一次抬起才算一次点击。
fn click(runtime: &mut RuntimeScene, x: f64, y: f64) {
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(x, y))
        .unwrap();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release_at(x, y))
        .unwrap();
}

/// 遮罩面板 (100,100)-(300,200)，padding 10 ⇒ 可见区 (110,110)-(290,190)；
/// 子图像铺满 400×300，另有一条完全落在遮罩右侧的子项。
fn mask_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_mask");
    add(
        &mut scene,
        1,
        "MaskPanel",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=100,100,300,200"),
            ("Mask", "padding=10,10,10,10"),
        ],
    );
    add(
        &mut scene,
        2,
        "Big",
        Some(1),
        &[
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=1,1,1,1",
            ),
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,400,300"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    add(
        &mut scene,
        3,
        "Outside",
        Some(1),
        &[
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=1,1,1,1",
            ),
            ("UiLayout", "anchors=0,0,0,0;offsets=300,0,500,50"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

#[test]
fn mask_clips_hit_rect_and_draw_quad() {
    let mut runtime = mask_runtime();
    let targets = runtime.compute_click_targets();
    let big = targets
        .iter()
        .find(|target| target.name == "Big")
        .unwrap_or_else(|| panic!("Big 应在命中集: {targets:#?}"));
    assert_close(big.origin[0], 110.0, "Big 命中框左沿贴遮罩内边");
    assert_close(big.origin[1], 110.0, "Big 命中框上沿贴遮罩内边");
    assert_close(big.size[0], 180.0, "Big 命中框宽度 = 遮罩内宽");
    assert_close(big.size[1], 80.0, "Big 命中框高度 = 遮罩内高");
    assert!(
        targets.iter().all(|target| target.name != "Outside"),
        "完全在遮罩外的项不得进入命中集：{targets:#?}"
    );

    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    runtime
        .push_scene_ui_images(&mut vertices, &mut draws, 1280, 720)
        .unwrap();
    assert_eq!(vertices.len(), 6, "只有 Big 一个可见四边形");
    let mut min = [f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN];
    for vertex in &vertices {
        let x = (vertex.position[0] + 1.0) * 0.5 * 1280.0;
        let y = (1.0 - vertex.position[1]) * 0.5 * 720.0;
        min[0] = min[0].min(x);
        min[1] = min[1].min(y);
        max[0] = max[0].max(x);
        max[1] = max[1].max(y);
    }
    assert_close(min[0], 110.0, "画出区域左沿");
    assert_close(min[1], 110.0, "画出区域上沿");
    assert_close(max[0], 290.0, "画出区域右沿");
    assert_close(max[1], 190.0, "画出区域下沿");

    let script = write_guard_script(
        "m80-mask-click",
        "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();
    // 点击遮罩外、但在 Big 作者矩形内的点 (320,150)：裁剪后不可命中。
    click(&mut runtime, 320.0, 150.0);
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "nil",
        "遮罩外的作者矩形区域不得响应点击"
    );
    // 遮罩内的点仍然命中 Big。
    click(&mut runtime, 200.0, 150.0);
    assert_eq!(runtime.script.call_update().unwrap(), "Big");
    let _ = std::fs::remove_file(script);
}

fn group_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_canvas_group");
    // 半透明组：子图像 alpha 应变成 0.5。
    add(
        &mut scene,
        1,
        "Fade",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=10,10,110,110"),
            ("CanvasGroup", "alpha=0.5"),
        ],
    );
    add(
        &mut scene,
        2,
        "FadeIcon",
        Some(1),
        &[
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=1,1,1,1",
            ),
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,100,100"),
        ],
    );
    // 不可交互组：子树按钮点不动。
    add(
        &mut scene,
        3,
        "Locked",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=200,10,300,110"),
            ("CanvasGroup", "interactable=false"),
        ],
    );
    add(
        &mut scene,
        4,
        "LockedButton",
        Some(3),
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,100,50"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    // 射线穿透组：命中应落到它下面的 Under。
    add(
        &mut scene,
        5,
        "Under",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=400,10,500,60"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    add(
        &mut scene,
        6,
        "Ghost",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=400,10,500,110"),
            ("CanvasGroup", "blocks_raycasts=false"),
        ],
    );
    add(
        &mut scene,
        7,
        "GhostButton",
        Some(6),
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,100,50"),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

#[test]
fn canvas_group_alpha_scales_draw() {
    let mut runtime = group_runtime();
    runtime.ui_rects_by_name([1280.0, 720.0]);
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    runtime
        .push_scene_ui_images(&mut vertices, &mut draws, 1280, 720)
        .unwrap();
    assert_eq!(vertices.len(), 6, "FadeIcon 是唯一的图像");
    assert_close(vertices[0].color[3], 0.5, "父组 alpha=0.5 乘进图像颜色");
}

#[test]
fn canvas_group_interactable_and_raycast_switches_apply_to_subtree() {
    let mut runtime = group_runtime();
    let script = write_guard_script(
        "m80-group-click",
        "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();

    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(250.0, 35.0))
        .unwrap();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release_at(250.0, 35.0))
        .unwrap();
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "nil",
        "interactable=false 的组内按钮不得响应点击"
    );

    click(&mut runtime, 450.0, 35.0);
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "Under",
        "blocks_raycasts=false 的组必须把命中让给下面的按钮"
    );
    let _ = std::fs::remove_file(script);
}
