//! 3D 场景视图的覆盖层：实体标记/名字的显示开关只影响画层，不动物体列表。

use super::*;

/// 画一帧 3D 场景视图（不经过 dock，避免层级面板里的同名文本混进读数），
/// 返回这一帧画出的所有文本。
fn draw_3d_view_texts(app: &mut EditorApp) -> Vec<String> {
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        ..Default::default()
    };
    let output = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| app.draw_scene_view_3d(ui));
    });
    output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn entity_label_overlay_toggle_hides_3d_names_only() {
    let root = copy_sample_project("entity-labels");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));
    app.scene_view_mode = crate::SceneViewMode::View3D;

    let shown = draw_3d_view_texts(&mut app);
    let entities = app.scene_view_3d_typed.entities.len();
    assert!(entities >= 2, "样例 3D 场景必须有可标名的实体");
    assert!(
        shown.iter().any(|text| text == "SkeletonWarrior"),
        "默认应画出实体名：{shown:?}"
    );

    app.show_entity_labels = false;
    let hidden = draw_3d_view_texts(&mut app);
    assert!(
        !hidden.iter().any(|text| text == "SkeletonWarrior"),
        "关掉覆盖层后不应再画实体名：{hidden:?}"
    );
    // 画层关掉不动数据源：点选与 Gizmo 仍吃这一份列表。
    assert_eq!(app.scene_view_3d_typed.entities.len(), entities);
    let _ = fs::remove_dir_all(root);
}
