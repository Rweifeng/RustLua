//! M76-07：场景侧虚拟列表的验收门（docs/76 §3.7）。
//!
//! 场景：200×100 视口列表（`ScrollView` + `UiVirtualList count=5000, item_height=20,
//! overscan=2`）+ 行模板子实体（`Image` + `Text "Row {n}"`）。
//!
//! 验收门原文（用户锁定）："把导出报错改成真实现后，用 5,000 行列表断言『每帧只实例化
//! 可见行 ± 1 屏』、滚动到中部再回顶部行内容不串位、总实体数与数据行数解耦。" 三条分别
//! 落在 [`window_rows_are_bounded_and_detached_from_data_rows`]、
//! [`scroll_to_middle_and_back_keeps_row_content_stable`]、
//! [`content_extent_uses_total_height_so_scroll_reaches_the_last_row`]。
//!
//! 窗口口径：视口 100px / 行高 20px ⇒ 满窗 5 行 + 压边 1 行 = 6 行可见；过扫 2 ⇒
//! 窗口上界 = 6 + 2×2 = 10（首窗贴顶时 start 夹 0，实际 8 行）。

use super::*;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_ui::ui::UiText;
use rustlua_ui::ui_virtual_list::UiVirtualList;

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

fn wheel_at(dy: f32) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.cursor_moved(60.0, 60.0);
    input.scroll_event(0.0, dy);
    input
}

/// 布局解算（绘制趟、裁剪集与虚拟行池都吃这份结果）。
fn resolve_ui(runtime: &mut RuntimeScene) {
    runtime.ui_rects_by_name([1280.0, 720.0]);
}

/// 5000 行虚拟列表：视口 200×100 + 行模板（Image + Text `Row {n}`）。
fn virtual_list_runtime() -> RuntimeScene {
    let mut scene = Scene::new("ui_virtual_list");
    add(
        &mut scene,
        1,
        "List",
        None,
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,200,100"),
            ("ScrollView", "horizontal=false"),
            (
                "UiVirtualList",
                "count=5000;item_height=20;spacing=0;overscan=2",
            ),
        ],
    );
    add(
        &mut scene,
        2,
        "Template",
        Some(1),
        &[
            ("UiLayout", "anchors=0,0,0,0;offsets=0,0,200,20"),
            (
                "Image",
                "texture=assets/textures/scroll_clip_probe.png;color=1,1,1",
            ),
            ("Text", "text=Row {n};font_size=8;color=1,1,1"),
        ],
    );
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
    runtime.set_ui_interaction_enabled(true);
    runtime
}

/// 断言某行实体的文本内容（从世界组件读，不是从名字猜）。
fn assert_row_text(runtime: &RuntimeScene, index: usize, expected: &str) {
    let handle = runtime.ui_virtual_rows["List"][&index];
    let text = runtime.script.with_world(|world| {
        world
            .get::<UiText>(handle)
            .ok()
            .flatten()
            .map(|t| t.text.clone())
    });
    assert_eq!(text.as_deref(), Some(expected), "row {index} 的文本内容");
}

/// 滚动到指定偏移（一帧一步的滚轮输入；`dy<0` 向下、`dy>0` 向上）。
fn wheel_to(runtime: &mut RuntimeScene, target: f32, dy: f32) {
    let mut guard = 0usize;
    loop {
        let offset = runtime.ui_scroll_offset("List").unwrap_or(0.0);
        let done = if dy < 0.0 {
            offset >= target - 1.0
        } else {
            offset <= target + 1.0
        };
        if done || guard >= 10_000 {
            break;
        }
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(dy))
            .unwrap();
        resolve_ui(runtime);
        guard += 1;
    }
}

/// 断言 1 + 断言 3：全 5000 行滚动过程中，每帧只材料化可见行 ± 1 屏；
/// 世界里的行实体数与数据行数解耦（窗口行 + 模板，与 5000 无关）。
#[test]
fn window_rows_are_bounded_and_detached_from_data_rows() {
    let mut runtime = virtual_list_runtime();
    resolve_ui(&mut runtime);

    let frame = runtime.ui_virtual_frame;
    assert_eq!(
        frame.live_rows, 8,
        "100px 视口 / 20px 行高 + 过扫 2 ⇒ 首窗 8 行"
    );
    assert_eq!(frame.spawned, 8, "首帧全部是新建");
    assert_eq!(frame.despawned, 0, "首帧无回收");
    assert_eq!(runtime.ui_virtual_rows["List"].len(), 8);
    // 行名按约定登记（`{列表名}.Row{n}`），`ui_rects_by_name` 读得到。
    let rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert!(
        rects.contains_key("List.Row3"),
        "行名约定 {{列表名}}.Row{{n}}：{rects:#?}"
    );
    assert!(
        !rects.contains_key("Template"),
        "行模板实体不得参与布局（只当克隆源）"
    );
    assert!(
        !rects.contains_key("List.Row9"),
        "首窗只到 Row7（过扫夹顶）"
    );

    // 滚 100 帧 × 40px = 4000px（覆盖中部）：每帧窗口上界成立、行数不涨。
    let mut max_live = frame.live_rows;
    let mut max_spawned = frame.spawned;
    let mut max_despawned = frame.despawned;
    for _ in 0..100 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &wheel_at(-1.0))
            .unwrap();
        resolve_ui(&mut runtime);
        let frame = runtime.ui_virtual_frame;
        max_live = max_live.max(frame.live_rows);
        max_spawned = max_spawned.max(frame.spawned);
        max_despawned = max_despawned.max(frame.despawned);
    }
    assert_close(
        runtime.ui_scroll_offset("List").unwrap_or(0.0),
        4000.0,
        "滚 100 步 = 4000px（中部）",
    );
    assert!(
        max_live <= 10,
        "窗口上界 = 可见 6 行 + 过扫 2×2 = 10：{max_live}"
    );
    assert!(
        max_spawned <= 10 && max_despawned <= 10,
        "单帧材料化数 ≤ 窗口上界：spawn {max_spawned} / despawn {max_despawned}"
    );

    // 总实体数与数据行数解耦：世界里的 `UiText` 数 = 窗口行 + 模板（1），与 5000 无关。
    let text_count = runtime.script.with_world(|world| {
        let mut count = 0usize;
        world.for_each::<UiText>(|_, _| count += 1);
        count
    });
    assert_eq!(
        text_count,
        runtime.ui_virtual_frame.live_rows + 1,
        "实体数只随窗口走，不随数据行数（5000）走"
    );
    assert!(text_count <= 11, "实体数上界：{text_count}");
}

/// 断言 2：滚动到中部再回顶部，行内容不串位——同一下标文本逐字节不变、
/// 矩形回原位；窗口内的行跨帧复用原实体（不重建），滚出再滚回由竞技场按槽位复用
/// （代际增长是 ECS 槽位复用的正常现象，内容与位置才是验收判据）。
#[test]
fn scroll_to_middle_and_back_keeps_row_content_stable() {
    let mut runtime = virtual_list_runtime();
    resolve_ui(&mut runtime);

    // 记住首窗 Row3 的实体句柄（窗口内复用判据）与矩形。
    let row3_handle = runtime.ui_virtual_rows["List"][&3];
    let top_rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    let row3_top_y = top_rects["List.Row3"].y;
    assert_close(row3_top_y, 60.0, "首窗 Row3 内容坐标 60");
    assert_row_text(&runtime, 3, "Row 3");

    // 滚 1 步（offset 40）：Row3 仍留在窗口（0..10）⇒ 必须复用原实体，不重建。
    runtime
        .run_script_steps(1, 1.0 / 60.0, &wheel_at(-1.0))
        .unwrap();
    resolve_ui(&mut runtime);
    assert_eq!(
        runtime.ui_virtual_rows["List"][&3], row3_handle,
        "窗口内滚动：同一下标复用原实体句柄"
    );

    // 滚到中部：offset 4000 ⇒ 窗口 198..208。
    wheel_to(&mut runtime, 4000.0, -1.0);
    assert_close(
        runtime.ui_scroll_offset("List").unwrap_or(0.0),
        4000.0,
        "滚到中部",
    );
    let mid_rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(
        mid_rects["List.Row201"].y,
        20.0 * 201.0 - 4000.0,
        "中部 Row201 内容坐标 4020 − 偏移 4000",
    );
    assert_row_text(&runtime, 201, "Row 201");
    // 首窗行已滚出并被回收。
    assert!(
        !runtime.ui_virtual_rows["List"].contains_key(&3),
        "Row3 应已滚出窗口"
    );

    // 回到顶部：正滚轮 100 步。
    wheel_to(&mut runtime, 0.0, 1.0);
    assert_close(
        runtime.ui_scroll_offset("List").unwrap_or(0.0),
        0.0,
        "回到顶部",
    );
    let back_rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(
        back_rects["List.Row3"].y,
        row3_top_y,
        "回顶后 Row3 矩形回原位（不串位）",
    );
    assert_row_text(&runtime, 3, "Row 3");
    assert_eq!(
        runtime.ui_virtual_rows["List"][&3].index(),
        row3_handle.index(),
        "回顶后 Row3 回到原槽位（ECS 槽位复用）"
    );
    // 中部行已回收。
    assert!(!runtime.ui_virtual_rows["List"].contains_key(&201));
}

/// 数据签名变化（count/行高/间距/过扫）⇒ 整池重建，旧行名必须摘干净，
/// 否则世界槽位复用后 `entity.find` 会把新实体认成旧行。
#[test]
fn data_change_rebuilds_the_pool_and_retires_old_row_names() {
    let mut runtime = virtual_list_runtime();
    resolve_ui(&mut runtime);
    assert!(
        runtime
            .script
            .entity_names_by_slot()
            .values()
            .any(|name| name == "List.Row3")
    );

    let list = runtime.script.entity_handle("List").expect("列表实体");
    runtime
        .script
        .with_world_mut(|world| {
            world
                .insert(
                    list,
                    UiVirtualList {
                        count: 3,
                        ..UiVirtualList::default()
                    },
                )
                .map_err(|error| error.to_string())
        })
        .expect("改写 count");

    resolve_ui(&mut runtime);
    assert_eq!(
        runtime.ui_virtual_frame.live_rows, 3,
        "count=3 ⇒ 只材料化 3 行"
    );
    let names = runtime.script.entity_names_by_slot();
    assert!(
        !names.values().any(|name| name == "List.Row3"),
        "整池重建后旧行名必须摘干净：{names:?}"
    );
    assert!(names.values().any(|name| name == "List.Row2"));
}

/// 断言 3 的滚动范围半边：内容尺寸必须用数据行总高（`total_height`）而非已材料化行的
/// 子孙包围盒——否则 5000 行只能滚一屏就夹死，滚不到底。
#[test]
fn content_extent_uses_total_height_so_scroll_reaches_the_last_row() {
    let mut runtime = virtual_list_runtime();
    // 直接写偏移是内容覆盖的验证点：99900 = 5000×20 − 100 必须被放行（夹回就是红）。
    runtime
        .ui_scroll_offsets
        .insert("List".to_owned(), 99_900.0);
    resolve_ui(&mut runtime);

    assert_close(
        runtime.ui_scroll_offset("List").expect("偏移应已写入"),
        99_900.0,
        "最大偏移 = total_height − 视口（内容尺寸覆盖）",
    );
    let rects = runtime.ui_rects_by_name([1280.0, 720.0]);
    assert_close(
        rects["List.Row4999"].y,
        20.0 * 4999.0 - 99_900.0,
        "最后一行（内容坐标 99980 − 99900）贴在视口内",
    );
    assert_row_text(&runtime, 4999, "Row 4999");
    // 底部窗口 4993..5000，共 7 行。
    assert_eq!(
        runtime.ui_virtual_frame.live_rows, 7,
        "底部窗口 = 5000 − start(4993)"
    );
}
