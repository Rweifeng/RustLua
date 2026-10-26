//! P3-E02 材质编辑段的编辑器测试（`panels/material.rs` 的子模块）。
//!
//! 单独放文件是为了让骨架保持可读；`#[path]` 挂回去后仍是 `material` 模块的子模块，
//! 可以直接检查私有状态机。

use super::*;
use editor::{ProjectInfo, SceneDocument, SelectionSource};
use rustlua_infra::logging::LogSink;

const MATERIAL: &str = "\
version=1
name=rusted
base_color=0.8,0.7,0.6,1
metallic=1
roughness=0.35
base_color_texture=assets/textures/rust.png
";

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-material-editor-{label}-{}-{}",
        std::process::id(),
        crate::util::test_token()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("assets/materials")).expect("material dir");
    std::fs::create_dir_all(root.join("assets/textures")).expect("texture dir");
    std::fs::write(root.join("assets/textures/rust.png"), b"png").expect("texture");
    std::fs::write(root.join("assets/materials/rusted.material"), MATERIAL).expect("material");
    root
}

fn app_with_mesh(label: &str, component: &str) -> (EditorApp, PathBuf) {
    let root = temp_root(label);
    let mut app = EditorApp::new(
        ProjectInfo {
            root: root.clone(),
            version: "test".to_owned(),
            directories: Vec::new(),
        },
        None,
        LogSink::new(16),
    );
    let mut document = SceneDocument::new("material-test");
    document.dirty = false;
    document
        .scene
        .add_entity(SceneEntityId(1), "ball", None)
        .expect("entity");
    document
        .scene
        .entities
        .get_mut(&SceneEntityId(1))
        .expect("entity")
        .components
        .insert(COMPONENT.to_owned(), component.to_owned());
    app.document.current_scene = Some(document);
    app.selection
        .set_single(SceneEntityId(1), SelectionSource::Command);
    (app, root)
}

fn ready(app: &EditorApp) -> &MaterialPreview {
    match &app.material.status {
        MaterialStatus::Ready(preview) => preview,
        other => panic!("期望材质可编辑状态，实际 {other:?}"),
    }
}

fn ready_mut(app: &mut EditorApp) -> &mut MaterialPreview {
    match &mut app.material.status {
        MaterialStatus::Ready(preview) => preview,
        other => panic!("期望材质可编辑状态，实际 {other:?}"),
    }
}

#[test]
fn material_editor_loads_the_override_of_the_selected_mesh() {
    let (mut app, _root) = app_with_mesh(
        "load",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    let preview = ready(&app);
    assert_eq!(preview.entity, SceneEntityId(1));
    assert_eq!(preview.entity_name, "ball");
    assert_eq!(preview.material_path, "assets/materials/rusted.material");
    assert_eq!(preview.session.material().metallic, 1.0);
    assert!(!preview.session.is_dirty());
    assert!(
        preview
            .textures
            .iter()
            .any(|path| path == "assets/textures/rust.png"),
        "贴图选择器应列出工程图片：{:?}",
        preview.textures
    );
}

#[test]
fn material_editor_previews_edits_and_invalidates_the_scene_view() {
    let (mut app, _root) = app_with_mesh(
        "preview",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    app.refresh_material_preview();
    let (path, material) = app.material.preview().expect("加载后应有预览");
    assert_eq!(path, "assets/materials/rusted.material");
    assert_eq!(material.base_color, [0.8, 0.7, 0.6, 1.0]);

    let document_before = app
        .document
        .current_scene
        .as_ref()
        .expect("document")
        .scene
        .to_text();
    // 模拟"上一帧渲染过"的 3D 缓存；帧间的一次编辑必须让它失效。
    app.scene_view_cache_3d = crate::SceneViewCache3d {
        revision: Some(7),
        size: (96, 96),
        camera: crate::EditorCamera::default(),
    };
    ready_mut(&mut app)
        .session
        .edit(|material| material.base_color = [0.2, 0.4, 0.8, 1.0])
        .expect("编辑基色");
    app.refresh_material_preview();
    assert_eq!(
        app.scene_view_cache_3d.revision, None,
        "预览内容变化后 3D 视图缓存必须失效"
    );
    let (_, material) = app.material.preview().expect("编辑后仍有预览");
    assert_eq!(material.base_color, [0.2, 0.4, 0.8, 1.0]);
    let document_after = app
        .document
        .current_scene
        .as_ref()
        .expect("document")
        .scene
        .to_text();
    assert_eq!(document_after, document_before, "预览不得写场景文档");
}

#[test]
fn material_editor_create_override_writes_file_and_binds_it() {
    let (mut app, root) = app_with_mesh("create", "mesh=assets/models/ball.glb");
    app.sync_material_from_selection();
    let MaterialStatus::NoOverride {
        entity,
        name,
        suggested,
        ..
    } = &app.material.status
    else {
        panic!("没有 material= 时应是 NoOverride");
    };
    let (entity, name, suggested) = (*entity, name.clone(), suggested.clone());
    let intent = create_override(entity, &name, &suggested, &root).expect("创建覆盖");
    let MaterialIntent::AssignOverride { path, .. } = &intent else {
        panic!("创建覆盖必须返回写回 action");
    };
    assert!(root.join(path).is_file(), "覆盖文件必须已原子写入");
    assert!(!root.join(path).with_extension("material.tmp").exists());

    let result = app.dispatch(EditorCommand::SetComponentField {
        entity,
        component: COMPONENT.to_owned(),
        field: "material".to_owned(),
        value: path.clone(),
    });
    assert!(
        matches!(result, editor::CommandResult::Applied),
        "{result:?}"
    );
    app.material.sync(None, &root);
    app.sync_material_from_selection();
    assert_eq!(ready(&app).material_path, *path);
    assert_eq!(ready(&app).session.material().name.as_deref(), Some("ball"));
}

#[test]
fn material_editor_edits_save_atomically_and_reload() {
    let (mut app, root) = app_with_mesh(
        "save",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    ready_mut(&mut app).apply(false, |material| material.roughness = 0.9);
    assert!(ready(&app).session.is_dirty());
    assert!(ready_mut(&mut app).save_to_disk());
    assert!(!ready(&app).session.is_dirty());
    let text = std::fs::read_to_string(root.join("assets/materials/rusted.material")).unwrap();
    assert!(text.contains("roughness=0.9"), "{text}");
    assert!(!root.join("assets/materials/rusted.material.tmp").exists());

    app.material.sync(None, &root);
    app.sync_material_from_selection();
    assert_eq!(ready(&app).session.material().roughness, 0.9);
    assert!(!ready(&app).session.is_dirty());
}

#[test]
fn material_editor_rejects_invalid_values_without_writing_disk() {
    let (mut app, root) = app_with_mesh(
        "invalid",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    let before = std::fs::read(root.join("assets/materials/rusted.material")).unwrap();
    ready_mut(&mut app).apply(false, |material| material.metallic = 9.0);
    let preview = ready(&app);
    assert_eq!(preview.session.material().metallic, 1.0);
    assert!(!preview.session.is_dirty());
    let (ok, message) = preview.message.as_ref().expect("拒绝必须有提示");
    assert!(!ok);
    assert!(message.contains("metallic"), "{message}");
    assert_eq!(
        std::fs::read(root.join("assets/materials/rusted.material")).unwrap(),
        before,
        "被拒绝的修改不得写盘"
    );
}

#[test]
fn material_editor_panel_never_writes_the_scene_document() {
    let (mut app, _root) = app_with_mesh(
        "no-doc",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    ready_mut(&mut app).apply(false, |material| material.double_sided = true);
    let before = {
        let document = app.current_scene().expect("scene");
        (document.scene.to_text(), document.dirty, document.revision)
    };
    let context = egui::Context::default();
    let _ = context.run(egui::RawInput::default(), |context| {
        egui::CentralPanel::default().show(context, |ui| {
            app.draw_material_section(ui);
        });
    });
    let after = {
        let document = app.current_scene().expect("scene");
        (document.scene.to_text(), document.dirty, document.revision)
    };
    assert_eq!(before, after, "材质面板不得改动场景文档");
}

#[test]
fn material_editor_dirty_session_blocks_selection_switch() {
    let (mut app, root) = app_with_mesh(
        "switch",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    std::fs::write(
        root.join("assets/materials/other.material"),
        MATERIAL.replace("name=rusted", "name=other"),
    )
    .unwrap();
    {
        let document = app.document.current_scene.as_mut().expect("scene");
        document
            .scene
            .add_entity(SceneEntityId(2), "other", None)
            .unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                COMPONENT.to_owned(),
                "mesh=assets/models/ball.glb;material=assets/materials/other.material".to_owned(),
            );
    }
    app.sync_material_from_selection();
    ready_mut(&mut app).apply(false, |material| material.roughness = 0.2);
    app.selection
        .set_single(SceneEntityId(2), SelectionSource::Command);
    app.sync_material_from_selection();
    let preview = ready(&app);
    assert_eq!(preview.entity, SceneEntityId(1), "脏会话不能静默切走");
    assert!(preview.switch_blocked);

    ready_mut(&mut app)
        .session
        .revert_to_saved()
        .expect("revert");
    app.sync_material_from_selection();
    assert_eq!(ready(&app).entity, SceneEntityId(2));
}

#[test]
fn material_editor_reports_broken_files_with_line_numbers() {
    let (mut app, root) = app_with_mesh(
        "broken",
        "mesh=assets/models/ball.glb;material=assets/materials/bad.material",
    );
    std::fs::write(
        root.join("assets/materials/bad.material"),
        "version=1\nroughness=oops\n",
    )
    .unwrap();
    app.sync_material_from_selection();
    let MaterialStatus::Error(error) = &app.material.status else {
        panic!("坏材质必须进入 Error");
    };
    assert!(error.contains("line 2"), "{error}");
}

#[test]
fn material_editor_clear_override_returns_to_the_embedded_material() {
    let (mut app, root) = app_with_mesh(
        "clear",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    let result = app.dispatch(EditorCommand::ClearMeshRendererMaterial {
        entity: SceneEntityId(1),
    });
    assert!(
        matches!(result, editor::CommandResult::Applied),
        "{result:?}"
    );
    let payload = app
        .current_scene()
        .unwrap()
        .scene
        .entities
        .get(&SceneEntityId(1))
        .unwrap()
        .components
        .get(COMPONENT)
        .unwrap()
        .clone();
    assert_eq!(payload, "mesh=assets/models/ball.glb");
    app.material.sync(None, &root);
    app.sync_material_from_selection();
    assert!(matches!(
        app.material.status,
        MaterialStatus::NoOverride { .. }
    ));
}

#[test]
fn material_editor_save_as_writes_a_new_file_and_rebinds() {
    let (mut app, root) = app_with_mesh(
        "save-as",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    let original =
        std::fs::read_to_string(root.join("assets/materials/rusted.material")).expect("原文件");
    let preview = ready_mut(&mut app);
    preview
        .session
        .edit(|material| material.metallic = 0.9)
        .expect("编辑");
    let intent = preview
        .save_as("assets/materials/ball_copy.material")
        .expect("另存为");
    assert_eq!(
        intent,
        MaterialIntent::AssignOverride {
            entity: SceneEntityId(1),
            path: "assets/materials/ball_copy.material".to_owned(),
        }
    );
    assert_eq!(preview.material_path, "assets/materials/ball_copy.material");
    assert!(!preview.session.is_dirty(), "另存为成功即到达保存点");
    let copy = editor::material_ops::load_material(&root, "assets/materials/ball_copy.material")
        .expect("新文件可解析");
    assert_eq!(copy.metallic, 0.9);
    assert_eq!(
        std::fs::read_to_string(root.join("assets/materials/rusted.material")).unwrap(),
        original,
        "另存为不得改动原文件"
    );
    let error = preview
        .save_as("assets/materials/rusted.material")
        .expect_err("已存在的目标必须拒绝");
    assert!(error.contains("已存在"), "{error}");
}

#[test]
fn material_editor_double_click_selects_the_referencing_mesh_renderer() {
    let (mut app, _root) = app_with_mesh(
        "locate",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    let scene = &mut app.document.current_scene.as_mut().unwrap().scene;
    scene
        .add_entity(SceneEntityId(2), "other", None)
        .expect("second entity");
    app.selection
        .set_single(SceneEntityId(2), SelectionSource::Command);
    app.open_material_resource("assets/materials/rusted.material");
    assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
    assert!(app.status.contains("已打开材质"), "{}", app.status);
    app.open_material_resource("assets/materials/absent.material");
    assert_eq!(
        app.selection.primary(),
        Some(SceneEntityId(1)),
        "找不到引用者时不应改选中"
    );
    assert!(app.status.contains("没有引用"), "{}", app.status);
}

/// 材质此前只有面板按钮能撤（Ctrl+Z 全归场景栈）。接进全局账本后（M79 P1-2），
/// 在材质里改完一个数值直接按 Ctrl+Z，撤的是这一步而不是场景的某一步；
/// 两条栈互不共享——场景那一步要等材质栈空了才轮到。
#[test]
fn material_edit_joins_the_global_undo_stack() {
    let (mut app, _root) = app_with_mesh(
        "global-undo",
        "mesh=assets/models/ball.glb;material=assets/materials/rusted.material",
    );
    app.sync_material_from_selection();
    assert_eq!(app.material.undo_signature(), (0, 0), "刚同步不该带历史");

    // 场景先提交一步，材质再提交一步：账本应按"最近提交"把材质排在前面。
    // 面板写入类命令有一道"工程未加载"闸门（见 `command_router.rs` 的 CreateEntity 分支），
    // 这里要的是"场景真的提交了一步"，所以把闸门打开。
    app.project_loaded = true;
    assert_eq!(
        app.dispatch(EditorCommand::CreateEntity {
            name: "Other".to_owned(),
            parent: None,
            transform_3d: false,
        }),
        editor::CommandResult::Applied
    );
    app.poll_undo_owners();
    ready_mut(&mut app)
        .session
        .edit(|material| material.roughness = 0.11)
        .expect("编辑粗糙度");
    app.poll_undo_owners();
    assert_eq!(app.material.undo_signature().1, 1, "材质应有一条可撤历史");

    assert_eq!(
        app.dispatch(EditorCommand::Undo),
        editor::CommandResult::Applied
    );
    assert_eq!(
        ready(&app).session.material().roughness,
        0.35,
        "第一次按键撤的是材质那一步"
    );
    assert_eq!(
        app.history.undo_len(),
        1,
        "同一次按键不得顺手撤掉场景的一步"
    );
    assert_eq!(app.history.redo_len(), 0);
    assert!(app.status.contains("材质"), "{}", app.status);

    // 材质栈空了，第二次按键才轮到场景。
    assert_eq!(
        app.dispatch(EditorCommand::Undo),
        editor::CommandResult::Applied
    );
    assert_eq!(app.history.undo_len(), 0);
    assert_eq!(app.history.redo_len(), 1);

    // 重做回到"刚撤掉的那个归属"（场景），材质栈原地不动——这就是各归属
    // 自己收场的代价：跨面板的相对顺序只按提交时刻排，不按撤销时刻重排。
    assert_eq!(
        app.dispatch(EditorCommand::Redo),
        editor::CommandResult::Applied
    );
    assert_eq!(app.history.undo_len(), 1);
    assert_eq!(
        ready(&app).session.material().roughness,
        0.35,
        "重做撤掉的是场景那一步，材质不得跟着复活"
    );
    assert!(
        app.material.undo_can_redo(),
        "材质的那一步应留在它自己的 redo 栈里"
    );
}
