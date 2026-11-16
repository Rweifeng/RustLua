//! 层级面板的导航与选择：键盘动作、范围选、折叠子树、聚焦（M79 结构切分，零行为变化）。
use super::*;

impl EditorApp {
    // `set_ui_root` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

    pub(super) fn focus_selected_scene_view(&mut self) {
        let Some(id) = self.selection.primary() else {
            self.status = "请先在层级面板中选择实体".to_owned();
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
                .get("Transform")
                .and_then(|value| Transform3D::from_component_str(value).ok())
                .map(|transform| transform.position)
                .unwrap_or(rustlua_math::math3d::Vec3::ZERO);
            self.editor_camera.target = target;
            self.status = "3D 场景视图已聚焦实体".to_owned();
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
        self.status = "场景视图已聚焦实体".to_owned();
    }

    pub(crate) fn copy_selected_entity(&mut self) {
        let Some(id) = self.selection.primary() else {
            self.status = "请先在层级面板中选择实体".to_owned();
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
        self.status = "实体已复制到剪贴板".to_owned();
    }

    /// H03：单选并更新 Shift 锚点（Hierarchy 内统一入口）。
    pub(crate) fn select_single_hierarchy(&mut self, id: SceneEntityId) {
        self.selection.set_single(id, SelectionSource::Hierarchy);
        self.selected_resource = None;
        self.selection_anchor = Some(id);
        self.sync_inspector(id);
    }

    /// H03：Shift+点击范围选择 —— 取锚点到点击行之间的可见连续区间。
    /// 锚点不可见（被过滤/折叠/切换场景）时退化为单选。
    pub(super) fn shift_range_select(&mut self, id: SceneEntityId) {
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

    /// H04：Hierarchy 键盘操作。仅当指针悬停面板、无文本输入焦点且无重命名会话时生效。
    pub(super) fn handle_hierarchy_shortcuts(&mut self, ui: &egui::Ui, context: &egui::Context) {
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
    pub(super) fn move_hierarchy_selection(&mut self, delta: i32) {
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
    pub(super) fn collapse_or_jump_parent(&mut self) {
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
    pub(super) fn expand_or_enter_child(&mut self) {
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
    pub(super) fn set_subtree_collapsed(&mut self, id: SceneEntityId, collapse: bool) {
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
    pub(super) fn set_hierarchy_collapsed(&mut self, id: SceneEntityId, collapse: bool) {
        if collapse {
            self.scene_state.collapsed.insert(id);
        } else {
            self.scene_state.collapsed.remove(&id);
        }
        self.save_hierarchy_collapsed();
    }

    /// 收集 `id` 及其全部后代（含自身）。
    pub(super) fn subtree_ids(&self, id: SceneEntityId) -> Vec<SceneEntityId> {
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

    // `handle_hierarchy_drop` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）：
    // 换父的事务/keep-world 提交边界不在面板里（面板只把命中的行与落点发出来）。

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
}
