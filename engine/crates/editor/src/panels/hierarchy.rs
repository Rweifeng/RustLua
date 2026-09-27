//! 行3 层级面板（资源面板右侧）：场景切换/新建 + 实体树 + 项目信息。

use crate::EditorApp;
use crate::util::{display_path, strip_verbatim_prefix};
use editor::{ReparentPosition, SelectionSource};
use eframe::egui;
use engine::core::{
    prefab::Prefab, resource_db::AssetId, scene::SceneEntityId, transform::Transform3D,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// One flattened Hierarchy row: everything the row renderer needs, captured at flatten time
/// so drawing never walks the tree and never clones the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HierarchyRow {
    pub id: SceneEntityId,
    pub depth: usize,
    /// False when a filter forces this node's children to stay visible while it is collapsed.
    pub effective_collapsed: bool,
    pub has_children: bool,
}

/// Base height of one Hierarchy row. Rows are uniform so the panel can virtualize scrolling
/// and only render the rows inside the viewport (10k entities stay interactive).
///
/// The effective height is derived from the live style by [`hierarchy_row_height`] so user
/// zoom / larger fonts cannot make the content taller than the row it is drawn into
/// (docs/55 §8: 新 UI 在 100/150/200% DPI 存在重叠或裁字不得封板).
pub(crate) const HIERARCHY_ROW_HEIGHT: f32 = 22.0;

/// Effective Hierarchy row height for the current style.
///
/// Row controls are 18×18 point widgets (`interact_size`), so the row needs that plus a
/// little breathing room; the base constant is the floor.
pub(crate) fn hierarchy_row_height(ui: &egui::Ui) -> f32 {
    // 必须跟随样式：硬编码 22 pt 会让 comfortable/large-text 密度下内容（23.25/26.75 pt）
    // 高于行高 → 相邻行重叠、文字被裁。该结论由 `layout_row_height_gate` 实测锁定
    // （负向对照：把这里改回常量即判红）。
    let spacing = ui.spacing();
    (spacing.interact_size.y + spacing.item_spacing.y + 2.0).max(HIERARCHY_ROW_HEIGHT * 0.5)
}

/// Layout probe: the tallest row content actually produced in a frame.
///
/// The virtualization gate is "content never exceeds the row height" (otherwise rows overlap
/// or get clipped). Measured in points, so it is checked under DPI and zoom variations by
/// tests instead of by eyeballing screenshots.
#[derive(Default)]
pub(crate) struct RowHeightProbe {
    max_content_height: f32,
    rows: usize,
}

impl RowHeightProbe {
    pub(crate) fn observe(&mut self, content_height: f32) {
        self.max_content_height = self.max_content_height.max(content_height);
        self.rows += 1;
    }

    pub(crate) fn reset(&mut self) {
        self.max_content_height = 0.0;
        self.rows = 0;
    }

    /// Tallest content height seen since the last reset (0 before any row was drawn).
    pub(crate) fn max_content_height(&self) -> f32 {
        self.max_content_height
    }

    pub(crate) fn rows(&self) -> usize {
        self.rows
    }
}

/// Cached flattened hierarchy rows. The key includes document revision, entity count, filter,
/// and collapse state so navigation, range selection, and drawing share one flatten per state.
#[derive(Default)]
pub(crate) struct HierarchyVisibleCache {
    /// False until the first successful build, so a default-constructed cache can never be
    /// mistaken for a hit on a fresh document whose revision is also 0.
    valid: bool,
    revision: u64,
    /// Entity count guards against structural changes that did not bump the revision
    /// (for example direct scene mutation from tooling or tests).
    entity_count: usize,
    filter: String,
    collapsed: Vec<SceneEntityId>,
    /// 过滤词**自身命中**的实体数（过滤框计数提示，随扁平化一次算出）。
    match_count: usize,
    /// 上一次扁平化用的解析结果：绘制行时复用，避免每行重新解析过滤串。
    parsed_filter: HierarchyFilter,
    /// 行模型用 `Arc` 共享：绘制期间持有它不借用 `self`，重入查询（Shift 范围选等）
    /// 也能拿到同一份数据，且每次取用只是引用计数 +1。
    rows: std::sync::Arc<Vec<HierarchyRow>>,
}

impl HierarchyVisibleCache {
    /// Number of rows in the last successful flatten (0 before the first build).
    pub(crate) fn len(&self) -> usize {
        if self.valid { self.rows.len() } else { 0 }
    }
}

/// 单个实体在过滤下的匹配状态（一次遍历算出，供扁平化整体复用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MatchState {
    /// 实体自身命中过滤查询（名称词 + `type:` 词都满足）。
    pub self_matches: bool,
    /// 自身或任意后代命中（决定该行是否可见）。
    pub subtree_matches: bool,
}

/// Hierarchy 过滤查询（第 41 轮，docs/55 ER-04 的"类型过滤"）。
///
/// 语法刻意最小：空白分隔多个词，**全部为 AND**；
/// - `type:<模式>` / `component:<模式>`：实体必须有**至少一个组件名**包含该模式；
/// - 其余词：实体**名称**必须包含该词。
///
/// 三条刻意行为：
/// 1. 全部大小写不敏感（与既有的名称过滤一致）；
/// 2. `type:` 后为空（刚输入到一半）时该词被忽略——否则每敲一个前缀就会瞬间清空整棵树；
/// 3. `type:` 用的是**子串**匹配而不是精确名：`type:ui` 能一次找出 `UiLayout`/`UiEvents`/
///    `UiAnimation`/`UiBindings`，这是这个过滤器最有用的用法。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HierarchyFilter {
    name_terms: Vec<String>,
    type_terms: Vec<String>,
}

impl HierarchyFilter {
    pub(crate) fn parse(raw: &str) -> Self {
        let mut filter = Self::default();
        for term in raw.split_whitespace() {
            let term = term.to_lowercase();
            if let Some(pattern) = term
                .strip_prefix("type:")
                .or_else(|| term.strip_prefix("component:"))
            {
                if !pattern.is_empty() {
                    filter.type_terms.push(pattern.to_owned());
                }
            } else {
                filter.name_terms.push(term);
            }
        }
        filter
    }

    /// 无有效词（空串与只有 `type:` 的输入都算"没有过滤"）。
    pub(crate) fn is_empty(&self) -> bool {
        self.name_terms.is_empty() && self.type_terms.is_empty()
    }

    /// 自身是否命中：名称词全部命中，且每个 `type:` 词都能在组件名里找到。
    pub(crate) fn matches_self(&self, lower_name: &str, lower_components: &[String]) -> bool {
        self.name_terms
            .iter()
            .all(|term| lower_name.contains(term.as_str()))
            && self.type_terms.iter().all(|pattern| {
                lower_components
                    .iter()
                    .any(|component| component.contains(pattern.as_str()))
            })
    }

    /// 只按名称词匹配（Scene View 的高亮过滤用它：类型词管 Hierarchy 的行可见性，
    /// 不该让 3D 视口里的对象凭空消失）。
    pub(crate) fn matches_name(&self, lower_name: &str) -> bool {
        self.name_terms
            .iter()
            .all(|term| lower_name.contains(term.as_str()))
    }
}

/// 小写化缓存（名称 + 组件名）：只在文档内容变化时重建。
///
/// 过滤匹配的真正热点不是"遍历"，而是每次按键都对 10k 个实体做 `to_lowercase()`
/// （Debug 下每次都是新分配）。名字与组件名只随文档修订变化，因此把"小写化"与"匹配"拆开：
/// 改过滤词只做 `contains`，不再重新小写化。
#[derive(Default)]
pub(crate) struct HierarchyLowerCache {
    valid: bool,
    revision: u64,
    entity_count: usize,
    names: std::collections::HashMap<SceneEntityId, String>,
    components: std::collections::HashMap<SceneEntityId, Vec<String>>,
}

impl HierarchyLowerCache {
    /// 取（必要时重建）当前文档的小写化名称/组件名表。
    pub(crate) fn ensure(&mut self, document: &editor::SceneDocument) -> &Self {
        let entity_count = document.scene.entities.len();
        if !self.valid || self.revision != document.revision || self.entity_count != entity_count {
            self.names.clear();
            self.names.extend(
                document
                    .scene
                    .entities
                    .values()
                    .map(|entity| (entity.id, entity.name.to_lowercase())),
            );
            self.components.clear();
            self.components
                .extend(document.scene.entities.values().map(|entity| {
                    (
                        entity.id,
                        entity
                            .components
                            .keys()
                            .map(|name| name.to_lowercase())
                            .collect::<Vec<_>>(),
                    )
                }));
            self.revision = document.revision;
            self.entity_count = entity_count;
            self.valid = true;
        }
        self
    }

    pub(crate) fn name(&self, id: SceneEntityId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// 该实体的小写化组件名（无记录时为空切片）。
    pub(crate) fn components(&self, id: SceneEntityId) -> &[String] {
        self.components.get(&id).map_or(&[], Vec::as_slice)
    }
}

/// 过滤匹配索引：一次后序遍历算出每个实体的 (自身命中, 子树命中)。
///
/// 旧实现 `hierarchy_filter_matches` 每行都要重新小写化自己的名字并递归整棵子树，
/// 10k 实体下一次过滤变化是 O(n·depth) 的字符串工作；索引把它压成 O(n) 遍历，
/// 并且复用 [`HierarchyLowerCache`] 的小写化结果，不改变任何可见性语义（单测锁定等价）。
#[derive(Debug, Default)]
pub(crate) struct HierarchyMatchIndex {
    /// 实体 ID 已派生 `Hash`，用哈希表以获得 O(1) 查询（10k 实体下与有序表差一个量级）。
    states: std::collections::HashMap<SceneEntityId, MatchState>,
}

impl HierarchyMatchIndex {
    /// 查询单个实体的匹配状态；未收录（理论上不会发生）视为不匹配。
    pub(crate) fn state(&self, id: SceneEntityId) -> MatchState {
        self.states.get(&id).copied().unwrap_or_default()
    }

    /// **自身命中**的实体数量（过滤框下的计数提示，无需再扫一遍）。
    pub(crate) fn match_count(&self) -> usize {
        self.states
            .values()
            .filter(|state| state.self_matches)
            .count()
    }

    /// 用预计算的小写化缓存构建索引（名称词与 `type:` 词都走 [`HierarchyFilter`]）。
    pub(crate) fn build(
        document: &editor::SceneDocument,
        filter: &HierarchyFilter,
        lower: &HierarchyLowerCache,
    ) -> Self {
        let mut states = std::collections::HashMap::with_capacity(document.scene.entities.len());
        if filter.is_empty() {
            for id in document.scene.entities.keys().copied() {
                states.insert(
                    id,
                    MatchState {
                        self_matches: true,
                        subtree_matches: true,
                    },
                );
            }
            return Self { states };
        }
        // 迭代式后序遍历：显式栈避免深树递归爆栈（10k 实体的链式场景过去踩过）。
        let roots: Vec<SceneEntityId> = document
            .scene
            .entities
            .values()
            .filter(|entity| entity.parent.is_none())
            .map(|entity| entity.id)
            .collect();
        let mut stack: Vec<(SceneEntityId, bool)> = roots.iter().map(|id| (*id, false)).collect();
        while let Some((id, expanded)) = stack.pop() {
            let Some(entity) = document.scene.entities.get(&id) else {
                continue;
            };
            if !expanded {
                stack.push((id, true));
                for child in entity.children.iter().rev() {
                    if !states.contains_key(child) {
                        stack.push((*child, false));
                    }
                }
                continue;
            }
            let self_matches = lower
                .name(id)
                .is_some_and(|name| filter.matches_self(name, lower.components(id)));
            let child_matches = entity
                .children
                .iter()
                .any(|child| states.get(child).is_some_and(|state| state.subtree_matches));
            states.insert(
                id,
                MatchState {
                    self_matches,
                    subtree_matches: self_matches || child_matches,
                },
            );
        }
        Self { states }
    }
}

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
    // `set_ui_root` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

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
                .get("Transform")
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

    // `paste_entity` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）：提交边界不在面板里。

    /// 层级面板：整合场景文件切换/新建，位于资源面板右侧。
    pub(crate) fn draw_hierarchy_panel(&mut self, context: &egui::Context) {
        egui::SidePanel::left("scene_tree")
            .resizable(true)
            // 宽度来自持久化布局（docs/55 P1：可保存布局）。
            .default_width(self.layout.panel_width(editor::PANEL_HIERARCHY))
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
                //
                // docs/55 阶段 0：把输入框矩形记进探针，GUI smoke 才能用**真实指针事件**把焦点
                // 打进文本框（"输入焦点"是计划 §8 质量门枚举的一项：键入时快捷键不得抢键）。
                // right-to-left：✕ 先画靠右，TextEdit 后画用 INFINITY 占满剩余宽度。
                // 之前按 available_width()-34 算 desired_width，输入文字比这个宽时 TextEdit
                // 的内容 galley 会反向撑大自身，导致输入框越输越宽（与 schema_inspector 同一问题）。
                let filter_response =
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        // 空时也在按钮位分配 0 宽占位：保持 TextEdit 的 widget 序号稳定，
                        // 否则输入首字符时 ✕ 按钮出现会让 TextEdit 序号 +1，egui 视为新控件而丢焦点。
                        if !self.hierarchy_filter.is_empty()
                            && ui.small_button("✕").on_hover_text("清空过滤").clicked()
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
                // 扁平化 + 虚拟滚动：只绘制视口内的行（10k 实体保持可交互）。
                // 行模型按 (修订, 实体数, 过滤词, 折叠集合) 缓存，绘制不再递归、不再克隆文档。
                let rows = self.hierarchy_rows();
                let row_count = rows.len();
                let body_height = ui.available_height().max(48.0);
                let row_height = hierarchy_row_height(ui);
                self.hierarchy_row_probe.reset();
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(body_height)
                    .show_rows(ui, row_height, row_count, |ui, visible| {
                        for index in visible {
                            if let Some(row) = rows.get(index).copied() {
                                self.draw_entity_row(ui, row, row_height);
                            }
                        }
                    });
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

    /// 过滤匹配（单实体语义入口）。
    ///
    /// 面板绘制与扁平化都走 [`HierarchyMatchIndex`]（一次遍历 + O(1) 查询），
    /// 此函数仅作为"自身或后代命中"语义的测试基准保留在测试构建中。
    #[cfg(test)]
    fn hierarchy_filter_matches(
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
    fn hierarchy_match_count(&mut self) -> usize {
        self.ensure_hierarchy_rows();
        self.hierarchy_visible_cache.match_count
    }

    /// 过滤匹配数（`pub(crate)` 版）：GUI smoke 要在**帧外**断言"输入真的缩窄了树"，
    /// 而上面那个私有版本需要在 `&mut self` 上跑一次扁平化——这正是我们要验证的东西。
    pub(crate) fn filtered_match_count(&mut self) -> usize {
        self.hierarchy_match_count()
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
    fn ensure_hierarchy_rows(&mut self) {
        let mut collapsed: Vec<_> = self.scene_state.collapsed.iter().copied().collect();
        collapsed.sort_unstable_by_key(|id| id.0);
        let filter = self.hierarchy_filter.trim().to_owned();
        let parsed_filter = HierarchyFilter::parse(&filter);
        let filter_active = !parsed_filter.is_empty();
        let mut out = Vec::new();
        let (revision, entity_count, match_count) = {
            let Some(document) = self.document.current_scene.as_ref() else {
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

    fn collect_visible_rows(
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

    // `commit_hierarchy_rename` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

    /// H02：行内重命名输入框。Enter 提交，Esc 取消，点击其他区域提交（Unity 习惯）。
    ///
    /// `row_height` 由调用方按当前样式传入：输入框必须落在行高内，
    /// 否则虚拟滚动的统一行高会被撑破（相邻行重叠 / 文字被裁）。
    fn draw_hierarchy_rename_edit(
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

    // `duplicate_selected_entity` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）。

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
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "New Entity".to_owned(),
                parent: Some(id),
                transform_3d: false,
            });
            ui.close();
        }
        if ui.button("新建同级节点").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "New Entity".to_owned(),
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
        if ui.button("创建 Prefab").clicked() {
            self.selection.set_single(id, SelectionSource::Hierarchy);
            self.create_prefab_from_selected();
            ui.close();
        }
        if ui.button("重新应用 Prefab").clicked() {
            self.selection.set_single(id, SelectionSource::Hierarchy);
            // typed action：目标显式携带（菜单不再依赖"先改选区"这个隐藏前提）。
            self.dispatch(editor::EditorCommand::ReapplyPrefab { entity: id });
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
    fn draw_hierarchy_create_menu(&mut self, ui: &mut egui::Ui, parent: Option<SceneEntityId>) {
        if self.current_scene().is_none() {
            ui.label("请先新建场景");
            return;
        }
        if ui.button("新建空实体").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "New Entity".to_owned(),
                parent,
                transform_3d: false,
            });
            ui.close();
        }
        if ui.button("新建 3D 实体").clicked() {
            self.dispatch(editor::EditorCommand::CreateEntity {
                name: "3D Entity".to_owned(),
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
    fn draw_entity_row(&mut self, ui: &mut egui::Ui, row: HierarchyRow, row_height: f32) {
        let HierarchyRow {
            id,
            depth,
            effective_collapsed,
            has_children,
        } = row;
        // 只提取本行需要的字段（旧实现整份文档 clone，10k 行即 10k 次全量克隆）。
        // 组件名也一起取：`type:` 过滤词要参与"这一行自己是否命中"的高亮判断。
        let (name, ui_root, is_3d, lower_components) = {
            let Some(entity) = self
                .document
                .current_scene
                .as_ref()
                .and_then(|scene| scene.scene.entities.get(&id))
            else {
                return;
            };
            (
                entity.name.clone(),
                entity
                    .metadata
                    .get("ui.root")
                    .is_some_and(|value| value == "true"),
                engine::core::spatial_transform::entity_space(&entity.components)
                    == engine::core::spatial_transform::SpatialKind::ThreeD,
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
            ui.dnd_drop_zone::<SceneEntityId, _>(egui::Frame::default(), |ui| {
                let inner = ui.horizontal(|ui| {
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
                        if has_children {
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
                        // 高度受限于行高：虚拟滚动要求行高统一，重命名不能把行撑高。
                        self.draw_hierarchy_rename_edit(ui, id, rename_row_height);
                    } else {
                        // H05：过滤命中高亮（名称词或 `type:` 词命中都算）。
                        let mut text =
                            egui::RichText::new(format!("{type_prefix}{name} [{}]", id.0));
                        if self_matches {
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

    // `persist_hierarchy_flag` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）：
    // 隐藏/锁定标记是场景 metadata，属于文档写入。

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
    /// 新建场景（菜单「文件 ▸ 新建场景」）。
    ///
    /// 菜单里**没有场景名输入框**（与 Unity 的 File ▸ New Scene 一致）：名字由这里决定，
    /// 并在冲突时自动去重（见 [`Self::next_new_scene_name`]），因此不会默默覆盖已有场景文件。
    pub(crate) fn create_scene(&mut self) {
        if !self.project_loaded {
            self.status = "Import a game project before editing".to_owned();
            return;
        }
        if self.is_playing() {
            self.status = "Play mode locks scene editing; click Stop first".to_owned();
            return;
        }
        let name = self.next_new_scene_name();
        self.request_document_change(crate::actions::PendingChange::CreateScene { name });
    }

    /// 新建场景的默认名：`main`；若 `scenes/main.scene` 已存在（或当前文档已叫这个名字）
    /// 则依次尝试 `main2`、`main3`…，避免"新建两次都在写同一个文件"。
    pub(crate) fn next_new_scene_name(&self) -> String {
        let scenes = strip_verbatim_prefix(&self.project.root).join("scenes");
        let current = self
            .current_scene()
            .map(|document| document.scene.name.clone());
        let taken = |candidate: &str| {
            current.as_deref() == Some(candidate)
                || scenes.join(format!("{candidate}.scene")).exists()
        };
        if !taken("main") {
            return "main".to_owned();
        }
        for index in 2..1000 {
            let candidate = format!("main{index}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        // 极端情况下（1000 个同名场景）退回时间戳，保证一定拿到一个可用名字。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        format!("main{stamp}")
    }

    // `perform_create_scene` / `perform_create_widget` / `add_3d_entity` **已搬到 shell**
    // （`crate::hierarchy_ops`，第 28 轮）：它们都会替换/写入当前文档。

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
        // **嵌套 Prefab 尚不支持**（第 47 轮）：从"已经是 Prefab 实例"的实体（或包含实例的
        // 子树）创建 Prefab 会写出一份**嵌套源**——根/后代带着 `prefab.source`，此后
        // `reapply_prefab` / 写回源都会把它当成另一个 Prefab 的实例来重建。
        // 与其悄悄生成这种数据，不如拒绝并让作者先 Unpack（编辑器里有这个动作）。
        if let Some(nested) = subtree_prefab_source(&scene.scene, selected) {
            self.status = format!(
                "所选实体里包含 Prefab 实例（{nested}）：嵌套 Prefab 尚不支持，请先 Unpack 再创建"
            );
            return;
        }
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

/// 子树里出现的第一个 `prefab.source`（含根自己）：非 `None` 说明这份子树里已经有
/// Prefab 实例，从它创建 Prefab 会产生**嵌套**（编辑器目前不支持）。
fn subtree_prefab_source(
    scene: &engine::core::scene::Scene,
    root: SceneEntityId,
) -> Option<String> {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(entity) = scene.entities.get(&id) else {
            continue;
        };
        if let Some(source) = entity
            .metadata
            .get("prefab.source")
            .filter(|source| !source.trim().is_empty())
        {
            return Some(source.clone());
        }
        stack.extend(entity.children.iter().copied());
    }
    None
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
    use super::{
        HIERARCHY_ROW_HEIGHT, HierarchyFilter, HierarchyMatchIndex, RowHeightProbe,
        hierarchy_row_height,
    };
    use editor::SceneDocument;
    use eframe::egui;
    use engine::core::{logging::LogSink, resource_db::AssetId, scene::SceneEntityId};

    /// 新建场景的默认名：`main` 可用就用它；`scenes/main.scene` 已存在则 `main2`…
    /// 菜单里没有名字输入框，名字必须由这里保证"不覆盖已有场景"。
    /// **嵌套 Prefab 的创建边界**（第 47 轮）：从 Prefab 实例（或含实例的子树）创建 Prefab
    /// 必须被拒绝——那会写出一份嵌套源，之后的 reapply/写回都会把它当成两份实例；
    /// 平实体照常创建成功。
    #[test]
    fn creating_a_prefab_from_a_prefab_instance_is_refused() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-create-prefab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(root.join("scripts/main.lua"), b"return true\n").unwrap();
        let project = editor::open_project(&root).expect("project opens");
        let mut app = crate::EditorApp::new(project, None, LogSink::new(64));
        let scene = app.document.new_scene("main");
        let instance_root = app.tree.create_entity(scene, "Hero", None).unwrap();
        let child = app
            .tree
            .create_entity(scene, "Child", Some(instance_root))
            .unwrap();
        // 子树里带 `prefab.source` = 这一支是 Prefab 实例（含后代的情况）。
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .entities
            .get_mut(&child)
            .unwrap()
            .metadata
            .insert(
                "prefab.source".to_owned(),
                "assets/prefabs/hero.prefab".to_owned(),
            );
        app.selection
            .set_single(instance_root, editor::SelectionSource::Hierarchy);

        app.create_prefab_from_selected();
        assert!(
            app.status.contains("嵌套 Prefab 尚不支持"),
            "必须明确拒绝并说明原因：{}",
            app.status
        );
        assert!(
            !root.join("assets/prefabs/Hero.prefab").exists(),
            "被拒绝时不得写出嵌套源"
        );

        // 平实体（去掉实例标记）照常创建。
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .entities
            .get_mut(&child)
            .unwrap()
            .metadata
            .remove("prefab.source");
        app.create_prefab_from_selected();
        // 注：创建成功会触发后台资源刷新，`status` 会被刷新的提示覆盖，所以这里断言产物。
        assert!(
            root.join("assets/prefabs/Hero.prefab").exists(),
            "平实体照常创建（status={}）",
            app.status
        );
        let prefab =
            engine::core::prefab::Prefab::load(root.join("assets/prefabs/Hero.prefab")).unwrap();
        assert!(
            prefab.nested_sources().is_empty(),
            "平 Prefab 不该带任何嵌套引用"
        );
        assert!(!prefab.entities.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// 扫出"这份 Prefab 里嵌了谁"（引擎侧事实源 `Prefab::nested_sources`）。
    #[test]
    fn nested_sources_report_every_prefab_reference_in_the_file() {
        let mut scene = engine::core::scene::Scene::new("hero");
        let root = SceneEntityId(1);
        scene.add_entity(root, "Hero", None).unwrap();
        let child = SceneEntityId(2);
        scene.add_entity(child, "Weapon", Some(root)).unwrap();
        scene.entities.get_mut(&child).unwrap().metadata.insert(
            "prefab.source".to_owned(),
            "assets/prefabs/sword.prefab".to_owned(),
        );
        let prefab = engine::core::prefab::Prefab::from_scene(
            &scene,
            root,
            AssetId::from_seed("hero"),
            "hero",
        )
        .unwrap();
        assert_eq!(
            prefab.nested_sources(),
            vec!["assets/prefabs/sword.prefab".to_owned()]
        );
    }

    #[test]
    fn new_scene_name_avoids_overwriting_existing_scenes() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-new-scene-name-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        let mut app = crate::EditorApp::new(
            editor::ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        // 空项目：直接叫 main。
        assert_eq!(app.next_new_scene_name(), "main");

        // 磁盘上已有 main.scene → 退到 main2；再占掉 main2 → main3。
        std::fs::write(root.join("scenes/main.scene"), "placeholder").unwrap();
        assert_eq!(app.next_new_scene_name(), "main2");
        std::fs::write(root.join("scenes/main2.scene"), "placeholder").unwrap();
        assert_eq!(app.next_new_scene_name(), "main3");

        // 当前文档即使还没落盘也要占位，避免"新建两次同名"。
        app.document.current_scene = Some(SceneDocument::new_scene("main3"));
        assert_eq!(app.next_new_scene_name(), "main4");

        let _ = std::fs::remove_dir_all(&root);
    }

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
                    .join(format!("rustlua-hierarchy-filter-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.document.current_scene = Some(document);
        app.hierarchy_filter = "Leaf".to_owned();

        let scene = app.current_scene().cloned().expect("scene present");
        let root_visible = app.hierarchy_filter_matches(&scene, root);
        let branch_visible = app.hierarchy_filter_matches(&scene, branch);
        let leaf_visible = app.hierarchy_filter_matches(&scene, leaf);

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
                    .join(format!("rustlua-hierarchy-opts-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.document.current_scene = Some(document);
        app
    }

    /// ER-04 的"类型过滤"（第 41 轮）：`type:` / `component:` 按**组件名**过滤（子串、
    /// 大小写不敏感），与名称词是 AND 关系；`type:` 只输入到一半不算过滤。
    #[test]
    fn type_filter_matches_components_and_combines_with_name_terms() {
        let mut document = sample_document();
        {
            let entities = &mut document.scene.entities;
            entities
                .get_mut(&SceneEntityId(2))
                .unwrap()
                .components
                .insert("Sprite".to_owned(), "texture=;size=1x1".to_owned());
            entities
                .get_mut(&SceneEntityId(3))
                .unwrap()
                .components
                .insert(
                    "Light".to_owned(),
                    "direction=0,1,0;color=1,1,1;intensity=1".to_owned(),
                );
            let solo = entities.get_mut(&SceneEntityId(4)).unwrap();
            solo.components
                .insert("UiLayout".to_owned(), "pivot=0,0".to_owned());
            solo.components
                .insert("UiEvents".to_owned(), "event=click;function=f".to_owned());
        }
        let mut app = make_app(document);

        // 组件名命中 → 该行与它的祖先路径可见（祖先自身不算命中）。
        app.hierarchy_filter = "type:sprite".to_owned();
        assert_eq!(
            app.hierarchy_visible_ids(),
            vec![SceneEntityId(1), SceneEntityId(2)]
        );
        assert_eq!(app.hierarchy_match_count(), 1, "计数只算**自身**命中的实体");

        // 子串匹配一次命中多个 UI 组件（`UiLayout`/`UiEvents` 都算）——这是最有用的用法。
        app.hierarchy_filter = "type:ui".to_owned();
        assert_eq!(app.hierarchy_visible_ids(), vec![SceneEntityId(4)]);
        assert_eq!(app.hierarchy_match_count(), 1);

        // 大小写不敏感；`component:` 与 `type:` 同义。
        app.hierarchy_filter = "component:SPRITE".to_owned();
        assert_eq!(app.hierarchy_match_count(), 1);

        // 名称词与类型词是 AND：`A` 有 Sprite → 命中；`Leaf` 只有 Light → 不命中。
        app.hierarchy_filter = "a type:sprite".to_owned();
        assert_eq!(app.hierarchy_match_count(), 1);
        app.hierarchy_filter = "Leaf type:sprite".to_owned();
        assert_eq!(app.hierarchy_match_count(), 0);
        assert!(
            app.hierarchy_visible_ids().is_empty(),
            "没有任何实体命中时不应留下祖先行"
        );

        // 只输入到一半的 `type:` → 视为没有过滤（否则每敲一个前缀就清空整棵树）。
        app.hierarchy_filter = "type:".to_owned();
        assert_eq!(app.hierarchy_visible_ids().len(), 4);
        assert_eq!(app.hierarchy_match_count(), 4);
        // 类型词不认识任何组件 → 空结果（不是"没有过滤"）。
        app.hierarchy_filter = "type:nope".to_owned();
        assert!(app.hierarchy_visible_ids().is_empty());
        assert_eq!(app.hierarchy_match_count(), 0);
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
    fn visible_cache_invalidates_on_structure_and_collapse_changes() {
        let mut app = make_app(sample_document());
        let first = app.hierarchy_visible_ids();
        assert_eq!(
            first,
            vec![
                SceneEntityId(1),
                SceneEntityId(2),
                SceneEntityId(3),
                SceneEntityId(4)
            ]
        );
        // Repeat call is a cache hit with identical content.
        assert_eq!(app.hierarchy_visible_ids(), first);
        // Structural addition without an explicit revision bump must still invalidate.
        app.document
            .current_scene
            .as_mut()
            .unwrap()
            .scene
            .add_entity(SceneEntityId(5), "Added", None)
            .unwrap();
        assert!(app.hierarchy_visible_ids().contains(&SceneEntityId(5)));
        // Collapse state is part of the key.
        app.scene_state.collapsed.insert(SceneEntityId(1));
        assert_eq!(
            app.hierarchy_visible_ids(),
            vec![SceneEntityId(1), SceneEntityId(4), SceneEntityId(5)]
        );
    }

    #[test]
    fn hierarchy_rows_carry_depth_and_effective_collapse() {
        // 行模型是虚拟滚动的唯一数据源：这里锁定深度与折叠语义，防止"少画/错缩进"退化。
        let mut app = make_app(sample_document());
        let rows = app.hierarchy_rows();
        assert_eq!(
            rows.iter()
                .map(|row| (row.id, row.depth))
                .collect::<Vec<_>>(),
            vec![
                (SceneEntityId(1), 0),
                (SceneEntityId(2), 1),
                (SceneEntityId(3), 1),
                (SceneEntityId(4), 0),
            ]
        );
        assert!(rows[0].has_children, "Root 有子节点");
        assert!(!rows[1].has_children, "A 是叶子");

        // 折叠 Root：子节点不再进入行模型，且 Root 行标记为折叠。
        app.scene_state.collapsed.insert(SceneEntityId(1));
        let rows = app.hierarchy_rows();
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![SceneEntityId(1), SceneEntityId(4)]
        );
        assert!(rows[0].effective_collapsed);

        // 过滤命中子节点时强制展开：折叠状态被 effective_collapsed=false 覆盖。
        app.hierarchy_filter = "Leaf".to_owned();
        let rows = app.hierarchy_rows();
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![SceneEntityId(1), SceneEntityId(3)]
        );
        assert!(!rows[0].effective_collapsed, "过滤强制展开时箭头显示为展开");
        assert_eq!(rows[1].depth, 1);
    }

    /// 索引化过滤必须与"逐行递归"参考实现逐实体等价（防止优化改变可见性语义）。
    #[test]
    fn match_index_is_equivalent_to_recursive_reference() {
        // 构造一个更深、更宽的树：4 个根，每根 3 层，名称里混入大小写与部分匹配词。
        let mut document = SceneDocument::new_scene("equivalent");
        let mut next = 1_u64;
        let mut ids = Vec::new();
        for root in 0..4_u64 {
            let root_id = SceneEntityId(next);
            next += 1;
            document
                .scene
                .add_entity(root_id, format!("Group{root}"), None)
                .unwrap();
            ids.push(root_id);
            let mut parents = vec![root_id];
            for depth in 0..3_u64 {
                let mut children = Vec::new();
                for slot in 0..3_u64 {
                    let id = SceneEntityId(next);
                    next += 1;
                    let name = match (depth + slot) % 3 {
                        0 => format!("Leaf_{depth}_{slot}"),
                        1 => format!("Branch_{depth}_{slot}"),
                        _ => format!("other_{depth}_{slot}"),
                    };
                    document
                        .scene
                        .add_entity(id, name, Some(parents[0]))
                        .unwrap();
                    // 混入组件名，让 `type:` 词也在这条等价性门里被覆盖（每个实体两种组件）。
                    let entity = document.scene.entities.get_mut(&id).unwrap();
                    entity.components.insert(
                        if slot % 2 == 0 { "Sprite" } else { "Light" }.to_owned(),
                        "x=1".to_owned(),
                    );
                    entity.components.insert(
                        if depth % 2 == 0 {
                            "UiLayout"
                        } else {
                            "Collider"
                        }
                        .to_owned(),
                        "x=1".to_owned(),
                    );
                    children.push(id);
                    ids.push(id);
                }
                parents = children;
            }
        }
        let mut app = make_app(document);

        for query in [
            "leaf",
            "LEAF_1",
            "branch",
            "Group2",
            "missing",
            "other_2_0",
            // 类型词、类型词 + 名称词、大小写、以及"刚输入到一半的 type:"。
            "type:sprite",
            "type:ui",
            "component:transform3d",
            "leaf type:sprite",
            "branch component:UiLayout",
            "type:",
        ] {
            app.hierarchy_filter = query.to_owned();
            let scene = app.current_scene().cloned().expect("scene");
            let filter = HierarchyFilter::parse(query);
            let index = {
                let lower = app.hierarchy_lower.ensure(&scene);
                HierarchyMatchIndex::build(&scene, &filter, lower)
            };
            for id in &ids {
                let reference = app.hierarchy_filter_matches(&scene, *id);
                assert_eq!(
                    index.state(*id).subtree_matches,
                    reference,
                    "query={query:?} entity={id:?} 索引与递归参考实现必须一致"
                );
            }
        }
    }

    /// H05：过滤命中计数取扁平化时一次算出的结果（不再每帧全量扫描）。
    #[test]
    fn match_count_matches_manual_scan() {
        let mut app = make_app(sample_document());
        app.hierarchy_filter = "Leaf".to_owned();
        let _ = app.hierarchy_rows();
        assert_eq!(app.hierarchy_match_count(), 1);
        app.hierarchy_filter = "o".to_owned();
        // Root(1)/Solo(4) 的名称含 'o'（大小写不敏感），A(2)/Leaf(3) 不含。
        assert_eq!(app.hierarchy_match_count(), 2);
    }

    /// 布局探针语义：取最大值、可重置、能计数（门槛判定依赖这三点）。
    #[test]
    fn row_height_probe_tracks_max_content_and_resets() {
        let mut probe = RowHeightProbe::default();
        assert_eq!(probe.rows(), 0);
        assert_eq!(probe.max_content_height(), 0.0);
        probe.observe(18.0);
        probe.observe(21.5);
        probe.observe(19.0);
        assert_eq!(probe.rows(), 3);
        assert_eq!(probe.max_content_height(), 21.5);
        probe.reset();
        assert_eq!(probe.rows(), 0);
        assert_eq!(probe.max_content_height(), 0.0);
    }

    /// 行高必须由样式推导，且不小于基础值（DPI/缩放下不塌陷）。
    #[test]
    fn hierarchy_row_height_follows_style_and_never_collapses() {
        let context = egui::Context::default();
        let raw = egui::RawInput::default();
        let mut height = 0.0_f32;
        let _ = context.run(raw, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                height = hierarchy_row_height(ui);
            });
        });
        assert!(
            height >= HIERARCHY_ROW_HEIGHT * 0.5,
            "行高不得塌陷：{height}"
        );
        assert!(
            height >= 14.0,
            "行高必须容纳 18×18 行首控件与文本：{height}"
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
        let root = std::env::temp_dir().join(format!("rustlua-hierarchy-persist-{token}"));
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
