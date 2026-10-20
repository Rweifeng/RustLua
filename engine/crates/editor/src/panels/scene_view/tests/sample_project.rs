//! 样例工程端到端：预设拖放落到元数据、瓦片涂改存盘后重开仍在。

use super::*;

#[test]
fn sample_project_prefab_drop_instantiates_prefab_metadata() {
    let root = copy_sample_project("prefab-drop");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    let before = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .len();

    let action = crate::dnd::plan(
        &crate::dnd::DragSource::Internal(crate::dnd::DropPayload::Asset(
            "assets/prefabs/slime_green.prefab".to_owned(),
        )),
        crate::dnd::DropZone::SceneCanvas,
    );
    assert_eq!(app.commit_spawn_drop(&action, None), "已创建 1 个实体");

    let scene = app.document.current_scene.as_ref().unwrap();
    let prefab_roots: Vec<_> = scene
        .scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
                && entity
                    .metadata
                    .get("prefab.source")
                    .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
        })
        .collect();
    assert_eq!(prefab_roots.len(), 1, "drop should create one prefab root");
    assert_eq!(prefab_roots[0].name, "SlimeGreen");
    assert!(
        scene.scene.entities.len() > before,
        "drop should add prefab entities to the scene"
    );
    assert!(
        scene.is_dirty(),
        "instantiating prefab should dirty the scene"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sample_project_tile_paint_persists_after_save_and_reopen() {
    let root = copy_sample_project("tilepaint");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/api_gameplay.scene"));
    app.sync_inspector(SceneEntityId(1));
    let width = app.tilemap.map.width;
    let x = 4usize;
    let y = 6usize;
    let index = y * width + x;

    app.tilemap.brush = 5;
    app.tilemap.paint_collision = false;
    app.apply_tile_paint_cell(x, y, false);
    app.tilemap.paint_collision = true;
    app.apply_tile_paint_cell(x, y, false);
    app.commit_tilemap_paint(SceneEntityId(1));

    let document = app.document.current_scene.as_ref().unwrap();
    let component = document.scene.entities[&SceneEntityId(1)].components["Tilemap"].clone();
    let tilemap = Tilemap::from_component_str(&component).unwrap();
    assert_eq!(tilemap.tiles[index], 5);
    assert!(
        tilemap.solid[index],
        "paint collision should persist to tilemap"
    );
    assert!(document.is_dirty(), "tile paint should dirty the scene");

    app.save_scene();
    assert_eq!(app.status, "Scene saved");
    app.open_scene_resource(PathBuf::from("scenes/api_gameplay.scene"));
    let reopened = app.document.current_scene.as_ref().unwrap();
    let reopened_component =
        reopened.scene.entities[&SceneEntityId(1)].components["Tilemap"].clone();
    let reopened_tilemap = Tilemap::from_component_str(&reopened_component).unwrap();
    assert_eq!(reopened_tilemap.tiles[index], 5);
    assert!(
        reopened_tilemap.solid[index],
        "reopened scene should preserve collision paint"
    );
    let _ = fs::remove_dir_all(root);
}
