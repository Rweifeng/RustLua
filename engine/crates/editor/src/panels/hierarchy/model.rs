//! 层级面板的行模型：行/过滤/小写缓存/匹配索引/行高（M79 结构切分，零行为变化）。
use eframe::egui;
use rustlua_scene::scene::SceneEntityId;

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
    pub(super) valid: bool,
    pub(super) revision: u64,
    /// Entity count guards against structural changes that did not bump the revision
    /// (for example direct scene mutation from tooling or tests).
    pub(super) entity_count: usize,
    pub(super) filter: String,
    pub(super) collapsed: Vec<SceneEntityId>,
    /// 过滤词**自身命中**的实体数（过滤框计数提示，随扁平化一次算出）。
    pub(super) match_count: usize,
    /// 上一次扁平化用的解析结果：绘制行时复用，避免每行重新解析过滤串。
    pub(super) parsed_filter: HierarchyFilter,
    /// 行模型用 `Arc` 共享：绘制期间持有它不借用 `self`，重入查询（Shift 范围选等）
    /// 也能拿到同一份数据，且每次取用只是引用计数 +1。
    pub(super) rows: std::sync::Arc<Vec<HierarchyRow>>,
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
pub(super) enum HierarchyNav {
    Up,
    Down,
    Left,
    Right,
    Rename,
    Focus,
}
