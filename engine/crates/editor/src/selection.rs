//! E03：编辑器统一选择模型。
//!
//! 这是 Hierarchy / Scene View / Inspector / 命令层（Undo/Redo、删除、Prefab、Play）之间
//! 共享的**单一权威选择源**，取代原先分散在 `SceneTreeState`、`InspectorState` 与
//! `SceneEditorState::selected` 上的三套互相独立、易失同步的选择状态。
//!
//! 不变量：
//! 1. `primary` 要么为 `None`，要么必须同时出现在 `multi` 集合中。
//! 2. `multi` 为空当且仅当 `primary` 为 `None`（即“没有任何选中对象”）。
//! 3. 集合中不允许残留指向已不存在实体的悬空 ID（由 `retain_valid` /
//!    `on_entities_removed` 保证清理）。
//!
//! 任意入口修改选区后，三个面板在同一帧内通过
//! [`SelectionModel::contains`] / [`SelectionModel::iter`] / [`SelectionModel::primary`]
//! 读到同一份数据。

use engine::core::scene::SceneEntityId;
use std::collections::BTreeSet;

/// 产生本次选择变化的入口，用于诊断与 Delete 快捷键来源判定。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectionSource {
    /// 无来源（默认/清理路径）。
    #[default]
    None,
    /// Hierarchy 面板点击/创建后自动选中。
    Hierarchy,
    /// 2D Scene View 点选/框选/拖拽。
    SceneView2D,
    /// 3D Scene View 点选/Gizmo。
    SceneView3D,
    /// Inspector 内操作（如 Prefab Apply/Reapply 后定位到新根）。
    Inspector,
    /// 命令层（创建/删除/Prefab 实例化等业务命令）。
    Command,
    /// Undo/Redo 后的选择修复。
    UndoRedo,
    /// Play/Stop 切换后的选择修复。
    Playback,
}

/// 实体选择的统一状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionModel {
    /// 主选中对象（Inspector 内容 / Prefab 创建 / Gizmo 主轴 / 删除目标的来源）。
    primary: Option<SceneEntityId>,
    /// 全部选中对象的集合（含主选中；Scene View 框选与拖拽批量操作的来源）。
    multi: BTreeSet<SceneEntityId>,
    /// 最后一次写入来源。
    source: SelectionSource,
}

impl SelectionModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// 主选中对象。Inspector 同步、Prefab 导出、单实体命令均以它为准。
    pub fn primary(&self) -> Option<SceneEntityId> {
        self.primary
    }

    /// 最近一次改变选区的来源。
    pub fn source(&self) -> SelectionSource {
        self.source
    }

    /// 是否没有任何选中对象。
    pub fn is_empty(&self) -> bool {
        self.multi.is_empty()
    }

    /// 当前多选集合大小（含主选中；为 0 等价于空选区）。
    pub fn len(&self) -> usize {
        self.multi.len()
    }

    /// 实体是否处于选中状态（Hierarchy 高亮 / Scene View 选择框统一判断入口）。
    pub fn contains(&self, id: SceneEntityId) -> bool {
        self.multi.contains(&id)
    }

    /// 遍历当前全部选中对象（任意顺序，升序）。
    pub fn iter(&self) -> impl Iterator<Item = SceneEntityId> + '_ {
        self.multi.iter().copied()
    }

    /// 收集为 `Vec`，便于传给批量操作与闭包。
    pub fn ids(&self) -> Vec<SceneEntityId> {
        self.multi.iter().copied().collect()
    }

    /// 单选：清空原集合并把 `id` 设为主选中（Hierarchy/Scene View 非加选点击）。
    pub fn set_single(&mut self, id: SceneEntityId, source: SelectionSource) {
        self.multi.clear();
        self.multi.insert(id);
        self.primary = Some(id);
        self.source = source;
    }

    /// 批量替换选区（框选、多选恢复）。若 `preferred_primary` 仍在集合中则保持为主选中，
    /// 否则回落到集合中最大的 ID；集合为空时主选中清空。
    pub fn set_many(
        &mut self,
        ids: impl IntoIterator<Item = SceneEntityId>,
        preferred_primary: Option<SceneEntityId>,
        source: SelectionSource,
    ) {
        self.multi = ids.into_iter().collect();
        self.primary = preferred_primary
            .filter(|id| self.multi.contains(id))
            .or_else(|| self.multi.last().copied());
        self.source = source;
    }

    /// 清空全部选择（点空白处 / 文档整表切换）。
    pub fn clear(&mut self, source: SelectionSource) {
        self.multi.clear();
        self.primary = None;
        self.source = source;
    }

    /// 把 `id` 无条件加入选区并设为主选中（创建实体后自动选中、拖拽起始）。
    pub fn add(&mut self, id: SceneEntityId, source: SelectionSource) {
        self.multi.insert(id);
        self.primary = Some(id);
        self.source = source;
    }

    /// Ctrl/Command 加选点击：若已在集合中则移出（并回填主选中），否则加入并成为主选中。
    pub fn toggle(&mut self, id: SceneEntityId, source: SelectionSource) {
        if self.multi.remove(&id) {
            if self.primary == Some(id) {
                self.primary = self.multi.last().copied();
            }
        } else {
            self.multi.insert(id);
            self.primary = Some(id);
        }
        self.source = source;
    }

    /// 仅把主选中改成集合中已有的 `id`（不动集合本身）。若 `id` 不在集合内则忽略。
    pub fn set_primary_if_selected(&mut self, id: SceneEntityId) {
        if self.multi.contains(&id) {
            self.primary = Some(id);
        }
    }

    /// 从集合移除 `id`，不改变来源（用于锁定/隐藏等不改变意图的过滤路径）。
    pub fn remove(&mut self, id: SceneEntityId) {
        if self.multi.remove(&id) && self.primary == Some(id) {
            self.primary = self.multi.last().copied();
        }
    }

    /// 清理指向不存在实体的悬空引用（Undo/Redo/删除/Prefab/Play-Stop 后调用）。
    /// 主选中若悬空则回落到集合中剩余的 ID；集合清空后主选中一并清空。
    pub fn retain_valid(&mut self, valid: impl Fn(SceneEntityId) -> bool) {
        let mut retained = BTreeSet::new();
        for id in self.multi.iter().copied() {
            if valid(id) {
                retained.insert(id);
            }
        }
        self.multi = retained;
        if self.primary.is_some_and(|id| !self.multi.contains(&id)) {
            self.primary = self.multi.last().copied();
        }
        debug_assert_eq!(self.multi.is_empty(), self.primary.is_none());
    }

    /// 实体被删除（含整棵子树）后的引用清理。`removed` 是本次消失的全部实体 ID，
    /// `parent_fallback` 是主选中实体删除前的父节点（仍存在时应回退到它，符合场景树习惯）。
    pub fn on_entities_removed(
        &mut self,
        removed: &BTreeSet<SceneEntityId>,
        parent_fallback: Option<SceneEntityId>,
    ) {
        for id in removed {
            self.multi.remove(id);
        }
        if self.primary.is_some_and(|id| removed.contains(&id)) {
            self.primary = parent_fallback
                .filter(|parent| !removed.contains(parent) && self.multi.contains(parent))
                .or_else(|| self.multi.last().copied());
        }
        if self.multi.is_empty() {
            self.primary = None;
        }
    }

    /// 保持主选中为 `id`（若仍被选中）并把来源记为 `source`，用于同步 Inspector 后的来源标注。
    pub fn note_source(&mut self, source: SelectionSource) {
        self.source = source;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> SceneEntityId {
        SceneEntityId(value)
    }

    #[test]
    fn defaults_to_empty_selection() {
        let selection = SelectionModel::new();
        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert_eq!(selection.primary(), None);
        assert_eq!(selection.source(), SelectionSource::None);
    }

    #[test]
    fn set_single_replaces_previous_multi_selection() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2), id(3)],
            Some(id(1)),
            SelectionSource::SceneView2D,
        );
        assert_eq!(selection.len(), 3);
        selection.set_single(id(5), SelectionSource::Hierarchy);
        assert_eq!(selection.ids(), vec![id(5)]);
        assert_eq!(selection.primary(), Some(id(5)));
        assert_eq!(selection.source(), SelectionSource::Hierarchy);
        assert!(selection.contains(id(5)));
        assert!(!selection.contains(id(1)));
    }

    #[test]
    fn set_many_prefers_primary_and_falls_back_to_last() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(4), id(2), id(9)],
            Some(id(2)),
            SelectionSource::SceneView3D,
        );
        assert_eq!(selection.primary(), Some(id(2)));
        // preferred_primary 不在集合中时回落到最大 ID。
        selection.set_many(
            [id(7), id(3)],
            Some(id(99)),
            SelectionSource::SceneView2D,
        );
        assert_eq!(selection.primary(), Some(id(7)));
        // 空集合 → 主选中清空。
        selection.set_many([], Some(id(3)), SelectionSource::SceneView2D);
        assert!(selection.is_empty());
        assert_eq!(selection.primary(), None);
    }

    #[test]
    fn toggle_inserts_then_removes_and_refills_primary() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2)],
            Some(id(1)),
            SelectionSource::SceneView2D,
        );
        // 加选新实体 → 成为主选中。
        selection.toggle(id(3), SelectionSource::SceneView2D);
        assert!(selection.contains(id(3)));
        assert_eq!(selection.primary(), Some(id(3)));
        // 减选主选中 → 主选中回落到剩余最大 ID。
        selection.toggle(id(3), SelectionSource::SceneView2D);
        assert!(!selection.contains(id(3)));
        assert_eq!(selection.primary(), Some(id(2)));
        // 逐个减到空 → 主选中清空。
        selection.toggle(id(2), SelectionSource::SceneView2D);
        selection.toggle(id(1), SelectionSource::SceneView2D);
        assert!(selection.is_empty());
        assert_eq!(selection.primary(), None);
    }

    #[test]
    fn retain_valid_drops_dangling_and_refills_primary() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2), id(3)],
            Some(id(1)),
            SelectionSource::Command,
        );
        selection.retain_valid(|candidate| candidate != id(1) && candidate != id(2));
        assert_eq!(selection.ids(), vec![id(3)]);
        assert_eq!(selection.primary(), Some(id(3)));
        selection.retain_valid(|_| false);
        assert!(selection.is_empty());
        assert_eq!(selection.primary(), None);
    }

    #[test]
    fn on_entities_removed_falls_back_to_parent_then_to_remaining() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2), id(3)],
            Some(id(1)),
            SelectionSource::Command,
        );
        let removed = BTreeSet::from([id(1)]);
        // 主选中被删除 → 回退父 2（若仍在集合）。
        selection.on_entities_removed(&removed, Some(id(2)));
        assert_eq!(selection.primary(), Some(id(2)));
        assert_eq!(selection.ids(), vec![id(2), id(3)]);
        // 父也被删、只剩一个 → 回落到剩余唯一实体。
        let removed = BTreeSet::from([id(2)]);
        selection.on_entities_removed(&removed, Some(id(1)));
        assert_eq!(selection.primary(), Some(id(3)));
        // 全部删除 → 空选区。
        let removed = BTreeSet::from([id(3)]);
        selection.on_entities_removed(&removed, None);
        assert!(selection.is_empty());
    }

    #[test]
    fn remove_keeps_primary_invariant() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2), id(3)],
            Some(id(1)),
            SelectionSource::Inspector,
        );
        selection.remove(id(1));
        assert!(!selection.contains(id(1)));
        assert_eq!(selection.primary(), Some(id(3)));
        selection.remove(id(3));
        selection.remove(id(2));
        assert!(selection.is_empty());
        assert_eq!(selection.primary(), None);
    }

    #[test]
    fn add_sets_primary_and_source() {
        let mut selection = SelectionModel::new();
        selection.add(id(8), SelectionSource::Playback);
        assert_eq!(selection.primary(), Some(id(8)));
        assert_eq!(selection.ids(), vec![id(8)]);
        assert_eq!(selection.source(), SelectionSource::Playback);
    }

    #[test]
    fn clear_resets_everything() {
        let mut selection = SelectionModel::new();
        selection.set_many(
            [id(1), id(2)],
            Some(id(1)),
            SelectionSource::SceneView2D,
        );
        selection.clear(SelectionSource::Playback);
        assert!(selection.is_empty());
        assert_eq!(selection.primary(), None);
        assert_eq!(selection.source(), SelectionSource::Playback);
    }
}
