//! 跨面板共享的编辑命令：撤销/重做、保存、场景切换、实体创建、Inspector 同步、对齐。

use crate::AlignMode;
use crate::EditorApp;
use crate::geometry::{encode_transform, parse_transform};
use editor::{CommandKind, TransformFields};
use engine::core::scene::SceneEntityId;
use std::path::PathBuf;

impl EditorApp {
    pub(crate) fn undo(&mut self) {
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.undo(x).ok())
                .map(str::to_owned)
        };
        self.status = label.map_or_else(|| "Nothing to undo".into(), |x| format!("Undo: {x}"));
    }

    pub(crate) fn redo(&mut self) {
        let label = {
            let h = &mut self.history;
            let d = &mut self.document;
            d.current_scene
                .as_mut()
                .and_then(|x| h.redo(x).ok())
                .map(str::to_owned)
        };
        self.status = label.map_or_else(|| "Nothing to redo".into(), |x| format!("Redo: {x}"));
    }

    /// 保存当前场景（未命名时写入 scenes/<场景名>.scene）。
    pub(crate) fn save_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        let Some(doc) = self.document.current_scene.as_mut() else {
            self.status = "No scene".into();
            return;
        };
        let r = if doc.path.is_some() {
            self.recovery.save_document(doc).map_err(|e| e.to_string())
        } else {
            doc.save_as(
                self.project
                    .root
                    .join("scenes")
                    .join(format!("{}.scene", doc.scene.name)),
            )
            .map_err(|e| e.to_string())
        };
        self.status = match r {
            Ok(()) => {
                if let Some(doc) = self.document.current_scene.as_ref() {
                    let _ = self.recovery.clear_for(doc);
                }
                self.autosave.reset();
                self.recovery_scan = self.recovery.scan().unwrap_or_default();
                "Scene saved".into()
            }
            Err(error) => error,
        };
    }

    /// 打开场景资源（资源面板双击 / 层级面板场景下拉）。
    pub(crate) fn open_scene_resource(&mut self, relative_path: PathBuf) {
        if !self.project_loaded {
            return;
        }
        let path = self.project.root.join(&relative_path);
        match self.document.open_scene(&path) {
            Ok(()) => {
                self.history.clear();
                self.autosave.reset();
                self.play_session = None;
                self.tree.clear_selection();
                self.inspector.clear_selection();
                self.scene_state.preview.clear();
                self.scene_state.selected.clear();
                self.scene_state.collapsed.clear();
                self.sync_hierarchy_metadata();
                self.status = format!("已打开场景：{}", relative_path.display());
            }
            Err(error) => self.status = format!("打开场景失败：{error}"),
        }
    }

    /// 同步 InspectorState ↔ EditorApp 快照字段。
    pub(crate) fn sync_inspector(&mut self, id: SceneEntityId) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let scene = scene.clone();
        if self.inspector.select(&scene, id).is_ok()
            && let Ok(snapshot) = self.inspector.snapshot(&scene)
        {
            self.entity_name = snapshot.name;
            self.transform = snapshot.transform;
            self.transform3d = snapshot.transform3d.unwrap_or_default();
            self.mesh_renderer = snapshot.mesh_renderer.unwrap_or_default();
            self.camera3d = snapshot.camera3d.unwrap_or_default();
            self.light = snapshot.light.unwrap_or_default();
            self.collider3d = snapshot.collider3d.unwrap_or_default();
            self.text = snapshot.text.unwrap_or_default();
            self.image = snapshot.image.unwrap_or_default();
            self.button = snapshot.button.unwrap_or_default();
            self.sprite_reference = snapshot.sprite_reference.unwrap_or_default();
            self.lua_script = snapshot.lua_script.unwrap_or_default();
        }
    }

    /// 对齐选中实体：网格吸附 / 水平 / 垂直。
    pub(crate) fn align_selected(&mut self, mode: AlignMode) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        let ids: Vec<_> = self.scene_state.selected.iter().copied().collect();
        if ids.is_empty() {
            self.status = "请先选中实体".to_owned();
            return;
        }
        let mut targets: Vec<(SceneEntityId, TransformFields)> = Vec::new();
        for id in &ids {
            if let Some(entity) = scene.scene.entities.get(id)
                && let Some(transform) = entity
                    .components
                    .get("Transform")
                    .and_then(|value| parse_transform(value))
            {
                targets.push((*id, transform));
            }
        }
        if targets.is_empty() {
            return;
        }
        let pivot = targets[0].1;
        let step = self.snap_step;
        let new_targets = match mode {
            AlignMode::Grid => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_x = (next.position_x / step).round() * step;
                    next.position_y = (next.position_y / step).round() * step;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
            AlignMode::H => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_y = pivot.position_y;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
            AlignMode::V => targets
                .iter()
                .map(|(id, transform)| {
                    let mut next = *transform;
                    next.position_x = pivot.position_x;
                    (*id, next)
                })
                .collect::<Vec<_>>(),
        };
        let Some(scene) = self.document.current_scene.as_mut() else {
            return;
        };
        let _ = self
            .history
            .execute(scene, "Align", CommandKind::Property, move |scene| {
                for (id, transform) in &new_targets {
                    if let Some(entity) = scene.scene.entities.get_mut(id) {
                        entity
                            .components
                            .insert("Transform".to_owned(), encode_transform(*transform));
                    }
                }
                scene.mark_dirty();
                Ok::<(), ()>(())
            });
    }

    /// 创建根实体（层级面板 / Inspector）。
    pub(crate) fn add_entity(&mut self, parent: Option<SceneEntityId>) {
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
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result =
            self.history
                .execute(scene, "Create entity", CommandKind::Create, move |scene| {
                    let mut tree = tree;
                    let id = tree
                        .create_entity(scene, name.clone(), parent)
                        .map_err(|e| e.to_string())?;
                    // 2D 实体默认只带 Transform 组件。
                    if let Some(entity) = scene.scene.entities.get_mut(&id) {
                        entity.components.insert(
                            "Transform".into(),
                            encode_transform(TransformFields::default()),
                        );
                    }
                    Ok::<(), String>(())
                });
        match result {
            Ok(true) => {
                if let Some(scene) = self.document.current_scene.as_ref()
                    && let Some(id) = scene.scene.entities.keys().max_by_key(|id| id.0).copied()
                {
                    let _ = self.tree.select(scene, id);
                    self.sync_inspector(id);
                }
                self.status = "Entity created".into();
            }
            Ok(false) => self.status = "No change".into(),
            Err(e) => self.status = e,
        }
    }
}
