//! M80-03：场景 `Text` 的对齐 / 换行 / 溢出 / 富文本必须真正到达网格与命中框。
//!
//! 这些用例全部走无项目根（`from_scene_edit(None, _)`）的运行时：没有 SDF/矢量
//! 图集可加载，排版必然落在内置 5×7 点阵路径上，因此顶点坐标是确定量：
//! `font_size=12` ⇒ scale=1，字距 advance=7px（5px 字模 + 1px spacing），
//! 行高 8+2=10px，首行字模高 7px。

use super::*;
use rustlua_scene::scene::Scene;

fn add(scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]) {
    scene.add_entity(SceneEntityId(id), name, None).unwrap();
    let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
    for (key, value) in components {
        entity.components.insert((*key).into(), (*value).into());
    }
}

/// 布局矩形 (20,20)-(20+width,60)；文字实体同时带点击事件，便于验证命中框。
fn text_runtime(width: f32, spec: &str) -> RuntimeScene {
    let mut scene = Scene::new("ui_text_layout");
    let layout = format!("anchors=0,0,0,0;offsets=20,20,{},60", 20.0 + width);
    add(
        &mut scene,
        1,
        "Label",
        &[
            ("UiLayout", layout.as_str()),
            ("Text", spec),
            ("UiEvents", "event=click;function=on_click"),
        ],
    );
    RuntimeScene::from_scene_edit(None, scene).unwrap()
}

fn pixel_bounds(vertices: &[OverlayVertex], width: u32, height: u32) -> [f32; 4] {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    vertices.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |mut bounds, vertex| {
            let x = (vertex.position[0] + 1.0) * 0.5 * w;
            let y = (1.0 - vertex.position[1]) * 0.5 * h;
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x);
            bounds[3] = bounds[3].max(y);
            bounds
        },
    )
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{what}: 期望 {expected}，实际 {actual}"
    );
}

#[test]
fn text_wrap_alignment_and_valign_shape_the_mesh() {
    let mut runtime = text_runtime(24.0, "text=AB CD;font_size=12;align=right;valign=bottom");
    let mesh = runtime.vertices(200, 100).unwrap();
    // 24px 框内 "AB CD" 在空格处换行 ⇒ 2 行 × 2 字形。
    assert_eq!(mesh.vertices.len(), 4 * 6, "应换行成两行四个字形");
    let bounds = pixel_bounds(&mesh.vertices, 200, 100);
    // 行宽 14（两个 advance 7），右对齐于 24px 框 ⇒ 起点 20+10=30，
    // 第二个字形再进 7px ⇒ 右沿 30+7+5=42。
    assert_close(bounds[0], 30.0, "右对齐行首");
    assert_close(bounds[2], 42.0, "右对齐行尾");
    // 内容高 = 首行 7 + 行高 10 = 17；40px 框内底对齐 ⇒ 顶部 20+23=43。
    assert_close(bounds[1], 43.0, "底对齐首行顶部");
    assert_close(bounds[3], 60.0, "第二行底部贴框底");
}

#[test]
fn text_overflow_ellipsis_keeps_glyphs_inside_the_box() {
    let mut overflowing = text_runtime(24.0, "text=ABCDEF;font_size=12;overflow=overflow");
    let overflow_mesh = overflowing.vertices(200, 100).unwrap();
    assert_eq!(overflow_mesh.vertices.len(), 6 * 6);
    let overflow_bounds = pixel_bounds(&overflow_mesh.vertices, 200, 100);
    assert_close(overflow_bounds[2], 60.0, "默认溢出照画所有字形");
    assert!(
        overflow_bounds[2] > 44.0,
        "默认溢出应越过 24px 框右沿：{overflow_bounds:?}"
    );

    let mut ellipsis = text_runtime(24.0, "text=ABCDEF;font_size=12;overflow=ellipsis");
    let mesh = ellipsis.vertices(200, 100).unwrap();
    // 24px 预算装不下 "A" + "..."（7 + 21），只保留后缀三个点。
    assert_eq!(mesh.vertices.len(), 3 * 6, "省略号行应只剩三个点");
    let bounds = pixel_bounds(&mesh.vertices, 200, 100);
    assert_close(bounds[0], 20.0, "行首贴框左沿");
    assert_close(bounds[2], 39.0, "三个点的右沿");
    assert!(bounds[2] <= 44.0, "省略号不得越过框右沿：{bounds:?}");
}

#[test]
fn text_ellipsis_hit_rect_covers_the_whole_layout_box() {
    let mut runtime = text_runtime(24.0, "text=ABCDEF;font_size=12;overflow=ellipsis");
    let targets = runtime.compute_click_targets();
    let label = targets
        .iter()
        .find(|target| target.name == "Label")
        .unwrap_or_else(|| panic!("Label 应在命中集: {targets:#?}"));
    // 字形只占框内左上一小块；截断/对齐后的可点区域按 Unity 语义取整个布局矩形。
    assert_close(label.origin[0], 20.0, "命中框左沿");
    assert_close(label.origin[1], 20.0, "命中框上沿");
    assert_close(label.size[0], 24.0, "命中框宽 = 布局宽");
    assert_close(label.size[1], 40.0, "命中框高 = 布局高");
}

#[test]
fn text_voverflow_truncate_drops_whole_lines() {
    let mut runtime = text_runtime(
        120.0,
        "text=A<br/>B<br/>C<br/>D<br/>E;font_size=12;voverflow=truncate",
    );
    let mesh = runtime.vertices(200, 100).unwrap();
    // 首行 7 + 3×10 = 37 ≤ 40，第四行起放不下 ⇒ 只画四行。
    assert_eq!(mesh.vertices.len(), 4 * 6, "40px 高只放得下四行");
    let bounds = pixel_bounds(&mesh.vertices, 200, 100);
    assert_close(bounds[3], 57.0, "最后一行底部");
    assert!(bounds[3] <= 60.0, "截断后不得越过框底：{bounds:?}");

    let mut unlimited = text_runtime(120.0, "text=A<br/>B<br/>C<br/>D<br/>E;font_size=12");
    let all = unlimited.vertices(200, 100).unwrap();
    assert_eq!(all.vertices.len(), 5 * 6, "默认垂直溢出不截行");
    let all_bounds = pixel_bounds(&all.vertices, 200, 100);
    assert!(all_bounds[3] > 60.0, "五行应越过框底：{all_bounds:?}");
}

#[test]
fn text_rich_colors_bold_and_opt_out_reach_the_mesh() {
    let mut colored = text_runtime(120.0, "text=<color=#ff0000>AB</color>;font_size=12");
    let mesh = colored.vertices(200, 100).unwrap();
    assert_eq!(mesh.vertices.len(), 2 * 6, "颜色标签不产生字形");
    for vertex in &mesh.vertices {
        assert_eq!(vertex.color, [1.0, 0.0, 0.0, 1.0], "标签内的字应为红色");
    }

    let mut bold = text_runtime(120.0, "text=<b>AB</b>;font_size=12");
    let mesh = bold.vertices(200, 100).unwrap();
    assert_eq!(mesh.vertices.len(), 4 * 6, "合成粗体每个字形多一份错位副本");

    let mut literal = text_runtime(120.0, "text=<b>AB</b>;font_size=12;rich_text=false");
    let mesh = literal.vertices(200, 100).unwrap();
    assert_eq!(mesh.vertices.len(), 9 * 6, "关闭富文本后标签按普通字符排版");
    assert!(
        mesh.vertices
            .iter()
            .all(|vertex| vertex.color == [1.0, 1.0, 1.0, 1.0]),
        "关闭富文本后不应套用标签颜色"
    );
}
