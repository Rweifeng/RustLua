//! 层级面板的**提交层**（docs/55 §4.1 规则 1：View 只发 typed action）。
//!
//! 这些方法原来住在 `panels/hierarchy.rs` 里；搬出面板文件后，架构棘轮
//! （`panels/architecture_tests.rs`）不再把它们算成"面板直接写文档"。行为与搬家前一致：
//! 一次操作一条 Undo、Play 中锁定、失败只改状态栏不写文档。
//!
//! 与 `scene_drag.rs` 同一思路：面板负责绘制、命中测试与事件采集，**提交边界**住在面板外。

use crate::EditorApp;
use editor::{CommandKind, ReparentPosition, SelectionSource};
use engine::core::{scene::SceneEntityId, transform::Transform3D};

impl EditorApp {
    pub(crate) fn paste_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(source) = self.hierarchy_clipboard.clone() else {
            self.status = "Entity clipboard is empty".to_owned();
            return;
        };
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".to_owned();
            return;
        };
        let result = self
            .history
            .execute(scene, "Paste entity", CommandKind::Create, |scene| {
                let id = tree
                    .duplicate_subtree(scene, &source, parent)
                    .map_err(|error| error.to_string())?;
                created.set(Some(id));
                Ok::<(), String>(())
            });
        match result {
            Ok(true) => {
                if let Some(id) = created.get() {
                    self.selection.set_single(id, SelectionSource::Hierarchy);
                    self.sync_inspector(id);
                }
                self.status = "Entity pasted".to_owned();
            }
            Ok(false) => self.status = "No change".to_owned(),
            Err(error) => self.status = error,
        }
    }

    /// H03：拖拽落点结算。多选时移动选中集合的顶层根，整批校验、整批一个事务；
    /// 任一目标非法整批取消（事务快照回滚）。
    pub(crate) fn handle_hierarchy_drop(
        &mut self,
        dropped_id: SceneEntityId,
        target_id: SceneEntityId,
        position: ReparentPosition,
    ) {
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.status = "Create a scene first".to_owned();
            return;
        };
        let batch: Vec<SceneEntityId> =
            if self.selection.len() > 1 && self.selection.contains(dropped_id) {
                self.top_most_selected_ids()
            } else {
                vec![dropped_id]
            };
        // Before/After 的有效父级是目标的父级；「目标不是移动方后代」检查同时
        // 覆盖两种情况（若目标的父级在移动方子树内，目标本身也在其中）。
        for moving_id in &batch {
            if let Err(error) = self.validate_reparent_target(scene, *moving_id, Some(target_id)) {
                self.status = error;
                return;
            }
        }
        let primary = batch[0];
        if !self.begin_edit_transaction("Reparent entities", CommandKind::Reparent, Some(primary)) {
            self.status = "重挂载事务已激活，无法重复开始".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.cancel_edit_transaction();
            self.status = "Create a scene first".to_owned();
            return;
        };
        let mut error = None;
        // docs/54 §8-⑦（§4.1）：换父提供 **keep-world** 语义——先记下移动方的世界矩阵，改完父
        // 再按新父反解局部值，因此"拖进新父"不会让实体在画面上跳走。反解不了（剪切/镜像/退化）
        // 时整笔事务取消，绝不静默近似。
        let worlds: Vec<(SceneEntityId, engine::core::math3d::Mat4)> = {
            let matrices = editor::document_space::world_matrices(&scene.scene);
            batch
                .iter()
                .filter_map(|id| matrices.get(id).map(|matrix| (*id, *matrix)))
                .collect()
        };
        for moving_id in &batch {
            if let Err(failure) = self
                .tree
                .reparent_at(scene, *moving_id, target_id, position)
            {
                error = Some(failure.to_string());
                break;
            }
        }
        if error.is_none() {
            for (id, world) in worlds {
                match editor::document_space::keep_world_after_reparent(&mut scene.scene, id, world)
                {
                    Ok(_) => {}
                    Err(reason) => {
                        error = Some(format!("换父无法保持世界位置：{reason}"));
                        break;
                    }
                }
            }
        }
        if let Some(error) = error {
            self.cancel_edit_transaction();
            self.status = error;
        } else {
            self.commit_edit_transaction();
            self.on_document_changed(crate::actions::DocumentChange::EntityReparented);
            self.status = if batch.len() > 1 {
                format!("已移动 {} 个实体", batch.len())
            } else {
                "Reparented by drag".to_owned()
            };
        }
    }

    /// 实际创建并切换空白场景（仅由确认流或 clean 快速路径调用）。
    pub(crate) fn perform_create_scene(&mut self, name: String) {
        self.set_current_scene_document(editor::SceneDocument::new(name.clone()));
        self.history.clear();
        self.autosave.reset();
        self.on_document_changed(crate::actions::DocumentChange::Created);
        self.status = format!("已创建场景 {name}（未保存）");
    }

    /// E19: create a Widget document with a Canvas root and enter Widget Mode.
    pub(crate) fn perform_create_widget(&mut self, name: String) {
        let mut document = editor::SceneDocument::new_widget(name.clone());
        let scene = &mut document.scene;
        scene
            .add_entity(SceneEntityId(1), "Canvas", None)
            .expect("a new scene can accept the Canvas root");
        if let Some(entity) = scene.entities.get_mut(&SceneEntityId(1)) {
            entity
                .metadata
                .insert("ui.root".to_owned(), "true".to_owned());
        }
        self.set_current_scene_document(document);
        self.history.clear();
        self.autosave.reset();
        self.scene_view_mode = crate::SceneViewMode::Widget;
        self.on_document_changed(crate::actions::DocumentChange::Created);
        self.scene_view_fit_pending = true;
        self.status = format!("已创建 Widget {name}（未保存）");
    }

    /// 创建一个带 Transform3D 组件的 3D 实体（M9-03）。
    pub(crate) fn add_3d_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "Entity name cannot be empty".into();
            return;
        }
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Create 3D entity", CommandKind::Create, |scene| {
                    let id = tree
                        .create_entity(scene, name.clone(), parent)
                        .map_err(|e| e.to_string())?;
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        entity.components.insert(
                            "Transform3D".into(),
                            Transform3D::default().to_component_str(),
                        );
                    }
                    created.set(Some(id));
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                if let Some(id) = created.get() {
                    self.selection.add(id, SelectionSource::Command);
                    self.sync_inspector(id);
                }
                self.status = "3D entity created".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }

    pub(crate) fn set_ui_root(&mut self, id: SceneEntityId) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".to_owned();
            return;
        };
        let result = self
            .history
            .execute(scene, "Set UI root", CommandKind::Property, |scene| {
                for entity in scene.scene.entities.values_mut() {
                    entity.metadata.remove("ui.root");
                }
                let entity = scene
                    .scene
                    .entities
                    .get_mut(&id)
                    .ok_or_else(|| "当前实体不存在".to_owned())?;
                entity
                    .metadata
                    .insert("ui.root".to_owned(), "true".to_owned());
                scene.mark_dirty();
                Ok::<(), String>(())
            });
        match result {
            Ok(true) => {
                self.on_document_changed(crate::actions::DocumentChange::UiRootChanged);
                self.status = "已设置 UI 根节点".to_owned();
            }
            Ok(false) => self.status = "No change".to_owned(),
            Err(error) => self.status = error,
        }
    }

    /// H02：提交行内重命名。空名拒绝并重新聚焦；Esc 取消不产生历史。
    pub(crate) fn commit_hierarchy_rename(&mut self, id: SceneEntityId) {
        let session = self.hierarchy_rename.take();
        let Some(session) = session.filter(|session| session.id == id) else {
            return;
        };
        let name = session.buffer.trim().to_owned();
        if name.is_empty() {
            self.hierarchy_rename = Some(crate::HierarchyRename {
                id,
                buffer: session.buffer,
                just_started: true,
            });
            self.status = "实体名称不能为空".to_owned();
            return;
        }
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".to_owned();
            return;
        };
        let result =
            self.history
                .execute(scene, "Rename entity", CommandKind::Rename, move |scene| {
                    tree.rename(scene, id, name)
                        .map_err(|error| error.to_string())
                });
        match result {
            Ok(true) => {
                self.on_document_changed(crate::actions::DocumentChange::EntityRenamed);
                self.status = "Entity renamed".to_owned();
            }
            Ok(false) => self.status = "No change".to_owned(),
            Err(error) => self.status = error,
        }
    }

    /// H04：Ctrl+D 原地复制 —— 复制到原父节点并紧随源节点之后，名称与源一致，
    /// 一个 Create 事务（Undo 一步整体回滚）。
    pub(crate) fn duplicate_selected_entity(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(source_id) = self.selection.primary() else {
            self.status = "请先在 Hierarchy 中选择实体".to_owned();
            return;
        };
        let Some(source) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&source_id))
            .cloned()
        else {
            self.status = "当前实体不存在".to_owned();
            return;
        };
        let parent = source.parent;
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".to_owned();
            return;
        };
        let result =
            self.history
                .execute(scene, "Duplicate entity", CommandKind::Create, |scene| {
                    let copy_id = tree
                        .duplicate_subtree(scene, &source, parent)
                        .map_err(|error| error.to_string())?;
                    tree.reparent_at(scene, copy_id, source_id, ReparentPosition::After)
                        .map_err(|error| error.to_string())?;
                    created.set(Some(copy_id));
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                if let Some(id) = created.get() {
                    self.selection.add(id, SelectionSource::Command);
                    self.selection_anchor = Some(id);
                    self.sync_inspector(id);
                }
                self.status = "Entity duplicated".to_owned();
            }
            Ok(false) => self.status = "No change".to_owned(),
            Err(error) => self.status = error,
        }
    }

    /// 隐藏/锁定标记落盘（场景级 metadata）：这是**直接写文档**的一处，
    /// 因此必须住在 shell 而不是面板里（面板只维护 `scene_state` 的视图态）。
    pub(crate) fn persist_hierarchy_flag(&mut self, id: SceneEntityId, kind: &str, enabled: bool) {
        if let Some(scene) = self.document.current_scene.as_mut() {
            let key = format!("editor.{kind}.{}", id.0);
            if enabled {
                scene.scene.metadata.insert(key, "true".to_owned());
            } else {
                scene.scene.metadata.remove(&key);
            }
            scene.mark_dirty();
        }
    }
}
