//! 文档切换闸门（E06）：脏文档的离开确认、取消 / 放弃 / 保存后离开，
//! 以及磁盘冲突与保存失败时不得吞掉脏标记。

use super::*;

/// docs/55 阶段 3 的数据安全半边：**磁盘文件被外部改动时保存必须被拒绝**。
///
/// 这条守的是一类静默数据损坏：`scenes/` 是外部工具（生成器、git checkout、另一个编辑器）
/// 会写的目录，而过去 `Ctrl+S` 无条件覆盖——别人的改动被悄悄吃掉，且**没有任何提示**。
#[test]
fn save_refuses_to_overwrite_an_externally_modified_scene() {
    let root = copy_sample_project("external-change");
    let project = open_project(&root).expect("打开工程");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.project_loaded = true;
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    let path = app
        .document
        .current_scene
        .as_ref()
        .and_then(|document| document.path.clone())
        .expect("场景应有磁盘路径");

    // ① 干净状态下保存：正常通过（不制造假冲突）。
    assert!(app.save_scene(), "无外部改动时保存必须成功：{}", app.status);

    // ② 外部改写磁盘文件（模拟生成器/git checkout）。
    let external = "scene\texternally\twritten\n";
    fs::write(&path, external).expect("外部写入");
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .expect("场景")
            .disk_status(),
        editor::DiskStatus::Modified,
        "外部写入之后必须被识别为已修改"
    );

    // ③ 普通保存被拒绝，且**磁盘内容原样保留**（这才是关键：拒绝不是提示，而是不写）。
    app.document
        .current_scene
        .as_mut()
        .expect("场景")
        .mark_dirty();
    assert!(!app.save_scene(), "外部改动后普通保存必须被拒绝");
    assert_eq!(
        fs::read_to_string(&path).expect("磁盘内容"),
        external,
        "被拒绝的保存不得改动磁盘文件"
    );
    assert!(
        app.status.contains("已阻止保存"),
        "状态栏必须说明原因：{}",
        app.status
    );
    assert!(
        app.document
            .current_scene
            .as_ref()
            .expect("场景")
            .is_dirty(),
        "被拒绝之后文档必须保持 dirty"
    );

    // ④ 显式强制保存：覆盖成功，磁盘内容变成编辑器里的场景。
    assert!(app.save_scene_force(), "强制保存必须成功：{}", app.status);
    let written = fs::read_to_string(&path).expect("磁盘内容");
    assert_ne!(written, external, "强制保存必须真的覆盖");
    assert!(
        written.contains(
            &app.document
                .current_scene
                .as_ref()
                .expect("场景")
                .scene
                .name
        ),
        "覆盖后的文件应当是被保存的场景"
    );
    // ⑤ 覆盖之后签名刷新：再保存一次不会再报冲突（否则用户会陷在"永远冲突"里）。
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .expect("场景")
            .disk_status(),
        editor::DiskStatus::Unchanged
    );
    app.document
        .current_scene
        .as_mut()
        .expect("场景")
        .mark_dirty();
    assert!(app.save_scene(), "覆盖之后应恢复正常保存：{}", app.status);
    let _ = fs::remove_dir_all(root);
}

/// 未命名文档没有磁盘基准，`disk_status()` 必须是 `Untracked` 而不是"冲突"。
#[test]
fn unnamed_document_never_reports_a_disk_conflict() {
    let root = copy_sample_project("unnamed-disk");
    let project = open_project(&root).expect("打开工程");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.project_loaded = true;
    app.create_scene();
    let document = app.document.current_scene.as_ref().expect("新场景");
    assert!(document.path.is_none());
    assert_eq!(document.disk_status(), editor::DiskStatus::Untracked);
    assert!(app.save_scene(), "新建场景必须能保存：{}", app.status);
    assert!(
        app.document
            .current_scene
            .as_ref()
            .expect("场景")
            .path
            .is_some(),
        "保存之后应获得路径"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn e06_dirty_open_is_pending_clean_open_is_immediate() {
    let (mut app, root) = open_sample_production("e06-pending");
    // 打开 production 时文档 clean：再次请求打开 menu 应立即执行，不挂起。
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_none());
    assert_eq!(app.status, "已打开场景：scenes/ui_test.scene");
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test"
    );

    // 打 dirty：请求「打开 production」被挂起，当前文档不被替换。
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    app.open_scene_resource(PathBuf::from("scenes/api_gameplay.scene"));
    assert!(app.pending_document_change.is_some());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test",
        "确认前不得切换文档"
    );

    // 已挂起 pending 时再次请求离开：被忽略，不覆盖也不重复弹窗。
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_some());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test",
        "重复请求不得覆盖挂起的离开操作"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn e06_cancel_keeps_document_and_state() {
    let (mut app, root) = open_sample_production("e06-cancel");
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    // 快照必须在 mark_dirty 之后：取消 = 保留这份 dirty 文档继续编辑。
    let doc_dirty = app.document.current_scene.as_ref().unwrap().clone();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_some());

    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Cancel);
    assert!(resolved);
    assert!(app.pending_document_change.is_none());
    assert!(!app.pending_save_failed);
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &doc_dirty,
        "取消后应保留 dirty 文档内容与状态（revision/dirty 均不变）"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn e06_discard_leaves_without_saving() {
    let (mut app, root) = open_sample_production("e06-discard");
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_some());

    // 「放弃并继续」不保存直接切换，且切换后文档 clean（来自磁盘）。
    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Discard);
    assert!(resolved);
    assert!(app.pending_document_change.is_none());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test",
        "放弃后应切到请求的场景"
    );
    assert!(!app.document.current_scene.as_ref().unwrap().is_dirty());

    let _ = fs::remove_dir_all(root);
}

/// 回归：「放弃并继续」退出时文档仍 dirty，我们发的 Close 会再触发一次
/// CloseRequested；必须置 `quit_approved` 让 guard 放行，否则重新弹窗、永远关不掉。
#[test]
fn e06_quit_discard_approves_close_echo() {
    let (mut app, root) = open_sample_production("e06-quit-discard");
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    app.pending_document_change = Some(PendingChange::QuitEditor);

    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Discard);
    assert!(resolved);
    assert!(app.pending_document_change.is_none());
    assert!(
        app.quit_approved,
        "确认退出后必须放行自己触发的 CloseRequested"
    );
    assert!(
        app.document.current_scene.as_ref().unwrap().is_dirty(),
        "放弃路径不保存：文档保持 dirty 直到进程退出"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn e06_save_success_saves_then_leaves() {
    let (mut app, root) = open_sample_production("e06-save-ok");
    // 打开 production（prefab 文档，path 指向磁盘）→ 正常「保存并继续」：
    // 保存成功 → 原文档 clean 并离开到 menu。
    let current_path = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .path
        .clone()
        .expect("磁盘打开的文档应有路径");
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_some());

    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
    assert!(resolved);
    assert!(app.pending_document_change.is_none());
    assert!(!app.pending_save_failed);
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test",
        "保存成功后应切到请求的场景"
    );

    // 原文档落盘且不再是 dirty（重新加载磁盘文件应等于保存时的内容）。
    let reloaded = editor::SceneDocument::open(&current_path).unwrap();
    assert!(!reloaded.is_dirty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn e06_save_failure_keeps_dirty_and_pending() {
    let (mut app, root) = open_sample_production("e06-save-fail");
    app.document.current_scene.as_mut().unwrap().mark_dirty();
    // 请求离开到 menu（dirty → pending；只挂起，不碰磁盘）。
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));
    assert!(app.pending_document_change.is_some());
    let doc_before = app.document.current_scene.as_ref().unwrap().clone();

    // 破坏保存目标：scenes 目录替换为同名普通文件 → 原子写 create_dir_all 必失败。
    let scenes_dir = app.project.root.join("scenes");
    let _ = fs::remove_dir_all(&scenes_dir);
    fs::write(&scenes_dir, "not a directory").unwrap();

    // 「保存并继续」保存失败：pending 保留、save_failed 置位、文档停在原地且保持 dirty。
    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
    assert!(!resolved, "保存失败时不得消费 pending");
    assert!(app.pending_document_change.is_some());
    assert!(app.pending_save_failed);
    assert!(app.document.current_scene.as_ref().unwrap().is_dirty());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &doc_before,
        "保存失败后文档不得被改动"
    );

    // 修复磁盘（恢复 scenes 目录与 menu.scene 副本）后重试「保存并继续」：
    // 保存成功 → pending 消费、切到 menu、save_failed 复位。
    let _ = fs::remove_file(&scenes_dir);
    fs::create_dir_all(&scenes_dir).unwrap();
    let sample_scenes = sample_project_root().join("scenes");
    for entry in fs::read_dir(&sample_scenes).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), scenes_dir.join(entry.file_name())).unwrap();
    }
    let resolved = app.resolve_pending_change(&test_context(), PendingDecision::Save);
    assert!(resolved);
    assert!(app.pending_document_change.is_none());
    assert!(!app.pending_save_failed);
    assert_eq!(
        app.document.current_scene.as_ref().unwrap().scene.name,
        "ui_test",
        "修复后保存成功应切到请求的场景"
    );

    let _ = fs::remove_dir_all(root);
}
