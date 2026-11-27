//! 层级面板的绘制：树壳/搜索、行绘制、右键菜单与重命名（M79 结构切分，零行为变化）。
use super::*;

impl EditorApp {
    // `paste_entity` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）：提交边界不在面板里。

    /// 层级面板外壳（`Context` 入口只服务 `perf_baseline` 的合成帧；生产路径由 dock
    /// 标签直接渲染 [`Self::draw_hierarchy_body`]）。
    pub(crate) fn draw_hierarchy_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scene_tree")
            .resizable(true)
            // 宽度来自持久化布局（docs/55 P1：可保存布局）。
            .default_width(self.layout.panel_width(editor::PANEL_HIERARCHY))
            .min_width(110.0)
            .show(context, |ui| self.draw_hierarchy_body(ui));
    }

    /// 层级面板主体（M27-00 阶段 1：dock 标签直接渲染，外壳宽度只服务合成帧路径）。
    pub(crate) fn draw_hierarchy_body(&mut self, ui: &mut egui::Ui) {
        // M81：工作区停在 UI 工作室标签时，本面板整体切成 UI 节点树（与属性面板内嵌
        // UI 检查器同一套上下文替换）；切回 Scene/Game 即回到实体层级。
        if self.draw_ui_studio_hierarchy_section(ui) {
            return;
        }
        // 场景文件集成：标题 + 「+」创建菜单 + 当前场景保存状态。
        ui.horizontal(|ui| {
            ui.heading("层级");
            // Play：这份树是**运行态镜像**（脚本 spawn/destroy 会改它，Stop 后回到编辑文档），
            // 标出来免得作者以为改行会写进场景文件。
            if self.is_playing() {
                ui.colored_label(egui::Color32::from_rgb(120, 200, 255), "运行态");
            }
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
        // P1-4：预制体 in-context 编辑横幅（阶段里树显示的是**源**，不是场景）。
        self.draw_prefab_stage_banner(ui);
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
        //
        // docs/55 阶段 0：把输入框矩形记进探针，GUI smoke 才能用**真实指针事件**把焦点
        // 打进文本框（"输入焦点"是计划 §8 质量门枚举的一项：键入时快捷键不得抢键）。
        // right-to-left：清除按钮先画靠右，TextEdit 后画用 INFINITY 占满剩余宽度。
        // 之前按 available_width()-34 算 desired_width，输入文字比这个宽时 TextEdit
        // 的内容 galley 会反向撑大自身，导致输入框越输越宽（与 schema_inspector 同一问题）。
        let filter_response = ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            // 空时也在按钮位分配 0 宽占位：保持 TextEdit 的 widget 序号稳定，
            // 否则输入首字符时清除按钮出现会让 TextEdit 序号 +1，egui 视为新控件而丢焦点。
            if !self.hierarchy_filter.is_empty()
                && crate::util::draw_icon_button(ui, crate::util::DrawIcon::Cross, "清空过滤")
                    .clicked()
            {
                self.hierarchy_filter.clear();
            } else {
                ui.allocate_exact_size(egui::vec2(0.0, 0.0), egui::Sense::hover());
            }
            // 显式固定 widget ID：按钮出现/消失会改变自动序号，导致 TextEdit 被当成新控件而丢焦点。
            let edit_response = ui.add(
                egui::TextEdit::singleline(&mut self.hierarchy_filter)
                    .id(ui.make_persistent_id("hierarchy_search_box"))
                    // 语法提示直接写在占位文本里：`type:`/`component:` 是 ER-04 的"类型过滤"。
                    .hint_text("搜索实体（type:Sprite 或 component:ui 按组件过滤）")
                    .desired_width(f32::INFINITY),
            );
            edit_response.rect
        });
        // 探针：GUI smoke 用它把合成点击打在输入框真正的位置上。
        self.hierarchy_filter_rect = filter_response.inner;
        if !self.hierarchy_filter.trim().is_empty() {
            ui.small(format!("匹配 {} 项", self.hierarchy_match_count()));
        }
        ui.separator();
        // 预制体徽标（嵌套 / 覆盖）与行模型同一次修订触发：编辑一次算一次，命中缓存时不碰磁盘。
        self.ensure_prefab_badges();
        // H3：从场景视图点选 / 粘贴 / 撤销恢复过来的实体，若被折叠挡在树里就**自动揭示**：
        // 展开祖先并请求滚动到它所在的行。只在"主选中真的换了"时触发，
        // 因此用户手动折叠正在选中的节点不会被下一帧抢回去。
        self.maybe_reveal_primary_selection();
        // 扁平化 + 虚拟滚动：只绘制视口内的行（10k 实体保持可交互）。
        // 行模型按 (修订, 实体数, 过滤词, 折叠集合) 缓存，绘制不再递归、不再克隆文档。
        let rows = self.hierarchy_rows();
        let row_count = rows.len();
        let body_height = ui.available_height().max(48.0);
        let row_height = hierarchy_row_height(ui);
        let mut reveal_target = self.hierarchy_scroll_target;
        self.hierarchy_row_probe.reset();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .max_height(body_height)
            .show_rows(ui, row_height, row_count, |ui, visible| {
                for index in visible {
                    if let Some(row) = rows.get(index).copied() {
                        self.draw_entity_row(ui, row, row_height);
                        // 目标行落在视口里：把它滚到中间（只请求一次）。
                        if reveal_target == Some(row.id) {
                            ui.scroll_to_cursor(Some(egui::Align::Center));
                            reveal_target = None;
                        }
                    }
                }
            });
        if self.hierarchy_scroll_target.is_some() && reveal_target.is_none() {
            self.hierarchy_scroll_target = None;
        }
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
        }
        // H04：键盘导航（↑↓←→ / F2 / F）。指针悬停面板且无文本焦点时生效。
        self.handle_hierarchy_shortcuts(ui, ui.ctx());
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
    }

    /// 过滤匹配（单实体语义入口）。
    ///
    /// 面板绘制与扁平化都走 [`HierarchyMatchIndex`]（一次遍历 + O(1) 查询），
    /// 此函数仅作为"自身或后代命中"语义的测试基准保留在测试构建中。
    #[cfg(test)]
    pub(super) fn hierarchy_filter_matches(
        &mut self,
        scene: &editor::SceneDocument,
        id: SceneEntityId,
    ) -> bool {
        let filter = HierarchyFilter::parse(&self.hierarchy_filter);
        if filter.is_empty() {
            return scene.scene.entities.contains_key(&id);
        }
        let lower = self.hierarchy_lower.ensure(scene);
        HierarchyMatchIndex::build(scene, &filter, lower)
            .state(id)
            .subtree_matches
    }

    /// H05：**自身命中**过滤查询的实体数，用于过滤框下的计数提示。
    ///
    /// 先确保扁平化缓存与**当前**状态一致（过滤词/折叠/修订都属于缓存键），
    /// 再取一次性算好的计数：过滤时每帧一次全量小写化扫描已无必要。
    pub(super) fn hierarchy_match_count(&mut self) -> usize {
        self.ensure_hierarchy_rows();
        self.hierarchy_visible_cache.match_count
    }

    /// 过滤匹配数（`pub(crate)` 版）：GUI smoke 要在**帧外**断言"输入真的缩窄了树"，
    /// 而上面那个私有版本需要在 `&mut self` 上跑一次扁平化——这正是我们要验证的东西。
    pub(crate) fn filtered_match_count(&mut self) -> usize {
        self.hierarchy_match_count()
    }

    /// H03/H04：当前帧可见顺序表（过滤 + 折叠后的先序遍历，含每行缩进深度）。
    ///
    /// 结果按 (文档修订, 实体数, 过滤词, 折叠集合) 缓存：导航、Shift 范围选与虚拟滚动绘制
    /// 在同一帧内共享**同一次**扁平化，既不再重复递归整棵树，也不再逐行克隆文档。
    /// 返回值是共享句柄（`Arc`），命中缓存时只做引用计数 +1，不复制行数据。
    pub(crate) fn hierarchy_rows(&mut self) -> std::sync::Arc<Vec<HierarchyRow>> {
        self.ensure_hierarchy_rows();
        self.hierarchy_visible_cache.rows.clone()
    }

    /// 重建（必要时）扁平化缓存；不返回数据、不复制，供绘制与计数使用。
    pub(super) fn ensure_hierarchy_rows(&mut self) {
        let mut collapsed: Vec<_> = self.scene_state.collapsed.iter().copied().collect();
        collapsed.sort_unstable_by_key(|id| id.0);
        let filter = self.hierarchy_filter.trim().to_owned();
        let parsed_filter = HierarchyFilter::parse(&filter);
        let filter_active = !parsed_filter.is_empty();
        let mut out = Vec::new();
        let (revision, entity_count, match_count) = {
            // Play 期间行模型读**运行态镜像**（脚本 spawn/destroy 的行只有回灌进镜像后才
            // 在这里出现）；编辑期间它与 `document.current_scene` 是同一份。
            let Some(document) =
                crate::play_sync::panel_scene_document(&self.play_session, &self.document)
            else {
                self.hierarchy_visible_cache = HierarchyVisibleCache::default();
                return;
            };
            let revision = document.revision;
            let entity_count = document.scene.entities.len();
            if self.hierarchy_visible_cache.valid
                && self.hierarchy_visible_cache.revision == revision
                && self.hierarchy_visible_cache.entity_count == entity_count
                && self.hierarchy_visible_cache.filter == filter
                && self.hierarchy_visible_cache.collapsed == collapsed
            {
                self.profiler.bump("hierarchy_cache_hit");
                return;
            }
            self.profiler.bump("hierarchy_cache_miss");
            let roots: Vec<SceneEntityId> =
                self.tree.roots(document).map(|entity| entity.id).collect();
            // 一次遍历算出过滤匹配索引，扁平化与计数共用（取代逐行递归 + 重复小写化）。
            // 小写化名称/组件名表按文档修订缓存：改过滤词只做 `contains`，不再重算 10k 条。
            let index = if filter_active {
                let lower = self.hierarchy_lower.ensure(document);
                Some(HierarchyMatchIndex::build(document, &parsed_filter, lower))
            } else {
                None
            };
            let match_count = index
                .as_ref()
                .map_or(entity_count, HierarchyMatchIndex::match_count);
            self.collect_visible_rows(document, &roots, index.as_ref(), 0, &mut out);
            (revision, entity_count, match_count)
        };
        self.hierarchy_visible_cache.valid = true;
        self.hierarchy_visible_cache.revision = revision;
        self.hierarchy_visible_cache.entity_count = entity_count;
        self.hierarchy_visible_cache.filter = filter;
        self.hierarchy_visible_cache.collapsed = collapsed;
        self.hierarchy_visible_cache.match_count = match_count;
        self.hierarchy_visible_cache.parsed_filter = parsed_filter;
        self.hierarchy_visible_cache.rows = std::sync::Arc::new(out);
    }

    /// 可见实体 id 顺序（键盘导航、Shift 范围选、测试入口）。
    pub(crate) fn hierarchy_visible_ids(&mut self) -> Vec<SceneEntityId> {
        self.hierarchy_rows().iter().map(|row| row.id).collect()
    }

    pub(super) fn collect_visible_rows(
        &self,
        document: &editor::SceneDocument,
        ids: &[SceneEntityId],
        index: Option<&HierarchyMatchIndex>,
        depth: usize,
        out: &mut Vec<HierarchyRow>,
    ) {
        for id in ids {
            if let Some(index) = index
                && !index.state(*id).subtree_matches
            {
                continue;
            }
            let Some(entity) = document.scene.entities.get(id) else {
                continue;
            };
            let has_children = !entity.children.is_empty();
            let collapsed = self.scene_state.collapsed.contains(id);
            let force_expand = index.is_some_and(|index| {
                entity
                    .children
                    .iter()
                    .any(|child| index.state(*child).subtree_matches)
            });
            let effective_collapsed = collapsed && !force_expand;
            out.push(HierarchyRow {
                id: *id,
                depth,
                effective_collapsed,
                has_children,
            });
            if !effective_collapsed && has_children {
                let children = entity.children.clone();
                self.collect_visible_rows(document, &children, index, depth + 1, out);
            }
        }
    }

    /// P1-4：预制体 in-context 编辑横幅。
    ///
    /// 阶段里这份树显示的是**源预制体**（不是场景），所以必须明确告诉用户"你现在改的是
    /// 源，返回时同步 N 个实例"——否则很容易改完就去找场景、以为改动丢了。
    /// 面板只读 `prefab_stage_banner()` 的事实并发 typed action，退出路径的实现体在
    /// shell（`crate::actions::prefab_stage`）。
    pub(crate) fn draw_prefab_stage_banner(&mut self, ui: &mut egui::Ui) {
        // 先拷成拥有值的字符串：`prefab_stage_banner()` 借用了 self，
        // 下面每个按钮都要 `&mut self`。
        let Some((source, instances)) = self.prefab_stage_banner().map(|(s, n)| (s.to_owned(), n))
        else {
            return;
        };
        let unsaved = self.prefab_stage_unsaved();
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(120, 200, 255), "预制体编辑中");
            ui.label(format!("{source}｜返回时同步 {instances} 个实例"));
        });
        ui.horizontal(|ui| {
            if ui
                .button(if unsaved {
                    "保存并返回 ●"
                } else {
                    "保存并返回"
                })
                .clicked()
            {
                self.dispatch(editor::EditorCommand::ExitPrefabStage { save: true });
            }
            if ui.button("放弃并返回").clicked() {
                self.dispatch(editor::EditorCommand::ExitPrefabStage { save: false });
            }
        });
        ui.separator();
    }

    /// H02：进入行内重命名（F2 / 双击 / 菜单）。
    pub(super) fn begin_hierarchy_rename(&mut self, id: SceneEntityId) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
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

    // `commit_hierarchy_rename` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

    /// H02：行内重命名输入框。Enter 提交，Esc 取消，点击其他区域提交（Unity 习惯）。
    ///
    /// `row_height` 由调用方按当前样式传入：输入框必须落在行高内，
    /// 否则虚拟滚动的统一行高会被撑破（相邻行重叠 / 文字被裁）。
    pub(super) fn draw_hierarchy_rename_edit(
        &mut self,
        ui: &mut egui::Ui,
        id: SceneEntityId,
        row_height: f32,
    ) {
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
        let size = egui::vec2(ui.available_width().max(60.0), row_height);
        let response = ui.add_sized(size, edit);
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

    // `duplicate_selected_entity` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

    /// H01：实体行右键 / `⋮` 共用菜单。Play 模式的锁定提示由各命令入口自查。
    pub(super) fn draw_entity_context_menu(&mut self, ui: &mut egui::Ui, id: SceneEntityId) {
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
            return;
        }
        let parent = self
            .current_scene()
            .and_then(|scene| scene.scene.entities.get(&id))
            .and_then(|entity| entity.parent);
        if ui.button("新建子节点").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "新实体".to_owned(),
                parent: Some(id),
                transform_3d: false,
            });
            ui.close();
        }
        if ui.button("新建同级节点").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "新实体".to_owned(),
                parent,
                transform_3d: false,
            });
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
        if ui.button("创建预制体").clicked() {
            self.selection.set_single(id, SelectionSource::Hierarchy);
            self.create_prefab_from_selected();
            ui.close();
        }
        // M79 P1-4：根实例才有「进入预制体编辑」。已经在阶段里时不画这一项：
        // 阶段文档不是场景，它的实体上没有 `prefab.root`，但把入口摆出来只会让人以为能套两层。
        if self.prefab_stage().is_none()
            && self.is_prefab_root(id)
            && ui.button("编辑预制体").clicked()
        {
            self.selection.set_single(id, SelectionSource::Hierarchy);
            self.dispatch(editor::EditorCommand::EnterPrefabStage { entity: id });
            ui.close();
        }
        if ui.button("重新应用预制体").clicked() {
            self.selection.set_single(id, SelectionSource::Hierarchy);
            // typed action：目标显式携带（菜单不再依赖"先改选区"这个隐藏前提）。
            self.dispatch(editor::EditorCommand::ReapplyPrefab { entity: id });
            ui.close();
        }
        ui.separator();
        if ui.button("聚焦场景视图").clicked() {
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
            self.dispatch(editor::EditorCommand::ReparentSelection {
                target: None,
                position: editor::ReparentPosition::Child,
            });
            ui.close();
        }
        // H03：多选批量操作。
        if self.selection.len() > 1 {
            ui.separator();
            if ui
                .button(format!("删除全部选中（{}）", self.selection.len()))
                .clicked()
            {
                self.dispatch(editor::EditorCommand::DeleteSelection);
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
            self.dispatch(editor::EditorCommand::DeleteSelection);
            ui.close();
        }
    }

    /// H01：头部「+」/ 空白右键的创建菜单。父级由调用方给出
    /// （「+」取当前主选中，即 Unity 的「创建为选中对象的子节点」习惯）。
    pub(super) fn draw_hierarchy_create_menu(
        &mut self,
        ui: &mut egui::Ui,
        parent: Option<SceneEntityId>,
    ) {
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
            return;
        }
        if ui.button("新建空实体").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "新实体".to_owned(),
                parent,
                transform_3d: false,
            });
            ui.close();
        }
        if ui.button("新建 3D 实体").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "3D 实体".to_owned(),
                parent,
                transform_3d: true,
            });
            ui.close();
        }
        ui.separator();
        if ui.button("新建 UI 实体").clicked() {
            self.create_ui_entity();
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

    /// 绘制一行扁平化实体行（虚拟滚动调用）。
    ///
    /// 与旧递归实现相比：不再 clone 整个文档、不再递归子节点、不再重算过滤可见性
    /// （三件事都由 `hierarchy_rows()` 的缓存行模型提供），每行只做一次轻量字段提取。
    pub(super) fn draw_entity_row(
        &mut self,
        ui: &mut egui::Ui,
        row: HierarchyRow,
        row_height: f32,
    ) {
        let HierarchyRow {
            id,
            depth,
            effective_collapsed,
            has_children,
        } = row;
        // 只提取本行需要的字段（旧实现整份文档 clone，10k 行即 10k 次全量克隆）。
        // 组件名也一起取：`type:` 过滤词要参与"这一行自己是否命中"的高亮判断。
        //
        // 这份行数据必须与**行模型**同源（`ensure_hierarchy_rows` 走 `panel_scene_document`）：
        // Play 期间行模型读运行态镜像，而运行期 `entity.spawn`/`prefab.spawn` 生成的行在
        // 编辑文档里根本不存在——这里若继续只认 `document.current_scene`，那些行会被静默
        // 跳过、画不出来，层级看起来就是空的（空场景 + 整屏运行期 UI 的工程必现）。
        let (name, ui_root, is_3d, lower_components) = {
            let Some(entity) =
                crate::play_sync::panel_scene_document(&self.play_session, &self.document)
                    .and_then(|document| document.scene.entities.get(&id))
            else {
                return;
            };
            (
                entity.name.clone(),
                entity
                    .metadata
                    .get("ui.root")
                    .is_some_and(|value| value == "true"),
                rustlua_ecs::spatial_transform::entity_space(&entity.components)
                    == rustlua_ecs::spatial_transform::SpatialKind::ThreeD,
                entity
                    .components
                    .keys()
                    .map(|component| component.to_lowercase())
                    .collect::<Vec<_>>(),
            )
        };
        // 过滤高亮用**上一次扁平化的解析结果**（缓存里带着它），不再逐行重新解析过滤串。
        let self_matches = !self.hierarchy_visible_cache.parsed_filter.is_empty()
            && self
                .hierarchy_visible_cache
                .parsed_filter
                .matches_self(&name.to_lowercase(), &lower_components);
        let selected = self.selection.contains(id);
        let hidden = self.scene_state.hidden.contains(&id);
        let locked = self.scene_state.locked.contains(&id);
        // 徽标是索引里的一小份拥有值：闭包里要 `&mut self`，不能带着借用进去。
        let row_badges = self.prefab_badges.get(id).cloned();
        // 行高固定（由当前样式推导，见 `hierarchy_row_height`）：虚拟滚动依赖统一行高，
        // 同时保证内容不会比行更高（否则相邻行重叠 / 文字被裁）。
        ui.set_min_height(row_height);
        // 虚拟滚动的直接证据：实际绘制的行数（基线场景断言它远小于实体总数）。
        self.profiler.bump("hierarchy_rows_drawn");
        let row_id = egui::Id::new(("hierarchy_entry", id.0));
        let renaming = self
            .hierarchy_rename
            .as_ref()
            .is_some_and(|session| session.id == id);
        let rename_row_height = (row_height - 4.0).max(14.0);
        let (row_zone, drop_payload) =
            ui.dnd_drop_zone::<crate::dnd::DropPayload, _>(egui::Frame::default(), |ui| {
                let inner = ui.horizontal(|ui| {
                    // H06：缩进参考线（每层一条淡色竖线）与行首控件的缩进由共用原语负责。
                    tree::indent(ui, depth);
                    // H10：行首控件统一 18×18、1px 间距；折叠箭头为绘制的三角形。
                    let active_color = ui.visuals().text_color();
                    let weak_color = ui.visuals().weak_text_color();
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;
                        if has_children {
                            let arrow_response = tree::disclosure_button(
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
                        // 高度受限于行高：虚拟滚动要求行高统一，重命名不能把行撑高。
                        self.draw_hierarchy_rename_edit(ui, id, rename_row_height);
                    } else {
                        // H05：过滤命中高亮（名称词或 `type:` 词命中都算）。
                        let mut text =
                            egui::RichText::new(format!("{type_prefix}{name} [{}]", id.0));
                        if self_matches {
                            text = text.color(egui::Color32::from_rgb(255, 200, 100));
                        }
                        // H4：**锁定只挡场景拾取**（2D/3D 命中与 Gizmo 拖动都跳过锁定实体），
                        // 层级里仍然可以点选、改名、重挂、删除——这些都是用户按下的显式动作。
                        // 旧实现把锁当成"整行禁用"，于是锁住的实体在层级里点不中，
                        // 与"锁"应有的语义正好相反。
                        let name_response =
                            tree::label_button(ui, selected, true, egui::Sense::click(), text);
                        if locked {
                            name_response.clone().on_hover_text(
                                "已锁定：场景视图里不可拾取、不能用 Gizmo 拖动；层级里仍可选中",
                            );
                        }
                        // H02：双击进入行内重命名。
                        // P1-4：预制体**根实例**的双击改道给 in-context 编辑（Unity 同手势），
                        // 根实例改名因此只剩 F2 与右键菜单——代价已写进 docs/25 §23。
                        if name_response.double_clicked() {
                            if self.is_prefab_root(id) && self.prefab_stage().is_none() {
                                self.selection.set_single(id, SelectionSource::Hierarchy);
                                self.dispatch(editor::EditorCommand::EnterPrefabStage {
                                    entity: id,
                                });
                            } else {
                                self.begin_hierarchy_rename(id);
                            }
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
                    // 名字右侧：嵌套 / 覆盖徽标（只读，悬停说明来源与条数）。
                    if let Some(badges) = &row_badges {
                        crate::panels::hierarchy_badges::draw_badges(ui, badges);
                    }
                });
                // 布局探针：记录本行**自然**内容高度，供"内容不得高于行高"的门槛断言。
                inner.response.rect.height()
            });
        // 布局探针：记录本行**自然**内容高度（不含 drop-zone 外框），
        // 供"内容不得高于行高"的门槛断言使用。
        self.hierarchy_row_probe.observe(row_zone.inner);
        let row_rect = row_zone.response.rect;
        // H07：行级拖拽检测——hover overlay 不抢 click/drag，移动阈值后才设 payload，
        // 消除 dnd_drag_source 的按下即拖 + Grab 强制光标。
        let row_hover = ui.interact(row_rect, row_id, egui::Sense::hover());
        let should_drag = row_hover.contains_pointer()
            && ui.input(|i| i.pointer.primary_down() && i.pointer.is_decidedly_dragging());
        if should_drag && !egui::DragAndDrop::has_any_payload(ui.ctx()) {
            egui::DragAndDrop::set_payload(ui.ctx(), crate::dnd::DropPayload::Entity(id));
        }
        let dragged = drop_payload.as_ref().map(|payload| (**payload).clone());
        // M27-01：这一行的落点语义交给 `dnd::plan` —— 实体=重排，资产=建成这一行的子节点。
        // 重排还要过父子合法性校验（不能把自己挂到自己/后代下面），非法时画红框并拒绝。
        let reparent_target = match dragged.as_ref() {
            Some(crate::dnd::DropPayload::Entity(dragged_id)) if *dragged_id != id => {
                Some(*dragged_id)
            }
            _ => None,
        };
        let drag_target_state = reparent_target.and_then(|dragged_id| {
            self.document.current_scene.as_ref().map(|scene| {
                (
                    dragged_id,
                    self.validate_reparent_target(scene, dragged_id, Some(id)),
                )
            })
        });
        let pointer = ui.input(|input| input.pointer.hover_pos());
        let drops_asset = matches!(dragged, Some(crate::dnd::DropPayload::Asset(_)));
        let drop_valid = matches!(drag_target_state, Some((_, Ok(())))) || drops_asset;
        let drop_invalid = matches!(drag_target_state, Some((_, Err(_))));
        // 只有重排才分 Before/After/Child 三条带；资产落点是"成为这一行的子节点"，
        // 画一条横线会把语义说反。
        let drop_slot = if drops_asset {
            tree::DropSlot::Child
        } else {
            pointer
                .map(|position| tree::DropSlot::from_pointer(row_rect, position.y, true))
                .unwrap_or(tree::DropSlot::Child)
        };
        let drop_position = match drop_slot {
            tree::DropSlot::Before => ReparentPosition::Before,
            tree::DropSlot::Child => ReparentPosition::Child,
            tree::DropSlot::After => ReparentPosition::After,
        };
        // 系统文件拖到某一行上：导入并建成这一行的子节点（行矩形互不重叠，谁包住指针谁认领）。
        if let Some((action, drop)) =
            self.claim_external_drop(crate::dnd::DropZone::Hierarchy, row_rect)
        {
            let text = self.commit_spawn_drop(&action, Some(id));
            self.status = format!("{text}{}", drop.notes());
        } else if let Some(action) =
            self.external_hover_action(crate::dnd::DropZone::Hierarchy, row_rect, pointer)
        {
            crate::dnd::paint_drop_feedback(ui, row_rect, 4, &action);
        }
        if drop_valid || drop_invalid {
            tree::paint_drop_target(ui.painter(), row_rect, drop_slot, drop_valid);
        }
        let Some(source) = dragged.map(crate::dnd::DragSource::Internal) else {
            return;
        };
        match (
            reparent_target,
            crate::dnd::plan(&source, crate::dnd::DropZone::Hierarchy),
        ) {
            // H03：重排落点结算移入 helper —— 多选批量移动 + 单事务原子回滚。
            (Some(dragged_id), _) => {
                self.handle_hierarchy_drop(dragged_id, id, drop_position);
            }
            // 把自己的行拖到自己身上：什么也没发生，不该留一条状态噪音。
            (None, crate::dnd::DropAction::Reparent) => {}
            // 资产拖到行上：建实体走同一提交层，父节点=这一行。
            (None, action) => self.status = self.commit_spawn_drop(&action, Some(id)),
        }
    }
}
