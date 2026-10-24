//! M76-02：实体 UI 的滚动与裁剪门。
//!
//! 场景是程序化摆的 200×100 视口 + 三个 200×80 子项（内容高 240，最大偏移 140）：
//! - 滚轮经交互树写偏移，布局把子项整体上移，越界值夹到 `content - viewport`；
//! - 命中框按视口裁剪：露出来的只剩可见高度，完全滚出去的项直接不进命中集；
//! - 图像绘制按可见子矩形裁 UV（无 scissor 的 CPU 路径），滚 40px 后只改 UV 不改位置。

use super::*;
use rustlua_input::input::InputSnapshot;
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

fn child_layout(y: f32) -> String {
    // UiLayout 的 offsets 是 left,top,right,bottom（相对父矩形），不是 x,y,w,h。
    format!("anchors=0,0,0,0;offsets=0,{y},200,{}", y + 80.0)
}

/// 视口 (10,10)-(210,110)，三个 200×80 子项依次排在 y=0/80/160（相对视口）。
fn scroll_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_scroll");
    add(
        &mut scene,
        1,
        "View",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=10,10,210,110"),
            ("ScrollView", "horizontal=false"),
        ],
    );
    for (id, name, y) in [(2, "Item1", 0.0), (3, "Item2", 80.0), (4, "Item3", 160.0)] {
        let layout = child_layout(y);
        add(
            &mut scene,
            id,
            name,
            Some(1),
            &[
                ("UiLayout", layout.as_str()),
                ("UiEvents", "event=click;function=on_item_click"),
            ],
        );
    }
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

fn wheel_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.scroll_event(0.0, -1.0);
    input
}

fn press_at(x: f64, y: f64) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(x, y);
    input.mouse_event(rustlua_input::input::MouseButton::Left, true);
    input
}

#[test]
fn wheel_scrolls_content_and_clips_hit_rects() {
    let mut runtime = scroll_runtime();
    runtime.set_ui_interaction_enabled(true);

    let before = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(before["Item2"].y, 90.0, "初始 Item2 的 y");
    // 偏移为 0 时 Item3(y=170..250) 整个在视口外；Item2(y=90..170) 只露出 20px。
    let targets = runtime.compute_click_targets();
    assert!(
        targets.iter().all(|target| target.name != "Item3"),
        "视口外的项不应出现在命中集：{targets:#?}"
    );
    let item2 = targets
        .iter()
        .find(|target| target.name == "Item2")
        .unwrap_or_else(|| panic!("Item2 应在命中集: {targets:#?}\n布局: {before:#?}"));
    assert_close(item2.size[1], 20.0, "Item2 的可见高度");

    // 4 个滚轮步 × 40px = 160px，夹到上限 140px。
    // 每个 `run_script_steps` 是多固定步的一"帧"：边沿在第一步后清掉，滚轮要一帧一帧喂。
    for _ in 0..4 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
            .unwrap();
    }

    let after = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(
        runtime.ui_scroll_offset("View").expect("滚动偏移应已写入"),
        140.0,
        "越界偏移夹回上限",
    );
    assert_close(after["Item1"].y, -130.0, "滚到底后 Item1 的 y");
    assert_close(after["Item2"].y, -50.0, "滚到底后 Item2 的 y");
    assert_close(after["Item3"].y, 30.0, "滚到底后 Item3 的 y");

    let targets = runtime.compute_click_targets();
    assert!(
        targets.iter().all(|target| target.name != "Item1"),
        "滚出视口的 Item1 不应出现在命中集：{targets:#?}"
    );
    let item2 = targets
        .iter()
        .find(|target| target.name == "Item2")
        .unwrap();
    assert_close(item2.origin[1], 10.0, "Item2 命中框上沿贴视口上沿");
    assert_close(item2.size[1], 20.0, "Item2 只剩 20px 可见高度");
    let item3 = targets
        .iter()
        .find(|target| target.name == "Item3")
        .unwrap();
    assert_close(item3.origin[1], 30.0, "Item3 命中框随内容上移");
    assert_close(item3.size[1], 80.0, "Item3 已完整进入视口");

    // 交互树吃的是内容空间矩形，由 `UiScroll` 自己平移；点视口中央（屏幕 y=60）
    // 落在滚上来的 Item3 上，而不是布局原点处的 Item1。
    let script = write_guard_script(
        "m76-scroll-click",
        "function on_item_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
    );
    runtime.script.execute_project_script(&script).unwrap();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press_at(60.0, 60.0))
        .unwrap();
    assert_eq!(
        runtime.script.call_update().unwrap(),
        "Item3",
        "滚到底后点击应命中滚上来的 Item3"
    );
    let _ = std::fs::remove_file(script);
}

#[test]
fn scroll_requires_interaction_enabled() {
    let mut runtime = scroll_runtime();
    runtime
        .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
        .unwrap();

    assert_eq!(
        runtime.ui_scroll_offset("View"),
        None,
        "交互路由关闭时滚轮不得改偏移（旧路径逐字节不变）"
    );
    let rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(rects["Item2"].y, 90.0, "未滚动时布局保持原位");
}

/// 图像裁剪：滚 40px 后可见区域不变、只重映射 UV，证明走的是裁 UV 而非挪四边形。
#[test]
fn clipped_image_remaps_uv_without_moving_quad() {
    let mut scene = Scene::new("ui_scroll_image");
    add(
        &mut scene,
        1,
        "View",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=10,10,210,110"),
            ("ScrollView", "horizontal=false"),
        ],
    );
    add(
        &mut scene,
        2,
        "Icon",
        Some(1),
        &[
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=1,1,1",
            ),
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,200,200"),
        ],
    );
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    runtime.set_ui_interaction_enabled(true);

    let push = |runtime: &mut RuntimeScene| {
        runtime.ui_rects_by_name([1280.0, 720.0]);
        let mut vertices = Vec::new();
        let mut draws = Vec::new();
        runtime
            .push_scene_ui_images(&mut vertices, &mut draws, 1280, 720)
            .unwrap();
        (vertices, draws)
    };

    // 图像 (10,10)-(210,210) 与视口下沿 110 相交：可见高 100 / 全高 200 ⇒ v1 = 0.5。
    let (vertices, _) = push(&mut runtime);
    assert_eq!(vertices.len(), 6, "单张图像应是一个四边形");
    assert_close(vertices[0].uv[1], 0.0, "裁剪前的上边 UV");
    assert_close(vertices[2].uv[1], 0.5, "裁剪后的下边 UV");
    let y0_before = vertices[0].position[1];

    runtime
        .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
        .unwrap();
    let (vertices, _) = push(&mut runtime);
    assert_eq!(vertices.len(), 6);
    assert_close(vertices[0].uv[1], 0.2, "滚 40px 后上边 UV 移到 40/200");
    assert_close(
        vertices[2].uv[1],
        0.7,
        "滚 40px 后下边 UV 移到 (110+40)/200",
    );
    assert_close(
        vertices[0].position[1],
        y0_before,
        "视口不动 ⇒ 四边形位置不变，只有 UV 变",
    );
}
