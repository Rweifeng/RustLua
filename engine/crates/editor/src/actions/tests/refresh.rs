//! 统一刷新路由：换场景 / 删除 / 撤销重做 / 改名 / 重贴预设 / 停止播放之后，
//! Scene 视图缓存与元数据缓存都必须被同一条路由失效（不许各面板自己记）。

use super::*;

#[test]
fn open_scene_resource_invalidates_scene_view_state() {
    let root = copy_sample_project("scene-switch");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.scene_runtime_revision = Some(0);
    app.scene_view_cache_2d = SceneViewCache2d {
        revision: Some(0),
        size: (320, 180),
        camera: (eframe::egui::vec2(12.0, -8.0), 1.5),
    };
    app.scene_view_cache_3d = SceneViewCache3d {
        revision: Some(0),
        size: (640, 360),
        camera: EditorCamera::default(),
    };
    app.scene_state
        .preview
        .insert(SceneEntityId(1), TransformFields::default());
    app.scene_state
        .preview3d_transform
        .insert(SceneEntityId(1), Transform3D::default());
    app.preview_diagnostic = Some("stale frame".to_owned());

    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));

    assert_eq!(app.scene_runtime_revision, None);
    assert_eq!(app.scene_view_cache_2d.revision, None);
    assert_eq!(app.scene_view_cache_2d.size, (0, 0));
    assert_eq!(app.scene_view_cache_3d.revision, None);
    assert_eq!(app.scene_view_cache_3d.size, (0, 0));
    assert!(app.scene_state.preview.is_empty());
    assert!(app.scene_state.preview3d_transform.is_empty());
    assert_eq!(app.preview_diagnostic, None);
    assert!(app.scene_view_repaint_pending);

    let _ = fs::remove_dir_all(root);
}

/// WP1-4：3D 视图的 typed 提取按文档 revision 缓存，并且切文档后必须失效——
/// 否则新文档会沿用上一份文档的 3D 实体列表（两个文档 revision 都可能为 0）。
#[test]
fn scene_view_3d_typed_cache_follows_document_and_resets_on_switch() {
    let root = copy_sample_project("wp1-04-typed");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));

    app.ensure_scene_view_3d_typed();
    assert!(
        !app.scene_view_3d_typed.entities.is_empty(),
        "demo3d 应提取出 3D 实体"
    );
    assert!(app.scene_view_3d_typed.diagnostics.is_empty());
    assert_eq!(
        app.scene_view_3d_typed.revision,
        app.document.current_scene.as_ref().map(|doc| doc.revision)
    );

    // 同一 revision 下不重算：清空缓存内容当探针，再调用必须保持清空。
    app.scene_view_3d_typed.entities.clear();
    app.ensure_scene_view_3d_typed();
    assert!(
        app.scene_view_3d_typed.entities.is_empty(),
        "revision 未变时必须命中缓存，不得重新提取"
    );

    // 切换文档 → 缓存整体失效。文档此时是干净的，否则会被 E06 未保存保护拦下、
    // 实际不切文档（那样断言的就是另一回事）。
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert_eq!(app.scene_view_3d_typed.revision, None);
    assert!(app.scene_view_3d_typed.entities.is_empty());

    // 重新打开 demo3d 并编辑：revision 自增后必须重新提取（同样清空当探针）。
    app.open_scene_resource(PathBuf::from("scenes/demo3d.scene"));
    app.ensure_scene_view_3d_typed();
    assert!(!app.scene_view_3d_typed.entities.is_empty());
    let before = app.scene_view_3d_typed.revision;
    app.scene_view_3d_typed.entities.clear();
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    assert_ne!(
        app.document.current_scene.as_ref().map(|doc| doc.revision),
        before
    );
    app.ensure_scene_view_3d_typed();
    assert!(
        !app.scene_view_3d_typed.entities.is_empty(),
        "revision 变化后必须重新提取"
    );

    let _ = fs::remove_dir_all(root);
}

/// 回归：同文档编辑提交（属性直写的事务提交）不得重置 Scene 视图的 zoom/pan，
/// 也不得置 fit_pending 把滚轮设置的视图状态覆盖回 fit 值；文档整表切换
/// （重新打开场景）则必须重新适配。
#[test]
fn edit_commit_preserves_scene_view_camera_but_reopen_refits() {
    let (mut app, root) = open_examples_main("zoom-preserve");
    // 清掉打开场景时的初始适配请求，聚焦「编辑提交」是否再次触发。
    app.scene_view_fit_pending = false;
    // 用户滚轮/拖拽后的视图状态。
    app.scene_zoom = 1.6;
    app.scene_state.pan = egui::vec2(42.0, -17.0);

    // 属性编辑提交路径：开事务 → 直写 → 提交（等价 Inspector 数值编辑松手）。
    app.selection
        .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
    app.begin_edit_transaction(
        "Inspector",
        editor::CommandKind::Property,
        Some(SceneEntityId(2)),
    );
    if let Some(entity) = app
        .document
        .current_scene
        .as_mut()
        .and_then(|doc| doc.scene.entities.get_mut(&SceneEntityId(2)))
    {
        entity.name = "ZoomPreserved".to_owned();
    }
    assert!(app.commit_edit_transaction(), "应有净差异提交");
    assert!(
        !app.scene_view_fit_pending,
        "同文档编辑提交不得触发视图适配"
    );
    assert_eq!(app.scene_zoom, 1.6, "编辑提交不得重置 zoom");
    assert_eq!(app.scene_state.pan, egui::vec2(42.0, -17.0));

    // 文档整表切换（重新打开场景）：必须重新适配视图。先撤销恢复 clean，
    // 否则 open 会走 dirty 挂起确认而不执行。
    app.undo();
    app.scene_zoom = 1.6;
    app.open_scene_resource(std::path::PathBuf::from("scenes/main.scene"));
    assert!(app.scene_view_fit_pending, "整表切换应触发视图适配");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn examples_main_open_scene_routes_unified_refresh() {
    let (mut app, root) = open_examples_main("examples-open");
    let entity_count = {
        let scene = app.document.current_scene.clone().expect("已打开场景");
        let count = scene.scene.entities.len();
        assert!(
            count >= 8,
            "示例 main.scene 应当有足够多的实体，实际 {count}"
        );
        assert!(
            scene
                .scene
                .entities
                .values()
                .any(|entity| entity.parent.is_some()),
            "示例 main.scene 应当有父子关系（Hierarchy 面板与传播路径的样本）"
        );
        count
    };
    assert_eq!(app.status, "已打开场景：scenes/main.scene");
    assert!(app.selection.is_empty());

    // 在已打开的文档上制造悬空选择、陈旧缓存与旧的 Hierarchy 元数据 key。
    app.selection
        .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
    app.delete_target = crate::DeleteTarget::Hierarchy;
    poison_scene_view_caches(&mut app);

    // 再次打开同一场景：Opened 属整表切换，必须无条件清空选择并全量刷新。
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len(),
        entity_count,
        "重开同一场景应当得到同一份实体集合"
    );
    assert!(app.selection.is_empty());
    assert_eq!(app.selection.primary(), None);
    assert!(matches!(app.delete_target, crate::DeleteTarget::None));
    assert_scene_view_and_metadata_invalidated(&app);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn examples_main_delete_selected_root_routes_unified_refresh() {
    let (mut app, root) = open_examples_main("examples-delete");
    let target = first_root_entity(&app);
    let before = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .len();
    app.selection
        .set_single(target, editor::SelectionSource::Hierarchy);
    app.delete_target = crate::DeleteTarget::Hierarchy;
    poison_scene_view_caches(&mut app);

    // 删除根实体：删除成功后统一入口必须清理悬空选择与陈旧缓存。
    app.delete_selected_entity();
    assert_eq!(app.status, "实体已删除");
    let scene = app.document.current_scene.as_ref().unwrap();
    assert_eq!(
        scene.scene.entities.len(),
        before - 1,
        "删除根实体（连同子树）"
    );
    assert!(!scene.scene.entities.contains_key(&target));
    assert!(app.selection.is_empty());
    assert_eq!(app.selection.primary(), None);
    assert!(matches!(app.delete_target, crate::DeleteTarget::None));
    assert_scene_view_and_metadata_invalidated(&app);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn examples_main_undo_redo_routes_unified_refresh() {
    let (mut app, root) = open_examples_main("examples-undo");
    let target = first_root_entity(&app);
    let before = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .len();
    app.selection
        .set_single(target, editor::SelectionSource::Hierarchy);
    poison_scene_view_caches(&mut app);

    app.delete_selected_entity();
    assert_eq!(app.status, "实体已删除");
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len(),
        before - 1
    );
    assert_scene_view_and_metadata_invalidated(&app);

    // Undo 走统一入口：整文档回到删除前快照，Selection/缓存一并收敛。
    app.undo();
    assert_eq!(app.status, "撤销：删除实体");
    let scene = app.document.current_scene.as_ref().unwrap();
    assert_eq!(scene.scene.entities.len(), before);
    assert!(scene.scene.entities.contains_key(&target));
    assert!(app.selection.is_empty());
    assert_eq!(app.selection.primary(), None);
    assert_scene_view_and_metadata_invalidated(&app);

    // Redo 同样经统一入口：删除再次生效。
    app.redo();
    assert_eq!(app.status, "重做：删除实体");
    let scene = app.document.current_scene.as_ref().unwrap();
    assert_eq!(scene.scene.entities.len(), before - 1);
    assert!(!scene.scene.entities.contains_key(&target));
    assert!(app.selection.is_empty());
    assert_eq!(app.selection.primary(), None);
    assert_scene_view_and_metadata_invalidated(&app);

    let _ = fs::remove_dir_all(root);
}

/// E02 Rename 收敛：重命名走统一入口（同一文档内编辑，不整表切换），
/// 选中实体保持有效并随后同步 Inspector，同时清理陈旧 Scene View 缓存与
/// Hierarchy 元数据 key。
#[test]
fn game_production_rename_selected_routes_unified_refresh() {
    let (mut app, root) = open_sample_production("rename");
    let scene = app.document.current_scene.clone().unwrap();
    assert!(scene.scene.entities.contains_key(&SceneEntityId(2)));
    app.selection
        .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
    app.sync_inspector(SceneEntityId(2));
    poison_scene_view_caches(&mut app);

    app.entity_name = "HeroRenamed".to_owned();
    app.rename_selected();

    assert_eq!(app.status, "实体已重命名");
    let scene = app.document.current_scene.as_ref().unwrap();
    assert_eq!(
        scene.scene.entities.get(&SceneEntityId(2)).unwrap().name,
        "HeroRenamed"
    );
    // 选中实体仍在同一文档内 → 选择保留而非清空，Inspector 主源已同步新名。
    assert_eq!(app.selection.primary(), Some(SceneEntityId(2)));
    assert_eq!(app.entity_name, "HeroRenamed");
    assert_scene_view_and_metadata_invalidated(&app);

    let _ = fs::remove_dir_all(root);
}

/// E02 Prefab Apply 收敛：重新应用源预制体 会把整棵实例替换为重建的新实体，
/// 统一入口必须让选择指向新的根 ID、保持其仍有效，并清理陈旧缓存与元数据。
#[test]
fn game_production_reapply_prefab_routes_unified_refresh() {
    let (mut app, root) = open_sample_production("prefab-apply");
    let old_root = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        scene
            .entities
            .values()
            .find(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
                    && entity
                        .metadata
                        .get("prefab.source")
                        .is_some_and(|value| value == "assets/prefabs/slime_green.prefab")
            })
            .unwrap()
            .id
    };
    app.selection
        .set_single(old_root, editor::SelectionSource::Inspector);
    app.sync_inspector(old_root);
    poison_scene_view_caches(&mut app);

    assert_eq!(
        app.dispatch(editor::EditorCommand::ReapplyPrefab { entity: old_root }),
        editor::CommandResult::Applied
    );
    assert!(
        app.status.contains("已重新应用预制体"),
        "status should confirm prefab reapply, got: {}",
        app.status
    );
    let new_root = app.selection.primary().unwrap();
    assert_ne!(
        new_root, old_root,
        "reapply rebuilds instance so selection must point at the new root"
    );
    let scene = &app.document.current_scene.as_ref().unwrap().scene;
    assert!(scene.entities.contains_key(&new_root));
    assert_eq!(
        scene
            .entities
            .get(&new_root)
            .unwrap()
            .metadata
            .get("prefab.source")
            .map(String::as_str),
        Some("assets/prefabs/slime_green.prefab")
    );
    assert!(
        !scene.entities.contains_key(&old_root),
        "old instance root must be removed"
    );
    assert_scene_view_and_metadata_invalidated(&app);

    let _ = fs::remove_dir_all(root);
}

/// E02 运行时修改应用（PlaybackStopped）收敛：Stop 丢弃运行态改动并恢复编辑器
/// 快照，选择无条件清空、delete_target 复位，场景/Game 缓存与 Hierarchy 元数据
/// 一并失效。
#[test]
fn game_production_playback_stop_routes_unified_refresh() {
    use crate::WorkspaceTab;
    use editor::PlaySession;
    use editor::PlayState;

    let (mut app, root) = open_sample_production("playback-stop");
    let original = app.document.current_scene.as_ref().unwrap().clone();

    // 模拟运行中：会话持有编辑器快照，运行时文档被脚本改动（如增删实体）。
    app.play_session = Some(PlaySession::start(&original));
    app.play.set_state_for_test(PlayState::Playing);
    app.workspace_tab = WorkspaceTab::Game;
    app.preview_input_state.clear_all();
    // 运行期脚本删掉一个实例（20），并选中一个实例（10）作为运行期选区。
    app.play_session
        .as_mut()
        .unwrap()
        .runtime_document_mut()
        .scene
        .entities
        .remove(&SceneEntityId(20));
    app.selection
        .set_single(SceneEntityId(10), editor::SelectionSource::Playback);
    poison_scene_view_caches(&mut app);

    app.stop();

    assert_eq!(app.play_state(), PlayState::Editing);
    assert!(app.play_session.is_none());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &original,
        "stop must restore the untouched editor snapshot"
    );
    assert!(app.selection.is_empty());
    assert_eq!(app.selection.primary(), None);
    assert!(matches!(app.delete_target, crate::DeleteTarget::None));
    assert_scene_view_and_metadata_invalidated(&app);
    assert_eq!(app.status, "运行已停止，运行期改动已丢弃");

    let _ = fs::remove_dir_all(root);
}

// ---- E06 未保存切换保护 ----
