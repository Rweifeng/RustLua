//! 原 `mod ui_circle_tests`（WP12 阶段 1 纯搬运；源行段 10412–10457）。

use super::*;

/// **回归**：`ui.circle` 的顶点必须是 triangle-list 序（每段 3 顶点）。
///
/// 怎么坏的：圆形曾按三角扇顶点序写（圆心 + 一圈环 = 34 顶点），而 2D 管线拓扑是
/// triangle list（矩形 6 顶点）——扇序被切成一个楔形 + 沿圆周的一串细片，画面上
/// 虚拟摇杆/按钮变成"轮廓圆 + 跨圆交叉细三角"，且没有任何报错。
///
/// 断言打在 [`RuntimeScene::push_ui_panels`] 而不是 `vertices()` 的 mesh 上：
/// mesh 级 `merge_adjacent_draws` 会把圆与相邻白纹理 draw 合批，单个圆的
/// 顶点数在 mesh 里看不见；这里直接取未合批的 draw 分段。
#[test]
fn circle_vertices_are_triangle_list_compatible() {
    let root = crate::core::games_dir().join("examples").join("game_api");
    let mut runtime = RuntimeScene::new(Some(&root)).expect("runtime for game_api sample");
    runtime
        .with_script(|lua| lua.execute_console("ui.circle('probe', 32, 18, 9, 1, 1, 1, 1)"))
        .expect("ui.circle probe");
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    runtime
        .push_ui_panels(&mut vertices, &mut draws, 64, 36)
        .expect("push_ui_panels");
    for draw in &draws {
        assert_eq!(
            draw.count % 3,
            0,
            "draw start={} count={} 不是 3 的倍数（triangle-list 拓扑）",
            draw.start,
            draw.count
        );
    }
    let circle = draws
        .iter()
        .find(|draw| draw.count == 96)
        .expect("圆应贡献 32 段 × 3 顶点 = 96 顶点");
    // 扇心复刻不变量：圆的每个三角形首顶点都是同一个圆心。
    let start = circle.start as usize;
    for triangle in 0..32 {
        assert_eq!(
            vertices[start + triangle * 3].position,
            vertices[start].position,
            "第 {triangle} 个三角形的首顶点必须是圆心"
        );
    }
}
