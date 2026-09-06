//! 行3 层级面板（资源面板右侧）：场景切换/新建 + 实体树 + 项目信息。

use crate::EditorApp;
use crate::util::{display_path, icon_button, strip_verbatim_prefix};
use editor::{CommandKind, ReparentPosition, SelectionSource};
use eframe::egui;
use engine::core::{
    prefab::Prefab, resource_db::AssetId, scene::SceneEntityId, transform3d::Transform3D,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl EditorApp {
    fn set_ui_root(&mut self, id: SceneEntityId) {
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

    fn focus_selected_scene_view(&mut self) {
        let Some(id) = self.selection.primary() else {
            self.status = "请先在 Hierarchy 中选择实体".to_owned();
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
        else {
            self.status = "当前实体不存在".to_owned();
            return;
        };
        if self.scene_view_mode == crate::SceneViewMode::View3D {
            let target = entity
                .components
                .get("Transform3D")
                .and_then(|value| Transform3D::from_component_str(value).ok())
                .map(|transform| transform.position)
                .unwrap_or(engine::core::math3d::Vec3::ZERO);
            self.editor_camera.target = target;
            self.status = "3D Scene View 已聚焦实体".to_owned();
            return;
        }
        let transform = entity
            .components
            .get("Transform")
            .and_then(|value| crate::geometry::parse_transform(value))
            .unwrap_or_default();
        self.scene_state.pan = egui::vec2(
            -transform.position_x * self.scene_zoom,
            transform.position_y * self.scene_zoom,
        );
        self.status = "Scene View 已聚焦实体".to_owned();
    }

    fn copy_selected_entity(&mut self) {
        let Some(id) = self.selection.primary() else {
            self.status = "请先在 Hierarchy 中选择实体".to_owned();
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
        else {
            self.status = "当前实体不存在".to_owned();
            return;
        };
        self.hierarchy_clipboard = Some(entity.clone());
        self.status = "Entity copied".to_owned();
    }

    fn paste_entity(&mut self, parent: Option<SceneEntityId>) {
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

    /// 层级面板：整合场景文件切换/新建，位于资源面板右侧。
    pub(crate) fn draw_hierarchy_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scene_tree")
            .resizable(true)
            .default_width(190.0)
            .min_width(110.0)
            .show(context, |ui| {
                // 场景文件集成：标题 + 当前场景保存状态。
                ui.horizontal(|ui| {
                    ui.heading("层级");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(document) = self.current_scene() {
                            if document.dirty {
                                ui.colored_label(egui::Color32::YELLOW, "● 未保存");
                            } else {
                                ui.label("已保存");
                            }
                        }
                    });
                });
                // 场景切换下拉：列出 scenes/ 目录全部场景文件。
                let scene_files = self.scene_files();
                let current_path = self
                    .current_scene()
                    .and_then(|document| document.path.clone());
                let mut open_path: Option<PathBuf> = None;
                egui::ComboBox::from_id_salt("scene_selector")
                    .width(ui.available_width())
                    .selected_text(self.scene_selector_label())
                    .show_ui(ui, |ui| {
                        if scene_files.is_empty() {
                            ui.label("（暂无场景文件）");
                        }
                        for file in &scene_files {
                            let is_current = current_path.as_ref().is_some_and(|path| {
                                strip_verbatim_prefix(path)
                                    == strip_verbatim_prefix(&self.project.root.join(file))
                            });
                            let label = file.file_name().map_or_else(
                                || file.display().to_string(),
                                |name| name.to_string_lossy().into_owned(),
                            );
                            if ui.selectable_label(is_current, label).clicked() && !is_current {
                                open_path = Some(file.clone());
                            }
                        }
                    });
                if let Some(document) = self.current_scene() {
                    let path = document
                        .path
                        .as_deref()
                        .map_or_else(|| "未保存，保存后将写入 scenes/".to_owned(), display_path);
                    ui.small(format!("路径：{path}"));
                }
                if let Some(path) = open_path {
                    self.open_scene_resource(path);
                }
                ui.separator();
                ui.text_edit_singleline(&mut self.hierarchy_filter);
                ui.separator();
                let roots: Vec<_> = self
                    .current_scene()
                    .map(|document| self.tree.roots(document).map(|entity| entity.id).collect())
                    .unwrap_or_default();
                for id in roots {
                    self.draw_entity(ui, id, 0);
                }
                if self.current_scene().is_none() {
                    ui.label("请先新建场景");
                }
            });
    }

    fn hierarchy_filter_matches(&self, scene: &editor::SceneDocument, id: SceneEntityId) -> bool {
        let Some(entity) = scene.scene.entities.get(&id) else {
            return false;
        };
        let query = self.hierarchy_filter.trim();
        if query.is_empty() {
            return true;
        }
        let query_lower = query.to_lowercase();
        let name_matches = entity.name.to_lowercase().contains(&query_lower);
        let descendant_matches = entity
            .children
            .iter()
            .any(|child| self.hierarchy_filter_matches(scene, *child));
        name_matches || descendant_matches
    }

    /// 递归绘制实体树节点。
    fn draw_entity(&mut self, ui: &mut egui::Ui, id: SceneEntityId, depth: usize) {
        let current_scene = self.document.current_scene.clone();
        let Some(scene) = current_scene.as_ref() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&id).cloned() else {
            return;
        };
        let query = self.hierarchy_filter.trim();
        let name = entity.name.clone();
        let children = entity.children.clone();
        let filter_active = !query.is_empty();
        let visible_subtree = !filter_active || self.hierarchy_filter_matches(scene, id);
        if filter_active && !visible_subtree {
            return;
        }
        let selected = self.selection.contains(id);
        let collapsed = self.scene_state.collapsed.contains(&id);
        let force_expand = filter_active
            && entity
                .children
                .iter()
                .any(|child| self.hierarchy_filter_matches(scene, *child));
        let effective_collapsed = collapsed && !force_expand;
        let hidden = self.scene_state.hidden.contains(&id);
        let locked = self.scene_state.locked.contains(&id);
        let ui_root = entity
            .metadata
            .get("ui.root")
            .is_some_and(|value| value == "true");
        let parent = entity.parent;
        let row_id = egui::Id::new(("hierarchy_entry", id.0));
        let row = ui.dnd_drag_source(row_id, id, |ui| {
            ui.dnd_drop_zone::<SceneEntityId, _>(egui::Frame::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    if !children.is_empty() {
                        if icon_button(
                            ui,
                            if effective_collapsed { "▸" } else { "▾" },
                            "展开或折叠子节点",
                        )
                        .clicked()
                        {
                            if effective_collapsed {
                                self.scene_state.collapsed.remove(&id);
                            } else {
                                self.scene_state.collapsed.insert(id);
                            }
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    if icon_button(ui, if hidden { "◌" } else { "◉" }, "显示或隐藏实体").clicked()
                    {
                        if hidden {
                            self.scene_state.hidden.remove(&id);
                            self.persist_hierarchy_flag(id, "hidden", false);
                        } else {
                            self.scene_state.hidden.insert(id);
                            self.persist_hierarchy_flag(id, "hidden", true);
                        }
                    }
                    if icon_button(ui, if locked { "⊠" } else { "⊡" }, "锁定或解锁实体").clicked()
                    {
                        if locked {
                            self.scene_state.locked.remove(&id);
                            self.persist_hierarchy_flag(id, "locked", false);
                        } else {
                            self.scene_state.locked.insert(id);
                            self.persist_hierarchy_flag(id, "locked", true);
                        }
                    }
                    if ui
                        .add_enabled(
                            !locked,
                            egui::Button::selectable(
                                selected,
                                format!(
                                    "{}{} [{}]",
                                    if ui_root { "[UI Root] " } else { "" },
                                    name,
                                    id.0
                                ),
                            ),
                        )
                        .clicked()
                    {
                        let additive =
                            ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                        if additive {
                            self.selection.toggle(id, SelectionSource::Hierarchy);
                        } else {
                            self.selection.set_single(id, SelectionSource::Hierarchy);
                        }
                        if let Some(primary) = self.selection.primary() {
                            self.sync_inspector(primary);
                        } else {
                            self.clear_inspector();
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("⋮", |ui| {
                            if ui.button("新建子节点").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.entity_name = "New Entity".to_owned();
                                self.add_entity(Some(id));
                                ui.close();
                            }
                            if ui.button("新建同级节点").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.entity_name = "New Entity".to_owned();
                                self.add_entity(parent);
                                ui.close();
                            }
                            if ui.button("重命名").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                let next_name = format!("{} Copy", name);
                                self.entity_name = next_name.clone();
                                self.rename_selected();
                                self.status = format!("Renamed to {next_name}");
                                ui.close();
                            }
                            if ui.button("复制").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.copy_selected_entity();
                                ui.close();
                            }
                            if ui.button("粘贴为子节点").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.paste_entity(Some(id));
                                ui.close();
                            }
                            if ui.button("创建 Prefab").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.create_prefab_from_selected();
                                ui.close();
                            }
                            if ui.button("重新应用 Prefab").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.reapply_selected_prefab();
                                ui.close();
                            }
                            if ui.button("聚焦 Scene View").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.focus_selected_scene_view();
                                ui.close();
                            }
                            if ui.button("设为 UI 根节点").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.set_ui_root(id);
                                ui.close();
                            }
                            if ui.button("移动到根节点").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.reparent_selected(None);
                                ui.close();
                            }
                            if ui.button("删除").clicked() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                if self.is_playing() {
                                    self.status = "Play mode locks scene editing; click Stop first"
                                        .to_owned();
                                } else {
                                    self.delete_selected_entity();
                                }
                                ui.close();
                            }
                        });
                    });
                })
            })
            .1
        });
        let drag_target_state = row
            .inner
            .as_ref()
            .map(|dragged_id| **dragged_id)
            .filter(|dragged_id| *dragged_id != id)
            .and_then(|dragged_id| {
                self.document.current_scene.as_ref().map(|scene| {
                    (
                        dragged_id,
                        self.validate_reparent_target(scene, dragged_id, Some(id)),
                    )
                })
            });
        let drop_valid = matches!(drag_target_state, Some((_, Ok(()))));
        let drop_invalid = matches!(drag_target_state, Some((_, Err(_))));
        let drop_position = ui
            .input(|input| input.pointer.interact_pos().map(|position| position.y))
            .map(|pointer_y| {
                let fraction = (pointer_y - row.response.rect.top()) / row.response.rect.height();
                if fraction < 0.33 {
                    ReparentPosition::Before
                } else if fraction > 0.66 {
                    ReparentPosition::After
                } else {
                    ReparentPosition::Child
                }
            })
            .unwrap_or(ReparentPosition::Child);
        if drop_valid || drop_invalid {
            let stroke = egui::Stroke::new(
                2.0_f32,
                if drop_valid {
                    egui::Color32::from_rgb(110, 210, 130)
                } else {
                    egui::Color32::from_rgb(220, 100, 100)
                },
            );
            let rect = row.response.rect;
            match drop_position {
                ReparentPosition::Before => {
                    ui.painter()
                        .line_segment([rect.left_top(), rect.right_top()], stroke);
                }
                ReparentPosition::After => {
                    ui.painter()
                        .line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
                }
                ReparentPosition::Child => {
                    ui.painter()
                        .rect_stroke(rect, 0.0_f32, stroke, egui::StrokeKind::Inside);
                }
            }
        }
        if let Some(dropped_id) = row.inner.as_ref().map(|id| **id) {
            if dropped_id != id {
                if let Some(scene) = self.document.current_scene.as_ref() {
                    if let Err(error) = self.validate_reparent_target(scene, dropped_id, Some(id)) {
                        self.status = error;
                    } else {
                        if !self.begin_edit_transaction(
                            "Reparent entity",
                            CommandKind::Reparent,
                            Some(dropped_id),
                        ) {
                            self.status = "重挂载事务已激活，无法重复开始".to_owned();
                        } else if let Some(scene) = self.document.current_scene.as_mut() {
                            match self.tree.reparent_at(scene, dropped_id, id, drop_position) {
                                Ok(()) => {
                                    self.commit_edit_transaction();
                                    self.on_document_changed(
                                        crate::actions::DocumentChange::EntityReparented,
                                    );
                                    self.status = "Reparented by drag".to_owned();
                                }
                                Err(error) => {
                                    self.cancel_edit_transaction();
                                    self.status = error.to_string();
                                }
                            }
                        } else {
                            self.cancel_edit_transaction();
                            self.status = "Create a scene first".to_owned();
                        }
                    }
                }
            }
        }
        if !effective_collapsed {
            for child in children {
                self.draw_entity(ui, child, depth + 1);
            }
        }
    }

    /// 从场景元数据同步 hidden/locked 到编辑态集合。
    /// 仅当场景标识（path, revision）变化时执行，避免每帧全量遍历实体。
    pub(crate) fn sync_hierarchy_metadata(&mut self) {
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.last_metadata_key = None;
            return;
        };
        let key = (scene.path.clone(), scene.revision);
        if self.last_metadata_key.as_ref() == Some(&key) {
            return;
        }
        self.last_metadata_key = Some(key);
        for id in scene.scene.entities.keys().copied().collect::<Vec<_>>() {
            let hidden = scene
                .scene
                .metadata
                .get(&format!("editor.hidden.{}", id.0))
                .is_some_and(|v| v == "true");
            let locked = scene
                .scene
                .metadata
                .get(&format!("editor.locked.{}", id.0))
                .is_some_and(|v| v == "true");
            if hidden {
                self.scene_state.hidden.insert(id);
            } else {
                self.scene_state.hidden.remove(&id);
            }
            if locked {
                self.scene_state.locked.insert(id);
            } else {
                self.scene_state.locked.remove(&id);
            }
        }
    }

    fn persist_hierarchy_flag(&mut self, id: SceneEntityId, kind: &str, enabled: bool) {
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

    /// 场景切换下拉框的选中文本：场景名 + 未保存标记。
    fn scene_selector_label(&self) -> String {
        let name = self.scene_name();
        if self.current_scene().is_some_and(|document| document.dirty) {
            format!("{name} ●未保存")
        } else {
            name
        }
    }

    /// 项目 `scenes/` 目录下的全部场景文件（相对路径，按名称排序），用于场景切换下拉。
    /// 至少 2 秒缓存一次，避免每帧 read_dir 扫 scenes 目录。
    fn scene_files(&mut self) -> Vec<PathBuf> {
        let root = strip_verbatim_prefix(&self.project.root);
        let now = Instant::now();
        if self
            .scene_files_cache_time
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(2))
        {
            return self.scene_files_cache.clone();
        }
        self.scene_files_cache_time = Some(now);
        self.scene_files_cache = scan_scene_files(&root);
        self.scene_files_cache.clone()
    }

    /// 新建场景（未保存；保存时写入 scenes/<名称>.scene）。
    ///
    /// E06：先做项目已加载 / Play 锁 / 名称校验（这些失败不弹确认），随后经
    /// 「未保存切换保护」网关请求——dirty 时弹确认窗，clean 时直接 `perform_create_scene`。
    pub(crate) fn create_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let name = self.new_scene_name.trim().to_owned();
        if name.is_empty() {
            self.status = "场景名称不能为空".to_owned();
            return;
        }
        self.request_document_change(crate::actions::PendingChange::CreateScene { name });
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

    /// 把选中实体保存为预制体（assets/prefabs/<name>.prefab）。
    pub(crate) fn create_prefab_from_selected(&mut self) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先在 Hierarchy 中选中实体".to_owned();
            return;
        };
        let Some(scene) = self.current_scene() else {
            return;
        };
        let name = scene
            .scene
            .entities
            .get(&selected)
            .map(|entity| entity.name.clone())
            .unwrap_or_else(|| "Prefab".to_owned());
        let prefab = Prefab::from_scene(
            &scene.scene,
            selected,
            AssetId::from_seed(&name),
            name.clone(),
        );
        match prefab {
            Ok(prefab) => {
                let root = strip_verbatim_prefix(&self.project.root);
                let dir = root.join("assets").join("prefabs");
                if std::fs::create_dir_all(&dir).is_err() {
                    self.status = "无法创建 prefabs 目录".to_owned();
                    return;
                }
                if let Err(error) = prefab.save(dir.join(format!("{name}.prefab"))) {
                    self.status = format!("预制体保存失败：{error}");
                    return;
                }
                self.status = format!("已创建预制体 {name}");
                self.refresh_resources();
            }
            Err(error) => self.status = format!("创建预制体失败：{error}"),
        }
    }
}

/// 扫描项目 `scenes/` 目录下的场景文件（相对路径，按名称排序）。
fn scan_scene_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("scenes")) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "scene"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_path_buf());
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use editor::SceneDocument;
    use engine::core::{logging::LogSink, scene::SceneEntityId};

    #[test]
    fn hierarchy_filter_keeps_matching_ancestor_path_visible() {
        let mut document = SceneDocument::new_scene("main");
        let root = SceneEntityId(1);
        let branch = SceneEntityId(2);
        let leaf = SceneEntityId(3);
        document.scene.add_entity(root, "Root", None).unwrap();
        document
            .scene
            .add_entity(branch, "Branch", Some(root))
            .unwrap();
        document
            .scene
            .add_entity(leaf, "Leaf", Some(branch))
            .unwrap();
        let mut app = crate::EditorApp::new(
            editor::ProjectInfo {
                root: std::env::temp_dir()
                    .join(format!("p2-hierarchy-filter-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.document.current_scene = Some(document);
        app.hierarchy_filter = "Leaf".to_owned();

        let root_visible = app.hierarchy_filter_matches(app.current_scene().unwrap(), root);
        let branch_visible = app.hierarchy_filter_matches(app.current_scene().unwrap(), branch);
        let leaf_visible = app.hierarchy_filter_matches(app.current_scene().unwrap(), leaf);

        assert!(root_visible);
        assert!(branch_visible);
        assert!(leaf_visible);
    }
}
