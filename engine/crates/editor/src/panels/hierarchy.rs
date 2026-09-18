//! 行3 层级面板（资源面板右侧）：场景切换/新建 + 实体树 + 项目信息。

use crate::EditorApp;
use crate::util::{display_path, strip_verbatim_prefix};
use editor::{CommandKind, ReparentPosition, SelectionSource};
use eframe::egui;
use engine::core::{
    prefab::Prefab, resource_db::AssetId, scene::SceneEntityId, transform::Transform3D,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// H04：Hierarchy 键盘导航动作。
#[derive(Clone, Copy)]
enum HierarchyNav {
    Up,
    Down,
    Left,
    Right,
    Rename,
    Focus,
}

/// 紧凑三角形展开/折叠按钮：用 painter 直接绘制，不依赖字体字形。
fn triangle_disclosure_button(ui: &mut egui::Ui, expanded: bool, tooltip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let color = if response.hovered() || response.is_pointer_button_down_on() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let c = rect.center();
    let points: Vec<egui::Pos2> = if expanded {
        // 向下：底边在上，尖端朝下。
        vec![
            egui::pos2(c.x - 5.0, c.y - 3.0),
            egui::pos2(c.x + 5.0, c.y - 3.0),
            egui::pos2(c.x, c.y + 4.0),
        ]
    } else {
        // 向右：底边在左，尖端朝右。
        vec![
            egui::pos2(c.x - 3.0, c.y - 5.0),
            egui::pos2(c.x - 3.0, c.y + 5.0),
            egui::pos2(c.x + 4.0, c.y),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
    response.on_hover_text(tooltip)
}

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

    pub(crate) fn copy_selected_entity(&mut self) {
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

    /// 层级面板：整合场景文件切换/新建，位于资源面板右侧。
    pub(crate) fn draw_hierarchy_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scene_tree")
            .resizable(true)
            .default_width(190.0)
            .min_width(110.0)
            .show(context, |ui| {
                // 场景文件集成：标题 + 「+」创建菜单 + 当前场景保存状态。
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
                        // H01：Unity 风格「+」创建菜单，父级取当前主选中（无则挂根）。
                        let create_parent = self.selection.primary();
                        ui.menu_button("+", |ui| {
                            self.draw_hierarchy_create_menu(ui, create_parent);
                        });
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
                // H05：搜索框 —— 占位提示 + 非空时一键清空 + 命中计数。
                ui.horizontal(|ui| {
                    let edit_width = (ui.available_width() - 34.0).max(60.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.hierarchy_filter)
                            .hint_text("搜索实体")
                            .desired_width(edit_width),
                    );
                    if !self.hierarchy_filter.is_empty()
                        && ui.small_button("✕").on_hover_text("清空过滤").clicked()
                    {
                        self.hierarchy_filter.clear();
                    }
                });
                if !self.hierarchy_filter.trim().is_empty() {
                    ui.small(format!("匹配 {} 项", self.hierarchy_name_match_count()));
                }
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
                // H04：键盘导航（↑↓←→ / F2 / F）。指针悬停面板且无文本焦点时生效。
                self.handle_hierarchy_shortcuts(ui, context);
                // H07：空白区域 —— 左键取消选择，右键打开创建菜单。
                let empty_size = egui::vec2(ui.available_width(), ui.available_height().max(48.0));
                let (_, empty) = ui.allocate_exact_size(empty_size, egui::Sense::click());
                if empty.clicked() && self.current_scene().is_some() {
                    self.selection.clear(SelectionSource::Hierarchy);
                    self.selection_anchor = None;
                    self.clear_inspector();
                }
                empty.context_menu(|ui| {
                    let parent = self.selection.primary();
                    self.draw_hierarchy_create_menu(ui, parent);
                });
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

    /// H05：按名称（不含后代）命中的实体数，用于过滤框下的计数提示。
    fn hierarchy_name_match_count(&self) -> usize {
        let Some(document) = self.current_scene() else {
            return 0;
        };
        let query = self.hierarchy_filter.trim().to_lowercase();
        if query.is_empty() {
            return document.scene.entities.len();
        }
        document
            .scene
            .entities
            .values()
            .filter(|entity| entity.name.to_lowercase().contains(&query))
            .count()
    }

    /// H03：单选并更新 Shift 锚点（Hierarchy 内统一入口）。
    fn select_single_hierarchy(&mut self, id: SceneEntityId) {
        self.selection.set_single(id, SelectionSource::Hierarchy);
        self.selection_anchor = Some(id);
        self.sync_inspector(id);
    }

    /// H03：Shift+点击范围选择 —— 取锚点到点击行之间的可见连续区间。
    /// 锚点不可见（被过滤/折叠/切换场景）时退化为单选。
    fn shift_range_select(&mut self, id: SceneEntityId) {
        let order = self.hierarchy_visible_ids();
        let Some(anchor) = self.selection_anchor else {
            self.select_single_hierarchy(id);
            return;
        };
        let Some(from) = order.iter().position(|item| *item == anchor) else {
            self.select_single_hierarchy(id);
            return;
        };
        let Some(to) = order.iter().position(|item| *item == id) else {
            self.select_single_hierarchy(id);
            return;
        };
        let (start, end) = if from <= to { (from, to) } else { (to, from) };
        let ids = order[start..=end].to_vec();
        self.selection
            .set_many(ids, Some(id), SelectionSource::Hierarchy);
        self.sync_inspector(id);
        self.status = format!("已选择 {} 个实体", self.selection.len());
    }

    /// H03/H04：当前帧可见顺序表（过滤 + 折叠后的先序遍历）。
    fn hierarchy_visible_ids(&self) -> Vec<SceneEntityId> {
        let Some(document) = self.current_scene() else {
            return Vec::new();
        };
        let filter_active = !self.hierarchy_filter.trim().is_empty();
        let roots: Vec<SceneEntityId> = self.tree.roots(document).map(|entity| entity.id).collect();
        let mut out = Vec::new();
        self.collect_visible_ids(document, &roots, filter_active, &mut out);
        out
    }

    fn collect_visible_ids(
        &self,
        document: &editor::SceneDocument,
        ids: &[SceneEntityId],
        filter_active: bool,
        out: &mut Vec<SceneEntityId>,
    ) {
        for id in ids {
            if filter_active && !self.hierarchy_filter_matches(document, *id) {
                continue;
            }
            out.push(*id);
            let Some(entity) = document.scene.entities.get(id) else {
                continue;
            };
            let collapsed = self.scene_state.collapsed.contains(id);
            let force_expand = filter_active
                && entity
                    .children
                    .iter()
                    .any(|child| self.hierarchy_filter_matches(document, *child));
            if !collapsed || force_expand {
                let children = entity.children.clone();
                self.collect_visible_ids(document, &children, filter_active, out);
            }
        }
    }

    /// H02：进入行内重命名（F2 / 双击 / 菜单）。
    fn begin_hierarchy_rename(&mut self, id: SceneEntityId) {
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let Some(name) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
            .map(|entity| entity.name.clone())
        else {
            self.status = "当前实体不存在".to_owned();
            return;
        };
        self.hierarchy_rename = Some(crate::HierarchyRename {
            id,
            buffer: name,
            just_started: true,
        });
    }

    /// H02：提交行内重命名。空名拒绝并重新聚焦；Esc 取消不产生历史。
    fn commit_hierarchy_rename(&mut self, id: SceneEntityId) {
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

    /// H02：行内重命名输入框。Enter 提交，Esc 取消，点击其他区域提交（Unity 习惯）。
    fn draw_hierarchy_rename_edit(&mut self, ui: &mut egui::Ui, id: SceneEntityId) {
        let Some(session) = self.hierarchy_rename.as_mut() else {
            return;
        };
        if session.id != id {
            return;
        }
        let edit_id = egui::Id::new(("hierarchy_rename", id.0));
        let edit = egui::TextEdit::singleline(&mut session.buffer)
            .id(edit_id)
            .desired_width(ui.available_width().max(60.0))
            .cursor_at_end(false);
        let response = ui.add(edit);
        if session.just_started {
            response.request_focus();
            session.just_started = false;
        }
        let focused = response.has_focus();
        let enter = focused && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let escape = focused && ui.input(|input| input.key_pressed(egui::Key::Escape));
        let lost_focus = response.lost_focus();
        if escape {
            self.hierarchy_rename = None;
            self.status = "已取消重命名".to_owned();
        } else if enter || lost_focus {
            self.commit_hierarchy_rename(id);
        }
    }

    /// H04：Hierarchy 键盘操作。仅当指针悬停面板、无文本输入焦点且无重命名会话时生效。
    fn handle_hierarchy_shortcuts(&mut self, ui: &egui::Ui, context: &egui::Context) {
        if context.wants_keyboard_input() || self.hierarchy_rename.is_some() {
            return;
        }
        if !ui.rect_contains_pointer(ui.max_rect()) {
            return;
        }
        if self.current_scene().is_none() || self.selection.primary().is_none() {
            return;
        }
        let mut action = None;
        ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowUp) {
                action = Some(HierarchyNav::Up);
            } else if input.key_pressed(egui::Key::ArrowDown) {
                action = Some(HierarchyNav::Down);
            } else if input.key_pressed(egui::Key::ArrowLeft) {
                action = Some(HierarchyNav::Left);
            } else if input.key_pressed(egui::Key::ArrowRight) {
                action = Some(HierarchyNav::Right);
            } else if input.key_pressed(egui::Key::F2) {
                action = Some(HierarchyNav::Rename);
            } else if input.key_pressed(egui::Key::F) {
                action = Some(HierarchyNav::Focus);
            }
        });
        match action {
            Some(HierarchyNav::Up) => self.move_hierarchy_selection(-1),
            Some(HierarchyNav::Down) => self.move_hierarchy_selection(1),
            Some(HierarchyNav::Left) => self.collapse_or_jump_parent(),
            Some(HierarchyNav::Right) => self.expand_or_enter_child(),
            Some(HierarchyNav::Rename) => {
                if let Some(primary) = self.selection.primary() {
                    self.begin_hierarchy_rename(primary);
                }
            }
            Some(HierarchyNav::Focus) => self.focus_selected_scene_view(),
            None => {}
        }
    }

    /// H04：↑/↓ 在可见顺序表中移动单选。
    fn move_hierarchy_selection(&mut self, delta: i32) {
        let order = self.hierarchy_visible_ids();
        let Some(primary) = self.selection.primary() else {
            return;
        };
        let Some(index) = order.iter().position(|id| *id == primary) else {
            return;
        };
        let next = (index as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        let Some(&id) = order.get(next) else {
            return;
        };
        self.select_single_hierarchy(id);
    }

    /// H04：← 折叠当前行（已折叠则跳到父节点）。
    fn collapse_or_jump_parent(&mut self) {
        let Some(primary) = self.selection.primary() else {
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&primary))
            .cloned()
        else {
            return;
        };
        if !entity.children.is_empty() && !self.scene_state.collapsed.contains(&primary) {
            self.set_hierarchy_collapsed(primary, true);
        } else if let Some(parent) = entity.parent {
            self.select_single_hierarchy(parent);
        }
    }

    /// H04：→ 展开当前行（已展开则进入首个子节点）。
    fn expand_or_enter_child(&mut self) {
        let Some(primary) = self.selection.primary() else {
            return;
        };
        let Some(entity) = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&primary))
            .cloned()
        else {
            return;
        };
        if entity.children.is_empty() {
            return;
        }
        if self.scene_state.collapsed.contains(&primary) {
            self.set_hierarchy_collapsed(primary, false);
        } else {
            self.select_single_hierarchy(entity.children[0]);
        }
    }

    /// H06：Alt+点击展开箭头 —— 折叠/展开整棵子树。
    fn set_subtree_collapsed(&mut self, id: SceneEntityId, collapse: bool) {
        for target in self.subtree_ids(id) {
            if collapse {
                self.scene_state.collapsed.insert(target);
            } else {
                self.scene_state.collapsed.remove(&target);
            }
        }
        self.save_hierarchy_collapsed();
    }

    /// 单个折叠状态变化；折叠属于编辑器配置，不写入 `.scene`。
    fn set_hierarchy_collapsed(&mut self, id: SceneEntityId, collapse: bool) {
        if collapse {
            self.scene_state.collapsed.insert(id);
        } else {
            self.scene_state.collapsed.remove(&id);
        }
        self.save_hierarchy_collapsed();
    }

    /// 以项目根相对路径作为场景标识（统一 `/` 分隔），保证跨会话稳定。
    fn hierarchy_scene_key(&self) -> Option<String> {
        let path = self.current_scene()?.path.as_deref()?;
        let clean_path = crate::util::strip_verbatim_prefix(path);
        let project_root = crate::util::strip_verbatim_prefix(&self.project.root);
        let relative = clean_path
            .strip_prefix(&project_root)
            .unwrap_or(&clean_path);
        let key = relative.to_string_lossy().replace('\\', "/");
        if key.is_empty() { None } else { Some(key) }
    }

    /// 保存当前场景折叠状态；Play 模式保持原快照机制，不落盘运行期变化。
    pub(crate) fn save_hierarchy_collapsed(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(key) = self.hierarchy_scene_key() else {
            return;
        };
        let collapsed: std::collections::BTreeSet<u64> =
            self.scene_state.collapsed.iter().map(|id| id.0).collect();
        crate::util::save_hierarchy_collapsed(&self.project.root, &key, &collapsed);
    }

    /// 打开/恢复文档后读取配置；新建或未保存文档没有稳定场景标识，保持全展开。
    pub(crate) fn load_hierarchy_collapsed(&mut self) {
        let Some(document) = self.current_scene() else {
            self.scene_state.collapsed.clear();
            return;
        };
        let Some(key) = self.hierarchy_scene_key() else {
            self.scene_state.collapsed.clear();
            return;
        };
        let collapsed: std::collections::BTreeSet<u64> =
            crate::util::load_hierarchy_collapsed(&self.project.root, &key);
        self.scene_state.collapsed = collapsed
            .into_iter()
            .map(SceneEntityId)
            .filter(|id| document.scene.entities.contains_key(id))
            .collect();
    }

    /// 文档切换时加载/清空；同文档编辑后剔除悬空 ID 并即时落盘。
    pub(crate) fn sync_hierarchy_collapsed(&mut self, change: crate::actions::DocumentChange) {
        match change {
            crate::actions::DocumentChange::Opened => self.load_hierarchy_collapsed(),
            crate::actions::DocumentChange::Created => self.scene_state.collapsed.clear(),
            crate::actions::DocumentChange::PlaybackStopped => {}
            _ => {
                let valid: std::collections::BTreeSet<SceneEntityId> = self
                    .current_scene()
                    .map(|document| document.scene.entities.keys().copied().collect())
                    .unwrap_or_default();
                let had_stale_id = self
                    .scene_state
                    .collapsed
                    .iter()
                    .any(|id| !valid.contains(id));
                self.scene_state.collapsed.retain(|id| valid.contains(id));
                if had_stale_id {
                    self.save_hierarchy_collapsed();
                }
            }
        }
    }

    /// 收集 `id` 及其全部后代（含自身）。
    fn subtree_ids(&self, id: SceneEntityId) -> Vec<SceneEntityId> {
        let Some(document) = self.current_scene() else {
            return Vec::new();
        };
        let mut out = vec![id];
        let mut queue = vec![id];
        while let Some(current) = queue.pop() {
            if let Some(entity) = document.scene.entities.get(&current) {
                for child in &entity.children {
                    out.push(*child);
                    queue.push(*child);
                }
            }
        }
        out
    }

    /// H03：拖拽落点结算。多选时移动选中集合的顶层根，整批校验、整批一个事务；
    /// 任一目标非法整批取消（事务快照回滚）。
    fn handle_hierarchy_drop(
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
        for moving_id in &batch {
            if let Err(failure) = self
                .tree
                .reparent_at(scene, *moving_id, target_id, position)
            {
                error = Some(failure.to_string());
                break;
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

    /// H03：选中集合中的顶层根 —— 祖先也在选中集合内的子孙剔除，
    /// 用于批量删除/拖拽（整棵子树随根移动或删除）。
    pub(crate) fn top_most_selected_ids(&self) -> Vec<SceneEntityId> {
        let Some(scene) = self.current_scene() else {
            return Vec::new();
        };
        let selected: std::collections::BTreeSet<SceneEntityId> =
            self.selection.ids().into_iter().collect();
        let mut result = Vec::new();
        for id in &selected {
            let mut is_top = true;
            let mut cursor = scene
                .scene
                .entities
                .get(id)
                .and_then(|entity| entity.parent);
            while let Some(parent) = cursor {
                if selected.contains(&parent) {
                    is_top = false;
                    break;
                }
                cursor = scene
                    .scene
                    .entities
                    .get(&parent)
                    .and_then(|entity| entity.parent);
            }
            if is_top {
                result.push(*id);
            }
        }
        result
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

    /// H01：实体行右键 / `⋮` 共用菜单。Play 模式的锁定提示由各命令入口自查。
    fn draw_entity_context_menu(&mut self, ui: &mut egui::Ui, id: SceneEntityId) {
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
            return;
        }
        let parent = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
            .and_then(|entity| entity.parent);
        if ui.button("新建子节点").clicked() {
            self.entity_name = "New Entity".to_owned();
            self.add_entity(Some(id));
            ui.close();
        }
        if ui.button("新建同级节点").clicked() {
            self.entity_name = "New Entity".to_owned();
            self.add_entity(parent);
            ui.close();
        }
        if ui.button("重命名").clicked() {
            self.begin_hierarchy_rename(id);
            ui.close();
        }
        if ui.button("复制").clicked() {
            if !self.selection.contains(id) {
                self.select_single_hierarchy(id);
            }
            self.copy_selected_entity();
            ui.close();
        }
        if ui.button("粘贴为子节点").clicked() {
            self.paste_entity(Some(id));
            ui.close();
        }
        ui.separator();
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
        ui.separator();
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
            if !self.selection.contains(id) {
                self.select_single_hierarchy(id);
            }
            self.reparent_selected(None);
            ui.close();
        }
        // H03：多选批量操作。
        if self.selection.len() > 1 {
            ui.separator();
            if ui
                .button(format!("删除全部选中（{}）", self.selection.len()))
                .clicked()
            {
                self.delete_selected_entity();
                ui.close();
            }
            if ui.button("隐藏全部选中").clicked() {
                for target in self.selection.ids() {
                    self.scene_state.hidden.insert(target);
                    self.persist_hierarchy_flag(target, "hidden", true);
                }
                ui.close();
            }
            if ui.button("锁定全部选中").clicked() {
                for target in self.selection.ids() {
                    self.scene_state.locked.insert(target);
                    self.persist_hierarchy_flag(target, "locked", true);
                }
                ui.close();
            }
        }
        ui.separator();
        if ui.button("删除").clicked() {
            if !self.selection.contains(id) {
                self.select_single_hierarchy(id);
            }
            self.delete_selected_entity();
            ui.close();
        }
    }

    /// H01：头部「+」/ 空白右键的创建菜单。父级由调用方给出
    /// （「+」取当前主选中，即 Unity 的「创建为选中对象的子节点」习惯）。
    fn draw_hierarchy_create_menu(&mut self, ui: &mut egui::Ui, parent: Option<SceneEntityId>) {
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
            return;
        }
        if ui.button("新建空实体").clicked() {
            self.entity_name = "New Entity".to_owned();
            self.add_entity(parent);
            ui.close();
        }
        if ui.button("新建 3D 实体").clicked() {
            self.entity_name = "3D Entity".to_owned();
            self.add_3d_entity(parent);
            ui.close();
        }
        let has_clipboard = self.hierarchy_clipboard.is_some();
        if ui
            .add_enabled(has_clipboard, egui::Button::new("粘贴"))
            .clicked()
        {
            self.paste_entity(parent);
            ui.close();
        }
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
        let name_matches = filter_active && name.to_lowercase().contains(&query.to_lowercase());
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
        let is_3d = entity.components.contains_key("Transform3D");
        let row_id = egui::Id::new(("hierarchy_entry", id.0));
        let renaming = self
            .hierarchy_rename
            .as_ref()
            .is_some_and(|session| session.id == id);
        let (row_zone, drop_payload) =
            ui.dnd_drop_zone::<SceneEntityId, _>(egui::Frame::default(), |ui| {
                ui.horizontal(|ui| {
                    // H06：缩进参考线（每层一条淡色竖线）。
                    let row_left = ui.cursor().left();
                    let row_top = ui.cursor().top();
                    let guide_color = ui
                        .visuals()
                        .widgets
                        .noninteractive
                        .fg_stroke
                        .color
                        .gamma_multiply(0.35);
                    for level in 0..depth {
                        let x = row_left + level as f32 * 14.0 + 6.0;
                        ui.painter().line_segment(
                            [egui::pos2(x, row_top + 2.0), egui::pos2(x, row_top + 22.0)],
                            egui::Stroke::new(1.0_f32, guide_color),
                        );
                    }
                    ui.add_space(depth as f32 * 14.0);
                    // H10：行首控件统一 18×18、1px 间距；折叠箭头为绘制的三角形。
                    let active_color = ui.visuals().text_color();
                    let weak_color = ui.visuals().weak_text_color();
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        if !children.is_empty() {
                            let arrow_response = triangle_disclosure_button(
                                ui,
                                !effective_collapsed,
                                "展开或折叠子节点（Alt+点击折叠整棵子树）",
                            );
                            if arrow_response.clicked() {
                                // H06：Alt+点击 折叠/展开整棵子树。
                                self.set_subtree_collapsed(id, !effective_collapsed);
                            }
                            arrow_response.context_menu(|ui| {
                                self.draw_entity_context_menu(ui, id);
                            });
                        } else {
                            ui.add_space(18.0);
                        }
                        let eye_text = egui::RichText::new(if hidden { "◌" } else { "◉" })
                            .color(if hidden { active_color } else { weak_color });
                        let eye_response = ui
                            .add_sized([18.0, 18.0], egui::Button::new(eye_text))
                            .on_hover_text("显示或隐藏实体");
                        if eye_response.clicked() {
                            if hidden {
                                self.scene_state.hidden.remove(&id);
                                self.persist_hierarchy_flag(id, "hidden", false);
                            } else {
                                self.scene_state.hidden.insert(id);
                                self.persist_hierarchy_flag(id, "hidden", true);
                            }
                        }
                        eye_response.context_menu(|ui| {
                            self.draw_entity_context_menu(ui, id);
                        });
                        let lock_text = egui::RichText::new(if locked { "⊠" } else { "⊡" })
                            .color(if locked { active_color } else { weak_color });
                        let lock_response = ui
                            .add_sized([18.0, 18.0], egui::Button::new(lock_text))
                            .on_hover_text("锁定或解锁实体");
                        if lock_response.clicked() {
                            if locked {
                                self.scene_state.locked.remove(&id);
                                self.persist_hierarchy_flag(id, "locked", false);
                            } else {
                                self.scene_state.locked.insert(id);
                                self.persist_hierarchy_flag(id, "locked", true);
                            }
                        }
                        lock_response.context_menu(|ui| {
                            self.draw_entity_context_menu(ui, id);
                        });
                    });
                    let type_prefix = format!(
                        "{}{}",
                        if ui_root { "[UI Root] " } else { "" },
                        if is_3d { "[3D] " } else { "" }
                    );
                    if renaming {
                        // H02：行内重命名输入框（Enter 提交 / Esc 取消）。
                        self.draw_hierarchy_rename_edit(ui, id);
                    } else {
                        // H05：过滤命中高亮。
                        let mut text =
                            egui::RichText::new(format!("{type_prefix}{name} [{}]", id.0));
                        if name_matches {
                            text = text.color(egui::Color32::from_rgb(255, 200, 100));
                        }
                        let name_response =
                            ui.add_enabled(!locked, egui::Button::selectable(selected, text));
                        // H02：双击进入行内重命名。
                        if name_response.double_clicked() {
                            self.begin_hierarchy_rename(id);
                        }
                        // H03：Ctrl/Cmd 加减选，Shift 范围选，普通点击单选。
                        if name_response.clicked() {
                            let additive =
                                ui.input(|input| input.modifiers.command || input.modifiers.ctrl);
                            let range = ui.input(|input| input.modifiers.shift);
                            if additive {
                                self.selection.toggle(id, SelectionSource::Hierarchy);
                                self.selection_anchor = Some(id);
                                if let Some(primary) = self.selection.primary() {
                                    self.sync_inspector(primary);
                                } else {
                                    self.clear_inspector();
                                }
                            } else if range {
                                self.shift_range_select(id);
                            } else {
                                self.select_single_hierarchy(id);
                            }
                        }
                        // H01：行右键菜单；右键未选中实体时先单选。
                        if name_response.secondary_clicked() && !selected {
                            self.select_single_hierarchy(id);
                        }
                        name_response.context_menu(|ui| {
                            self.draw_entity_context_menu(ui, id);
                        });
                    }
                })
            });
        let row_rect = row_zone.response.rect;
        // H07：行级拖拽检测——hover overlay 不抢 click/drag，移动阈值后才设 payload，
        // 消除 dnd_drag_source 的按下即拖 + Grab 强制光标。
        let row_hover = ui.interact(row_rect, row_id, egui::Sense::hover());
        let should_drag = row_hover.contains_pointer()
            && ui.input(|i| i.pointer.primary_down() && i.pointer.is_decidedly_dragging());
        if should_drag && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            egui::DragAndDrop::set_payload(ui.ctx(), id);
        }
        let drag_target_state = drop_payload
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
                let fraction = (pointer_y - row_rect.top()) / row_rect.height();
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
            let rect = row_rect;
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
        if let Some(dropped_id) = drop_payload.as_ref().map(|id| **id)
            && dropped_id != id
        {
            // H03：落点结算移入 helper —— 多选拖拽批量移动 + 单事务原子回滚。
            self.handle_hierarchy_drop(dropped_id, id, drop_position);
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

    /// 测试夹具：Root(1) -> A(2), Leaf(3)；Solo(4) 为第二个根。
    fn sample_document() -> SceneDocument {
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        document
            .scene
            .add_entity(SceneEntityId(2), "A", Some(SceneEntityId(1)))
            .unwrap();
        document
            .scene
            .add_entity(SceneEntityId(3), "Leaf", Some(SceneEntityId(1)))
            .unwrap();
        document
            .scene
            .add_entity(SceneEntityId(4), "Solo", None)
            .unwrap();
        document
    }

    fn make_app(document: SceneDocument) -> crate::EditorApp {
        let mut app = crate::EditorApp::new(
            editor::ProjectInfo {
                root: std::env::temp_dir()
                    .join(format!("p2-hierarchy-opts-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.document.current_scene = Some(document);
        app
    }

    #[test]
    fn hierarchy_visible_ids_respects_collapse_and_filter() {
        let mut app = make_app(sample_document());
        app.scene_state.collapsed.insert(SceneEntityId(1));
        assert_eq!(
            app.hierarchy_visible_ids(),
            vec![SceneEntityId(1), SceneEntityId(4)]
        );
        app.hierarchy_filter = "Leaf".to_owned();
        // Leaf 是 Root 的直接子节点：A 不是 Leaf 的祖先，应被过滤掉。
        assert_eq!(
            app.hierarchy_visible_ids(),
            vec![SceneEntityId(1), SceneEntityId(3)]
        );
    }

    #[test]
    fn shift_range_select_covers_visible_range_and_falls_back_to_single() {
        let mut app = make_app(sample_document());
        app.select_single_hierarchy(SceneEntityId(2));
        app.shift_range_select(SceneEntityId(4));
        assert!(app.selection.contains(SceneEntityId(2)));
        assert!(app.selection.contains(SceneEntityId(3)));
        assert!(app.selection.contains(SceneEntityId(4)));
        assert_eq!(app.selection.primary(), Some(SceneEntityId(4)));

        // 锚点被折叠隐藏时退化为单选。
        app.selection
            .set_single(SceneEntityId(4), editor::SelectionSource::Hierarchy);
        app.scene_state.collapsed.insert(SceneEntityId(1));
        app.selection_anchor = Some(SceneEntityId(2));
        app.shift_range_select(SceneEntityId(1));
        assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
        assert_eq!(app.selection.len(), 1);
    }

    #[test]
    fn top_most_selected_ids_drop_descendants_of_selected_ancestors() {
        let mut app = make_app(sample_document());
        app.selection.set_many(
            [SceneEntityId(1), SceneEntityId(2)],
            None,
            editor::SelectionSource::Command,
        );
        assert_eq!(app.top_most_selected_ids(), vec![SceneEntityId(1)]);
    }

    #[test]
    fn hierarchy_collapsed_state_roundtrips_per_scene_and_drops_stale_ids() {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("p2-hierarchy-persist-{token}"));
        let mut document = sample_document();
        document.path = Some(root.join("scenes/main.scene"));
        let mut app = make_app(document);

        app.scene_state
            .collapsed
            .extend([SceneEntityId(1), SceneEntityId(2), SceneEntityId(99)]);
        app.save_hierarchy_collapsed();
        app.scene_state.collapsed.clear();
        app.scene_state.collapsed.insert(SceneEntityId(99));
        app.load_hierarchy_collapsed();

        assert_eq!(
            app.scene_state.collapsed,
            std::collections::BTreeSet::from([SceneEntityId(1), SceneEntityId(2)])
        );
    }

    #[test]
    fn duplicate_in_place_sits_right_after_source_and_undo_restores() {
        let mut app = make_app(sample_document());
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);
        app.duplicate_selected_entity();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        // 复制体紧随源节点之后：Root -> [A, A, Leaf]，Solo 保持在后。
        assert_eq!(
            scene.entities[&SceneEntityId(1)].children,
            vec![SceneEntityId(2), SceneEntityId(5), SceneEntityId(3)]
        );
        assert_eq!(scene.entities[&SceneEntityId(5)].name, "A");
        assert_eq!(
            scene.entities[&SceneEntityId(5)].parent,
            Some(SceneEntityId(1))
        );

        app.undo_with_transaction_guard();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert_eq!(
            scene.entities[&SceneEntityId(1)].children,
            vec![SceneEntityId(2), SceneEntityId(3)]
        );
        assert!(!scene.entities.contains_key(&SceneEntityId(5)));
    }

    #[test]
    fn inline_rename_rejects_empty_and_commits_valid_name() {
        let mut app = make_app(sample_document());
        app.select_single_hierarchy(SceneEntityId(2));
        app.begin_hierarchy_rename(SceneEntityId(2));

        // 空名：会话保留待重试，名称不变、不产生历史。
        app.hierarchy_rename.as_mut().unwrap().buffer = "   ".to_owned();
        app.commit_hierarchy_rename(SceneEntityId(2));
        assert!(app.hierarchy_rename.is_some());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&SceneEntityId(2)].name,
            "A"
        );

        app.hierarchy_rename.as_mut().unwrap().buffer = "Hero".to_owned();
        app.commit_hierarchy_rename(SceneEntityId(2));
        assert!(app.hierarchy_rename.is_none());
        assert_eq!(
            app.document.current_scene.as_ref().unwrap().scene.entities[&SceneEntityId(2)].name,
            "Hero"
        );
    }

    #[test]
    fn batch_delete_removes_selected_roots_in_one_undo_step() {
        let mut app = make_app(sample_document());
        app.selection.set_many(
            [SceneEntityId(2), SceneEntityId(4)],
            Some(SceneEntityId(2)),
            editor::SelectionSource::Hierarchy,
        );
        app.delete_selected_entity();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert!(!scene.entities.contains_key(&SceneEntityId(2)));
        assert!(!scene.entities.contains_key(&SceneEntityId(4)));
        assert_eq!(
            scene.entities[&SceneEntityId(1)].children,
            vec![SceneEntityId(3)]
        );

        app.undo_with_transaction_guard();
        let scene = &app.document.current_scene.as_ref().unwrap().scene;
        assert!(scene.entities.contains_key(&SceneEntityId(2)));
        assert!(scene.entities.contains_key(&SceneEntityId(4)));
    }
}
