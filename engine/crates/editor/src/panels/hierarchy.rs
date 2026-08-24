//! 行3 层级面板（资源面板右侧）：场景切换/新建 + 实体树 + 项目信息。

use crate::EditorApp;
use crate::util::{display_path, strip_verbatim_prefix};
use editor::CommandKind;
use eframe::egui;
use engine::core::{
    prefab::Prefab, resource_db::AssetId, scene::SceneEntityId, transform3d::Transform3D,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl EditorApp {
    /// 层级面板：整合场景文件切换/新建，位于资源面板右侧。
    pub(crate) fn draw_hierarchy_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scene_tree")
            .resizable(true)
            .default_width(190.0)
            .min_width(110.0)
            .show(context, |ui| {
                // 场景文件集成：标题 + 当前场景保存状态。
                ui.horizontal(|ui| {
                    ui.heading("Hierarchy");
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

    /// 递归绘制实体树节点。
    fn draw_entity(&mut self, ui: &mut egui::Ui, id: SceneEntityId, depth: usize) {
        let Some(scene) = self.current_scene() else {
            return;
        };
        let Some(entity) = scene.scene.entities.get(&id) else {
            return;
        };
        let name = entity.name.clone();
        let children = entity.children.clone();
        if !self.hierarchy_filter.trim().is_empty()
            && !name
                .to_lowercase()
                .contains(&self.hierarchy_filter.to_lowercase())
        {
            return;
        }
        let selected = self.tree.selected() == Some(id);
        let collapsed = self.scene_state.collapsed.contains(&id);
        let hidden = self.scene_state.hidden.contains(&id);
        let locked = self.scene_state.locked.contains(&id);
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 14.0);
            if !children.is_empty() {
                if ui.small_button(if collapsed { ">" } else { "v" }).clicked() {
                    if collapsed {
                        self.scene_state.collapsed.remove(&id);
                    } else {
                        self.scene_state.collapsed.insert(id);
                    }
                }
            } else {
                ui.add_space(18.0);
            }
            if ui.small_button(if hidden { "隐" } else { "显" }).clicked() {
                if hidden {
                    self.scene_state.hidden.remove(&id);
                    self.persist_hierarchy_flag(id, "hidden", false);
                } else {
                    self.scene_state.hidden.insert(id);
                    self.persist_hierarchy_flag(id, "hidden", true);
                }
            }
            if ui.small_button(if locked { "L" } else { "U" }).clicked() {
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
                    egui::Button::selectable(selected, format!("{} [{}]", name, id.0)),
                )
                .clicked()
            {
                let additive = ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                if !additive {
                    self.scene_state.selected.clear();
                }
                self.scene_state.selected.insert(id);
                if let Some(scene) = self.current_scene() {
                    let scene = scene.clone();
                    let _ = self.tree.select(&scene, id);
                }
                self.sync_inspector(id);
            }
        });
        if !collapsed {
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
        self.document.new_scene(name.clone());
        self.history.clear();
        self.autosave.reset();
        self.tree.clear_selection();
        self.inspector.clear_selection();
        self.status = format!("已创建场景 {name}（未保存）");
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
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "Create a scene first".into();
            return;
        };
        let result = self.history.execute(
            scene,
            "Create 3D entity",
            CommandKind::Create,
            move |scene| {
                let mut tree = tree;
                let id = tree
                    .create_entity(scene, name.clone(), parent)
                    .map_err(|e| e.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform3D".into(),
                        Transform3D::default().to_component_str(),
                    );
                }
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(true) => {
                if let Some(scene) = self.document.current_scene.as_ref()
                    && let Some(id) = scene.scene.entities.keys().max_by_key(|id| id.0).copied()
                {
                    let _ = self.tree.select(scene, id);
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
        let Some(selected) = self.tree.selected() else {
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
