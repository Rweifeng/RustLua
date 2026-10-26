//! M76-02：实体 UI 的滚动与裁剪门。
//!
//! 场景是程序化摆的 200×100 视口 + 三个 200×80 子项（内容高 240，最大偏移 140）：
//! - 滚轮经交互树写偏移，布局把子项整体上移，越界值夹到 `content - viewport`；
//! - 命中框按视口裁剪：露出来的只剩可见高度，完全滚出去的项直接不进命中集；
//! - 图像绘制按可见子矩形裁 UV（无 scissor 的 CPU 路径），滚 40px 后只改 UV 不改位置。
//!
//! M79 另加一组「视口外条目不进装配」的门（`scrolled_away_slots`）：画面逐字节不变，
//! 变的是**进趟的条目数**——长列表每帧的克隆/查表/字形预热从"总项数"降到"窗口项数"。

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

// ── M79：视口外条目不进装配 ────────────────────────────────────────────────

/// 长列表：视口与 `scroll_runtime` 同款 (10,10)-(210,110)，`rows` 个 `row_height` 高的条目，
/// 每条同时带 `Image` 与 `Text`（文字趟的省头更大：进趟的每条都会被栅格化进图集）。
/// 名字 `Row0..`，槽位 = 2 + 行号。
fn scroll_rows_runtime(rows: usize, row_height: f32) -> RuntimeScene {
    let mut scene = Scene::new("ui_scroll_rows");
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
    for row in 0..rows {
        let top = row_height * row as f32;
        let layout = format!("anchors=0,0,0,0;offsets=0,{top},200,{}", top + row_height);
        let text = format!("text=r{row};font_size=8;color=1,1,1");
        add(
            &mut scene,
            2 + row as u64,
            &format!("Row{row}"),
            Some(1),
            &[
                (
                    "Image",
                    "texture=assets/textures/scroll_clip_probe.png;color=1,1,1",
                ),
                ("UiLayout", layout.as_str()),
                ("Text", text.as_str()),
            ],
        );
    }
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

/// 布局解算（绘制趟与裁剪集都吃这份结果）。
fn resolve_ui(runtime: &mut RuntimeScene) {
    runtime.ui_rects_by_name([1280.0, 720.0]);
}

/// 本帧被摘掉的槽位换成实体名（排序后好比对）。
fn away_names(runtime: &RuntimeScene) -> Vec<String> {
    let names = runtime.script.entity_names_by_slot();
    let mut away = runtime
        .scrolled_away_slots()
        .iter()
        .filter_map(|slot| names.get(slot).cloned())
        .collect::<Vec<_>>();
    away.sort();
    away
}

/// 只跑图像趟，返回写出的顶点数。
fn pushed_image_vertices(runtime: &mut RuntimeScene) -> usize {
    resolve_ui(runtime);
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    runtime
        .push_scene_ui_images(&mut vertices, &mut draws, 1280, 720)
        .unwrap();
    vertices.len()
}

/// 1000 项 × 20px：进装配的条目数与画出来的四边形数**分开**断言——
/// 前者是这批改的东西（6 条：满窗 5 条 + 压边 1 条），后者是它**不该**改的东西（5 个四边形）。
#[test]
fn long_list_assembles_only_the_visible_window() {
    let mut runtime = scroll_rows_runtime(1000, 20.0);
    runtime.set_ui_interaction_enabled(true);
    resolve_ui(&mut runtime);

    let (images, image_masked) = runtime.scene_ui_image_candidates();
    assert_eq!(
        images.len(),
        6,
        "100px 视口 / 20px 行高 ⇒ 满窗 5 行，压边的第 6 行靠余量留下"
    );
    assert_eq!(image_masked, 0, "没有遮罩时不该有条目被拦下");
    let (texts, text_masked) = runtime.scene_ui_text_candidates();
    assert_eq!(
        texts.len(),
        6,
        "文字趟同理：`prewarm_frame_glyphs` 吃的就是这份"
    );
    assert_eq!(text_masked, 0);
    assert_eq!(
        away_names(&runtime).len(),
        994,
        "1000 项里应摘掉 994 项（窗口 5 条 + 压边 1 条 + 容器自身留在原地）"
    );

    assert_eq!(
        pushed_image_vertices(&mut runtime),
        30,
        "画面不变：压边那条进了装配但真实交集为空 ⇒ 写不出顶点"
    );
    assert_eq!(
        runtime.take_view_filtered(),
        0,
        "视口裁剪不得记进 `view_filtered`——那个数的口径是被可见性遮罩拦下"
    );

    // 滚 5 步 × 40px = 200px：窗口平移到第 10..14 行，两侧各一条压边。
    for _ in 0..5 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
            .unwrap();
    }
    assert_close(
        runtime.ui_scroll_offset("View").expect("偏移应已写入"),
        200.0,
        "滚 200px",
    );
    resolve_ui(&mut runtime);
    assert_eq!(
        runtime.scene_ui_image_candidates().0.len(),
        7,
        "窗口跟着位移：第 9..15 行留在装配里"
    );
    assert_eq!(away_names(&runtime).len(), 993);
    assert_eq!(
        pushed_image_vertices(&mut runtime),
        30,
        "位移后进网格的仍是 5 个四边形"
    );
}

/// 1px 余量单独成门：去掉 `SCROLL_CULL_MARGIN_PX` 时 `Row5` 会被摘掉，这一条会红。
/// 症状是"滚动时边缘闪一下白"——最难复现的一类，所以宁可多留一条不进网格的条目。
#[test]
fn straddle_row_stays_in_assembly() {
    let mut runtime = scroll_rows_runtime(12, 20.0);
    runtime.set_ui_interaction_enabled(true);
    resolve_ui(&mut runtime);

    let away = away_names(&runtime);
    assert!(
        !away.contains(&"Row5".to_owned()),
        "Row5 屏幕矩形 110..130 正好压在视口下沿，放大 1px 后必须留在装配里：{away:?}"
    );
    assert!(
        away.contains(&"Row6".to_owned()),
        "Row6（130..150）放大 1px 后仍不相交，应当摘掉：{away:?}"
    );
    assert_eq!(away, ["Row10", "Row11", "Row6", "Row7", "Row8", "Row9"]);
    assert!(
        !away.contains(&"View".to_owned()),
        "滚动容器自身没有滚动祖先 ⇒ 不进裁剪集：{away:?}"
    );
}

/// 裁剪集与命中集必须互斥：命中框早就按视口裁过（M76-02），装配侧再摘一层不能把
/// 还点得到的项摘掉，否则"看得见却点不中"会比白块更难归因。
#[test]
fn culled_slots_never_appear_in_hit_targets() {
    let mut runtime = scroll_runtime();
    runtime.set_ui_interaction_enabled(true);
    resolve_ui(&mut runtime);

    let away = away_names(&runtime);
    assert_eq!(
        away,
        ["Item3".to_owned()],
        "初始只有整条在视口外的 Item3 该摘掉"
    );
    let targets = runtime
        .compute_click_targets()
        .iter()
        .map(|target| target.name.clone())
        .collect::<Vec<_>>();
    for name in &away {
        assert!(
            !targets.contains(name),
            "被摘掉的 {name} 不该出现在命中集：{targets:?}"
        );
    }

    for _ in 0..4 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
            .unwrap();
    }
    resolve_ui(&mut runtime);
    let away = away_names(&runtime);
    assert_eq!(away, ["Item1".to_owned()], "滚到底后换 Item1 出视口");
    let targets = runtime
        .compute_click_targets()
        .iter()
        .map(|target| target.name.clone())
        .collect::<Vec<_>>();
    assert!(
        targets.contains(&"Item3".to_owned()),
        "滚进来的 Item3 既要点得中也要进装配：{targets:?}"
    );
    for name in &away {
        assert!(
            !targets.contains(name),
            "被摘掉的 {name} 不该在命中集：{targets:?}"
        );
    }
}

/// 没有 `ScrollView` 的场景必须一个都不摘（逐帧行为与今天逐字节一致）：
/// 屏幕外的条目归相机/视口那趟管，这一层只认"滚出滚动容器视口"。
#[test]
fn no_scroll_container_keeps_every_entity_in_assembly() {
    let mut scene = Scene::new("ui_no_scroll_rows");
    for (id, name, y) in [(1, "Near", 10.0), (2, "Mid", 500.0), (3, "Far", 3000.0)] {
        let layout = format!("anchors=0,0,0,0;offsets=10,{y},210,{}", y + 80.0);
        add(
            &mut scene,
            id,
            name,
            None,
            &[
                (
                    "Image",
                    "texture=assets/textures/scroll_clip_probe.png;color=1,1,1",
                ),
                ("UiLayout", layout.as_str()),
            ],
        );
    }
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    runtime.set_ui_interaction_enabled(true);
    resolve_ui(&mut runtime);

    assert!(
        runtime.scrolled_away_slots().is_empty(),
        "没有滚动容器 ⇒ 裁剪集必须为空"
    );
    assert_eq!(
        runtime.scene_ui_image_candidates().0.len(),
        3,
        "三条图像全部进装配，包括屏幕外的 Far"
    );
    assert_eq!(pushed_image_vertices(&mut runtime), 18, "三个四边形");
}

/// 1000 项 × 60 帧滚动的装配读数（`cargo test -- --nocapture` 可见）。
/// 只断言结构性上界；时间用于基线，不设阈值（CI 机器间不可比，同 `ui_virtual_list` 的口径）。
#[test]
fn thousand_row_scroll_frames_report_assembly_cost() {
    let mut runtime = scroll_rows_runtime(1000, 20.0);
    runtime.set_ui_interaction_enabled(true);
    let frames = 60usize;
    let mut max_images = 0usize;
    let mut max_texts = 0usize;
    let mut max_vertices = 0usize;
    let start = std::time::Instant::now();
    for _ in 0..frames {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(60.0, 60.0))
            .unwrap();
        resolve_ui(&mut runtime);
        let images = runtime.scene_ui_image_candidates().0.len();
        let texts = runtime.scene_ui_text_candidates().0.len();
        let vertices = pushed_image_vertices(&mut runtime);
        max_images = max_images.max(images);
        max_texts = max_texts.max(texts);
        max_vertices = max_vertices.max(vertices);
    }
    let elapsed = start.elapsed();
    eprintln!(
        "1000 项滚动列表：{frames} 帧，峰值装配 {max_images} 图/{max_texts} 文、{max_vertices} 顶点，平均 {:?}/帧（含布局解算）",
        elapsed / frames as u32
    );
    assert!(max_images <= 40, "装配条目数失控：{max_images}");
    assert!(max_texts <= 40, "文字装配条目数失控：{max_texts}");
    assert!(max_vertices <= 240, "顶点数失控：{max_vertices}");
}
