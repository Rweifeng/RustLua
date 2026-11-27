//! 层级面板的**提交层**（docs/55 §4.1 规则 1：View 只发 typed action）。
//!
//! 这些方法原来住在 `panels/hierarchy.rs` 里；搬出面板文件后，架构棘轮
//! （`panels/architecture_tests.rs`）不再把它们算成"面板直接写文档"。行为与搬家前一致：
//! 一次操作一条 Undo、Play 中锁定、失败只改状态栏不写文档。
//!
//! 与 `scene_drag.rs` 同一思路：面板负责绘制、命中测试与事件采集，**提交边界**住在面板外。

use crate::EditorApp;
use editor::{CommandKind, ReparentPosition, SelectionSource};
use rustlua_ecs::transform::Transform3D;
use rustlua_scene::scene::SceneEntityId;

/// 换父的落点：拖放（相对目标的前/后/内）或直接指定父级（「移动到根节点」/命令层）。
#[derive(Clone, Copy, Debug)]
pub(crate) enum ReparentTarget {
    /// 只指定新父级（`None` = 根）。
    Parent(Option<SceneEntityId>),
    /// 相对某个目标的落点（拖放语义，见 `SceneTree::reparent_at`）。
    At {
        target: SceneEntityId,
        position: ReparentPosition,
    },
}

impl EditorApp {
    pub(crate) fn paste_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(source) = self.hierarchy_clipboard.clone() else {
            self.status = "实体剪贴板为空".to_owned();
            return;
        };
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        let result = self
            .history
            .execute(scene, "粘贴实体", CommandKind::Create, |scene| {
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
                self.status = "实体已粘贴".to_owned();
            }
            Ok(false) => self.status = "没有变化".to_owned(),
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
        // 拖的是选中集合里的一员 ⇒ 整批移动顶层根；否则只移动这一行。
        let batch: Vec<SceneEntityId> =
            if self.selection.len() > 1 && self.selection.contains(dropped_id) {
                self.top_most_selected_ids()
            } else {
                vec![dropped_id]
            };
        // Before/After 的有效父级是目标的父级；「目标不是移动方后代」检查同时
        // 覆盖两种情况（若目标的父级在移动方子树内，目标本身也在其中）。
        match self.commit_reparent_batch(
            batch,
            ReparentTarget::At {
                target: target_id,
                position,
            },
        ) {
            Ok(moved) => {
                self.status = if moved > 1 {
                    format!("已移动 {moved} 个实体")
                } else {
                    "已通过拖拽更改父实体".to_owned()
                };
            }
            Err(error) => self.status = error,
        }
    }

    /// 批量换父的提交内核：**整批校验 → 一条事务 → keep-world → 提交或整体回滚**。
    ///
    /// 拖放与「移动到根节点」共用这一份：两条路径以前各写一遍，结果 `reparent_selected`
    /// 只搬主选中，而拖拽会搬整批——同一个"移动"动作出现两种语义。
    pub(crate) fn commit_reparent_batch(
        &mut self,
        batch: Vec<SceneEntityId>,
        target: ReparentTarget,
    ) -> Result<usize, String> {
        if batch.is_empty() {
            return Err("请先选择实体".to_owned());
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return Err("请先创建场景".to_owned());
        };
        let validated_against = match target {
            ReparentTarget::Parent(parent) => parent,
            ReparentTarget::At { target, .. } => Some(target),
        };
        for moving_id in &batch {
            self.validate_reparent_target(scene, *moving_id, validated_against)?;
        }
        let primary = batch[0];
        if !self.begin_edit_transaction("更改父实体", CommandKind::Reparent, Some(primary)) {
            return Err("重挂载事务已激活，无法重复开始".to_owned());
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.cancel_edit_transaction();
            return Err("请先创建场景".to_owned());
        };
        // docs/54 §8-⑦（§4.1）：换父提供 **keep-world** 语义——先记下移动方的世界矩阵，改完父
        // 再按新父反解局部值，因此"拖进新父"不会让实体在画面上跳走。反解不了（剪切/镜像/退化）
        // 时整笔事务取消，绝不静默近似。
        let worlds: Vec<(SceneEntityId, rustlua_math::math3d::Mat4)> = {
            let matrices = editor::document_space::world_matrices(&scene.scene);
            batch
                .iter()
                .filter_map(|id| matrices.get(id).map(|matrix| (*id, *matrix)))
                .collect()
        };
        let mut error = None;
        for moving_id in &batch {
            let placed = match target {
                ReparentTarget::Parent(parent) => self
                    .tree
                    .reparent(scene, *moving_id, parent)
                    .map_err(|error| error.to_string()),
                ReparentTarget::At { target, position } => self
                    .tree
                    .reparent_at(scene, *moving_id, target, position)
                    .map_err(|error| error.to_string()),
            };
            if let Err(failure) = placed {
                error = Some(failure);
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
            Err(error)
        } else {
            self.commit_edit_transaction();
            self.on_document_changed(crate::actions::DocumentChange::EntityReparented);
            Ok(batch.len())
        }
    }

    /// 实际创建并切换空白场景（仅由确认流或 clean 快速路径调用）。
    pub(crate) fn perform_create_scene(&mut self, name: String) {
        self.set_current_scene_document(editor::SceneDocument::new(name.clone()));
        self.history.clear();
        self.autosave.reset();
        // P1-4：换文档是释放旧场景派生物的天然时机——新场景多半用不到旧网格/贴图，
        // 留着只是占内存。释放不影响任何像素（下次绘制按需重建），代价只是首帧变慢。
        let released = self.release_all_resources();
        self.on_document_changed(crate::actions::DocumentChange::Created);
        self.status = if released > 0 {
            format!(
                "已创建场景 {name}（未保存，释放缓存 {}）",
                crate::panels::performance::format_bytes(released)
            )
        } else {
            format!("已创建场景 {name}（未保存）")
        };
    }

    /// 创建一个带 Transform3D 组件的 3D 实体（M9-03）。
    pub(crate) fn add_3d_entity(&mut self, parent: Option<SceneEntityId>) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".into();
            return;
        }
        let name = self.entity_name.trim().to_owned();
        if name.is_empty() {
            self.status = "实体名不能为空".into();
            return;
        }
        let tree = self.tree.clone();
        let created = std::cell::Cell::new(None::<SceneEntityId>);
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".into();
            return;
        };
        let result = self
            .history
            .execute(scene, "创建 3D 实体", CommandKind::Create, |scene| {
                let id = tree
                    .create_entity(scene, name.clone(), parent)
                    .map_err(|e| e.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
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
                self.status = "3D 实体已创建".into();
            }
            Ok(false) => self.status = "没有变化".into(),
            Err(e) => self.status = e,
        }
    }

    pub(crate) fn set_ui_root(&mut self, id: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        let result = self.history.execute(
            scene,
            "设置界面根节点",
            CommandKind::Property,
            |scene| {
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
            },
        );
        match result {
            Ok(true) => {
                self.on_document_changed(crate::actions::DocumentChange::UiRootChanged);
                self.status = "已设置界面根节点".to_owned();
            }
            Ok(false) => self.status = "没有变化".to_owned(),
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
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let tree = self.tree.clone();
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        let result =
            self.history
                .execute(scene, "重命名实体", CommandKind::Rename, move |scene| {
                    tree.rename(scene, id, name)
                        .map_err(|error| error.to_string())
                });
        match result {
            Ok(true) => {
                self.on_document_changed(crate::actions::DocumentChange::EntityRenamed);
                self.status = "实体已重命名".to_owned();
            }
            Ok(false) => self.status = "没有变化".to_owned(),
            Err(error) => self.status = error,
        }
    }

    /// H04：Ctrl+D 原地复制 —— 复制到原父节点并紧随源节点之后，名称与源一致，
    /// 一个 Create 事务（Undo 一步整体回滚）。
    ///
    /// 多选时复制**整个选中集合的顶层根**，复制体紧随各自源节点，并成为新的选中集合
    /// （Unity 的 Ctrl+D 语义）；以前只复制 `primary`，多选后按 Ctrl+D 会悄悄只多一个。
    pub(crate) fn duplicate_selected_entities(&mut self) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        if self.selection.primary().is_none() {
            self.status = "请先在层级面板中选择实体".to_owned();
            return;
        }
        // 按可见顺序取顶层根：先复制上层再复制下层，落点位置才能"紧随源节点"。
        let order = self
            .top_most_selected_ids()
            .into_iter()
            .filter_map(|id| {
                let entity = self
                    .current_scene()
                    .and_then(|scene| scene.scene.entities.get(&id))?
                    .clone();
                Some((id, entity))
            })
            .collect::<Vec<_>>();
        if order.is_empty() {
            self.status = "当前实体不存在".to_owned();
            return;
        }
        let tree = self.tree.clone();
        let created: std::cell::RefCell<Vec<SceneEntityId>> = std::cell::RefCell::new(Vec::new());
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "请先创建场景".to_owned();
            return;
        };
        let result = self
            .history
            .execute(scene, "复制实体", CommandKind::Create, |scene| {
                for (source_id, source) in &order {
                    let copy_id = tree
                        .duplicate_subtree(scene, source, source.parent)
                        .map_err(|error| error.to_string())?;
                    tree.reparent_at(scene, copy_id, *source_id, ReparentPosition::After)
                        .map_err(|error| error.to_string())?;
                    created.borrow_mut().push(copy_id);
                }
                Ok::<(), String>(())
            });
        match result {
            Ok(true) => {
                let created = created.into_inner();
                if let Some(last) = created.last().copied() {
                    self.selection
                        .set_many(created.clone(), Some(last), SelectionSource::Command);
                    self.selection_anchor = Some(last);
                    self.sync_inspector(last);
                }
                self.status = if created.len() > 1 {
                    format!("已复制 {} 个实体", created.len())
                } else {
                    "实体已复制".to_owned()
                };
            }
            Ok(false) => self.status = "没有变化".to_owned(),
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
