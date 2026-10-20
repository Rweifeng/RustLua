//! 编辑事务与撤销（E07）与 Inspector 脏编辑（E08）：一次交互只产生一条历史、
//! 撤销守卫先收尾再回退，Escape 取消后不留残留。

use super::*;

/// E07：App 层事务网关。直接对历史提交事务一次连续编辑（模拟多帧预览写入），
/// 最终只产生一条 Undo、dirty 一次、提交/取消状态机正确。
#[test]
fn e07_commit_transaction_produces_single_undo_and_dirty_once() {
    let (mut app, root) = open_examples_main("e07-commit");
    let id = SceneEntityId(2);
    let began = app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    );
    assert!(began, "事务应成功开始");
    assert!(app.is_edit_transaction_active());
    let before = app.document.current_scene.clone().unwrap();

    // 多次更新（模拟拖拽帧），仅更新文档内容，不调用 save_scene。
    for frame in 0..5 {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("Transform".to_owned(), format!("position={frame},0"));
        scene.mark_dirty();
    }
    assert_eq!(app.history.undo_len(), 0, "事务期间不得追加历史（未提交）");

    app.commit_edit_transaction();
    assert!(!app.is_edit_transaction_active());
    assert_eq!(app.history.undo_len(), 1, "一次连续编辑只产生一条 Undo");

    // Undo：整文档回到事务开始前（单条快照模型 = before）。
    app.undo();
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &before,
        "Undo 应回到事务开始前内容"
    );
    // Redo：回到事务结束内容。
    app.redo();
    let after = app.document.current_scene.as_ref().unwrap();
    assert_eq!(
        after.scene.entities[&id].components["Transform"],
        "position=4,0"
    );
    let _ = fs::remove_dir_all(root);
}

/// E07：取消事务 → 无历史、无 dirty（干净文档上取消后仍是 clean），
/// 文档内容回退到 begin 前。
#[test]
fn e07_cancel_transaction_no_history_no_dirty() {
    let (mut app, root) = open_examples_main("e07-cancel");
    let id = SceneEntityId(2);
    // 打开文档为 clean，作为 dirty 判定基线。
    assert!(!app.document.current_scene.as_ref().unwrap().is_dirty());
    let began = app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    );
    assert!(began, "事务应成功开始");
    let before = app.document.current_scene.clone().unwrap();
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("Transform".to_owned(), "position=99,99".to_owned());
        scene.mark_dirty();
    }

    app.cancel_edit_transaction();
    assert!(!app.is_edit_transaction_active());
    assert_eq!(app.history.undo_len(), 0, "取消不产生历史");
    assert!(
        !app.document.current_scene.as_ref().unwrap().is_dirty(),
        "取消不标记 Dirty"
    );
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &before,
        "取消恢复事务开始前状态"
    );
    let _ = fs::remove_dir_all(root);
}

/// E07：Undo 快捷键先取消活动事务（不产生历史/不改 dirty），再撤回到上一条
/// 历史；活动事务不把预览内容泄漏成一条 Undo。
#[test]
fn e07_undo_guard_cancels_active_transaction_then_undoes() {
    let (mut app, root) = open_examples_main("e07-undo-guard");
    let id = SceneEntityId(2);
    let pristine = app.document.current_scene.clone().unwrap();
    // 先做一条离散编辑（产生 1 条历史、dirty）。
    app.selection
        .set_single(id, editor::SelectionSource::Hierarchy);
    app.entity_name = "RenamedByTest".to_owned();
    app.rename_selected();
    assert_eq!(app.history.undo_len(), 1);
    let after_rename = app.document.current_scene.clone().unwrap();
    assert_ne!(&after_rename, &pristine);

    // Begin 事务并写入内容。
    let began = app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    );
    assert!(began, "事务应成功开始");
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("Transform".to_owned(), "position=5,5".to_owned());
        scene.mark_dirty();
    }
    assert_eq!(app.history.undo_len(), 1, "事务内容尚未成为历史");

    // 撤销：先取消事务（回到 rename 后），再撤 rename。
    app.undo_with_transaction_guard();
    assert!(!app.is_edit_transaction_active());
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &pristine,
        "取消事务并撤销 rename 后应回到最开始的文档"
    );
    let _ = fs::remove_dir_all(root);
}

/// E07：保存时若仍有活动事务，自动以单条历史落盘（save_scene 前兜底提交）。
#[test]
fn e07_save_flushes_active_transaction() {
    let (mut app, root) = open_examples_main("e07-save-flush");
    let id = SceneEntityId(2);
    let began = app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    );
    assert!(began, "事务应成功开始");
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("Transform".to_owned(), "position=3,3".to_owned());
        scene.mark_dirty();
    }
    assert!(app.is_edit_transaction_active());
    assert!(app.save_scene());
    assert!(!app.is_edit_transaction_active(), "保存后事务必须被消费");
    assert_eq!(app.history.undo_len(), 1, "保存前自动提交单条历史");
    let _ = fs::remove_dir_all(root);
}

// ── E08：Inspector 直写/提交边界 ─────────────────────────────────────────
// 体验标准：拖动中磁盘不持续变化、一次拖拽一条历史；Inspector 输入提交后
// 可撤销、不产生空操作；Escape 取消后不留拖拽残留。

/// E08：Inspector 缓冲编辑先 `mark`（直写不写历史）再 `finish`（收敛一条
/// 历史）；再次 finish 无差异 → no-op 不产生空历史；撤销一次回到编辑前。
#[test]
fn e08_inspector_mark_then_finish_yields_single_history_and_undo() {
    let (mut app, root) = open_sample_production("e08-inspector-mark-finish");
    // `Animation` 面板是仍在**缓冲**里的组件（Transform/Text/UiLayout 已迁到 schema 字段
    // 编辑器），因此这条测试挑一个**真的有 Animation 组件**的实体，而不是写死 id 2：
    // 示例工程的场景会随演示增删而变化，"id 2 必须是动画实体"只是历史巧合。
    let (id, default_clip) = {
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        let entity = scene
            .entities
            .values()
            .find(|entity| entity.components.contains_key("Animation"))
            .expect("示例场景应当至少有一个带 Animation 的实体");
        let machine = AnimationStateMachine::from_component_str(&entity.components["Animation"])
            .expect("Animation 组件串应当可解析");
        (entity.id, machine.default)
    };
    app.selection
        .set_single(id, editor::SelectionSource::Hierarchy);
    app.sync_inspector(id);
    let pristine = app.document.current_scene.clone().unwrap();
    assert_eq!(app.history.undo_len(), 0);
    assert_eq!(app.animation.machine.default, default_clip);
    assert!(
        !app.animation.machine.clips.is_empty(),
        "动画实体应当至少有一个 clip"
    );

    // 帧内拖动/键入：mark 直写文档并置 pending，但不写历史、不落盘。
    app.animation.machine.default = "move".to_owned();
    app.mark_inspector_edit();
    assert!(app.is_edit_transaction_active(), "mark 应已自动开事务");
    assert!(app.inspector_edit.pending);
    assert_eq!(app.history.undo_len(), 0, "直写阶段不产生历史");

    // 同一交互多次 mark：不产生多条历史（一次拖拽=一帧一次 mark 的等价）。
    app.animation.machine.default = "attack".to_owned();
    app.mark_inspector_edit();
    app.animation.machine.default = "move".to_owned();
    app.mark_inspector_edit();
    assert_eq!(app.history.undo_len(), 0, "多次直写仍不产生历史");

    // 提交边界：收敛为一条历史。
    let changed = app.finish_inspector_edits_if_dirty();
    assert!(changed, "有真实 diff 时应提交一条历史");
    assert_eq!(app.history.undo_len(), 1, "一次交互收敛为一条历史");
    assert!(!app.inspector_edit.pending);
    assert!(!app.is_edit_transaction_active(), "提交后事务关闭");
    let committed = app.document.current_scene.clone().unwrap();
    assert_ne!(&committed, &pristine);

    // 无净差异的再次提交：no-op，不产生空历史。
    assert!(!app.finish_inspector_edits_if_dirty());
    assert_eq!(app.history.undo_len(), 1, "空操作不产生空历史");

    // 撤销一步回到编辑前。
    app.undo_with_transaction_guard();
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &pristine,
        "撤销应回到 Inspector 编辑前"
    );
    let _ = fs::remove_dir_all(root);
}

/// E08：Scene 拖拽（拖动中多帧直写文档）在松手提交前不写历史；提交后恰
/// 一条历史、一次 undo 整体回退（不留逐帧碎片）。
#[test]
fn e08_scene_drag_direct_writes_commit_once_on_release() {
    let (mut app, root) = open_sample_production("e08-scene-drag-writes");
    let id = SceneEntityId(2);
    app.selection
        .set_single(id, editor::SelectionSource::Hierarchy);
    app.sync_inspector(id);
    let pristine = app.document.current_scene.clone().unwrap();
    assert_eq!(app.history.undo_len(), 0);
    // `Transform` 缓冲已随迁移删除（第 36 轮）：初值直接从**文档**读。
    let original_x = crate::geometry::parse_transform(
        &app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"],
    )
    .expect("Transform 可解析")
    .position_x;

    // 拖动首帧：begin 事务；随后多帧在事务内直写（等效 apply_drag_preview 逐帧写）。
    assert!(app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    ));
    for offset in [10.0_f32, 25.0, 42.0] {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(
                "Transform".to_owned(),
                format!("x={};y=0", original_x + offset),
            );
        scene.mark_dirty();
        assert!(app.is_edit_transaction_active());
        assert_eq!(
            app.history.undo_len(),
            0,
            "拖动直写期间不产生历史（不写盘）"
        );
    }

    // 松手：提交为一条历史，dirty（交由 autosave / 保存点落盘）。
    let committed = app.commit_edit_transaction();
    assert!(committed, "拖拽有净变化应提交成功");
    assert_eq!(app.history.undo_len(), 1, "一次拖拽 = 一条历史");
    assert!(!app.is_edit_transaction_active());

    // 撤销一次 → 全部拖拽位移一次性回退（无碎片）。
    app.undo_with_transaction_guard();
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &pristine,
        "单次 undo 应撤销整段拖动"
    );
    let _ = fs::remove_dir_all(root);
}

/// E08：Escape 取消活动事务后清空 Scene View 拖拽残留 —— stale preview /
/// gizmo 不留在状态里，避免下一帧把已取消的拖动重新写回文档。
#[test]
fn e08_escape_cancel_clears_scene_drag_residue() {
    let (mut app, root) = open_sample_production("e08-escape-clear");
    let id = SceneEntityId(2);
    app.selection
        .set_single(id, editor::SelectionSource::Hierarchy);
    app.sync_inspector(id);
    let pristine = app.document.current_scene.clone().unwrap();

    // 模拟一个已开事务的拖拽 + preview / gizmo 残留。
    assert!(app.begin_edit_transaction(
        "Scene Transform",
        editor::CommandKind::Transform(id),
        Some(id),
    ));
    {
        let scene = app.document.current_scene.as_mut().unwrap();
        scene
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert(
                    "Transform".to_owned(),
                    "position=999.000000000,999.000000000,0.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000"
                        .to_owned(),
                );
        scene.mark_dirty();
    }
    // 拖拽预览值从**文档**读（`Transform` 缓冲已随迁移删除，第 36 轮）。
    let preview_transform = crate::geometry::parse_transform(
        &app.document.current_scene.as_ref().unwrap().scene.entities[&id].components["Transform"],
    )
    .expect("Transform 可解析");
    app.scene_state.preview.insert(id, preview_transform);
    app.scene_state
        .preview_ui
        .insert(id, [3.0, -5.0, 3.0, -5.0]);
    app.scene_state
        .preview3d_transform
        .insert(id, Transform3D::default());
    assert!(app.is_edit_transaction_active());

    // Escape 取消：文档回滚 + 残留清理。
    app.cancel_edit_transaction();
    app.clear_active_scene_drag();

    assert!(!app.is_edit_transaction_active());
    assert_eq!(app.history.undo_len(), 0, "取消不产生历史");
    assert_eq!(
        app.document.current_scene.as_ref().unwrap(),
        &pristine,
        "取消回滚到事务前"
    );
    assert!(app.scene_state.preview.is_empty());
    assert!(app.scene_state.preview_ui.is_empty());
    assert!(app.scene_state.preview3d_transform.is_empty());
    let _ = fs::remove_dir_all(root);
}
