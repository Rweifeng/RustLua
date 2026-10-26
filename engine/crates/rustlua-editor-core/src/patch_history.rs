//! docs/55 ER-03「patch 原型」：把一次编辑表示成**补丁**，而不是整份文档快照。
//!
//! 计划 §1 把"Undo 主要保存完整文档快照，大场景和长编辑会话的内存成本不可控"列为当前主要矛盾之一；
//! 阶段 1 的"历史内存预算"（`CommandService` 默认 200 条、超出丢最旧并计数）只做了**止血**：
//! 内存上界有了，但每条历史的成本仍然是 O(文档)，于是一次编辑在 1k 实体场景上就要几百 KiB。
//!
//! 这个模块最初是 ER-03 要求的**原型**：证明"把编辑表达成补丁"在编辑器真正要保证的那条
//! 不变量（**Undo/Redo 等价**，docs/55 §8 硬门槛里"Undo 不等价"直接阻断）上站得住，
//! 并给出可测量的字节数差异。
//!
//! **M79 P1-2b 起它上了生产**：[`crate::CommandHistory`] 里的字段级编辑（组件载荷、实体名）
//! 不再保存两份整文档快照，而是保存这里的一条 `EditPatch` + 一份文档级锚点；结构变更
//! （新建/删除/换父/场景级字段）压不动，仍旧走快照。两种形态共存于**同一条栈**，
//! 因为跨面板全局 Undo 只认一条栈的顺序（见 editor bin 的 `actions/undo_ledger.rs`），
//! 分成两条栈就会在"撤销哪一条"上判错。判据与代价见 `docs/25` §24。
//!
//! 范围（刻意划清，避免把原型说成成品）：
//!
//! * 覆盖**字段级编辑**——`Component` 载荷与实体名。这正是长编辑会话里占比最高的操作
//!   （Inspector 改字段、批量改属性、重命名）；
//! * **不覆盖**结构变更（新建/删除/换父）。那些改动天然需要"被删掉的那棵子树"这类信息，
//!   把它做对需要单独的补丁形态，属于后续轮次；原型不假装支持。
//! * 与快照历史同形的语义：空操作不入栈、新的编辑清空 redo、相邻同目标变换可合并、
//!   预算上限丢最旧并计数。

use rustlua_ecs::transform::Transform3D;
use rustlua_scene::scene::{Scene, SceneEntityId};

use crate::CommandKind;

/// 一条字段级变更（`before` = `None` 表示"这个组件原本不存在"）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldChange {
    /// 写入组件载荷（`None` = 移除组件）。
    Component {
        entity: SceneEntityId,
        component: String,
        before: Option<String>,
        after: Option<String>,
    },
    /// 实体重命名。
    Name {
        entity: SceneEntityId,
        before: String,
        after: String,
    },
}

impl FieldChange {
    pub fn entity(&self) -> SceneEntityId {
        match self {
            Self::Component { entity, .. } | Self::Name { entity, .. } => *entity,
        }
    }

    /// 这条变更是否会改变文档。
    pub fn is_effective(&self) -> bool {
        match self {
            Self::Component { before, after, .. } => before != after,
            Self::Name { before, after, .. } => before != after,
        }
    }

    /// 保留这条变更所需的字节数（字符串内容 = 补丁的全部成本）。
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Component {
                component,
                before,
                after,
                ..
            } => {
                component.len()
                    + before.as_ref().map_or(0, String::len)
                    + after.as_ref().map_or(0, String::len)
                    + std::mem::size_of::<Self>()
            }
            Self::Name { before, after, .. } => {
                before.len() + after.len() + std::mem::size_of::<Self>()
            }
        }
    }

    fn apply(&self, scene: &mut Scene, forward: bool) {
        let Some(entity) = scene.entities.get_mut(&self.entity()) else {
            return;
        };
        match self {
            Self::Component {
                component,
                before,
                after,
                ..
            } => {
                let value = if forward { after } else { before };
                match value {
                    Some(value) => {
                        entity.components.insert(component.clone(), value.clone());
                    }
                    None => {
                        entity.components.remove(component);
                    }
                }
            }
            Self::Name { before, after, .. } => {
                entity.name = if forward {
                    after.clone()
                } else {
                    before.clone()
                };
            }
        }
    }
}

/// 一次编辑 = 若干条字段级变更。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditPatch {
    pub label: String,
    pub kind: CommandKind,
    changes: Vec<FieldChange>,
}

impl EditPatch {
    pub fn new(label: impl Into<String>, kind: CommandKind, changes: Vec<FieldChange>) -> Self {
        Self {
            label: label.into(),
            kind,
            changes,
        }
    }

    /// 单字段组件写入（`before = None` 表示新增该组件）。
    pub fn set_component(
        entity: SceneEntityId,
        component: impl Into<String>,
        before: Option<String>,
        after: impl Into<String>,
    ) -> Self {
        Self::new(
            "设置组件",
            CommandKind::Property,
            vec![FieldChange::Component {
                entity,
                component: component.into(),
                before,
                after: Some(after.into()),
            }],
        )
    }

    /// 实体重命名。
    pub fn rename(
        entity: SceneEntityId,
        before: impl Into<String>,
        after: impl Into<String>,
    ) -> Self {
        Self::new(
            "重命名",
            CommandKind::Rename,
            vec![FieldChange::Name {
                entity,
                before: before.into(),
                after: after.into(),
            }],
        )
    }

    pub fn changes(&self) -> &[FieldChange] {
        &self.changes
    }

    /// 是否有任何一条变更真的会改变文档。
    pub fn is_effective(&self) -> bool {
        self.changes.iter().any(FieldChange::is_effective)
    }

    /// 补丁保留的字节数：与"快照历史"对比的就是这个数。
    pub fn retained_bytes(&self) -> usize {
        self.label.len()
            + self
                .changes
                .iter()
                .map(FieldChange::retained_bytes)
                .sum::<usize>()
            + std::mem::size_of::<Self>()
    }

    fn apply(&self, scene: &mut Scene, forward: bool) {
        for change in &self.changes {
            change.apply(scene, forward);
        }
    }

    /// 同一条补丁，正/反向落到场景上（生产历史与原型历史共用这一份实现，避免两套语义）。
    pub(crate) fn apply_to(&self, scene: &mut Scene, forward: bool) {
        self.apply(scene, forward);
    }

    /// 相邻同类变换的合并：同目标**同字段**只把终点推到最新，起点保持最早那次；
    /// 这一步新触碰到的字段原样追加（那条变更的 `before` 正好等于合并前那条的终点状态，
    /// 所以追加之后"一次撤销回到起点"仍然成立）。
    ///
    /// 字段必须逐个对上（[`same_slot`]）：只按实体配对会把同一实体上的 `Quad` 与 `Transform`
    /// 并成一条，先前的终点被后一次覆盖——那是丢编辑，不是合并。
    pub(crate) fn merge_next(&mut self, next: &Self) {
        for change in &next.changes {
            let position = self.changes.iter().position(|candidate| {
                candidate.entity() == change.entity() && same_slot(candidate, change)
            });
            match position {
                Some(index) => match (&mut self.changes[index], change) {
                    (
                        FieldChange::Component { after: slot, .. },
                        FieldChange::Component { after, .. },
                    ) => *slot = after.clone(),
                    (FieldChange::Name { after: slot, .. }, FieldChange::Name { after, .. }) => {
                        *slot = after.clone()
                    }
                    _ => {}
                },
                None => self.changes.push(change.clone()),
            }
        }
    }
}

/// 两条变更是否作用在**同一个可写字段**上。
fn same_slot(a: &FieldChange, b: &FieldChange) -> bool {
    match (a, b) {
        (
            FieldChange::Component {
                component: left, ..
            },
            FieldChange::Component {
                component: right, ..
            },
        ) => left == right,
        (FieldChange::Name { .. }, FieldChange::Name { .. }) => true,
        _ => false,
    }
}

/// 一次 `Transform3D` 字段编辑的便利构造（走 `Component`，因此可以按字段最小改写）。
pub fn transform3d_patch(entity: SceneEntityId, before: String, after: Transform3D) -> EditPatch {
    EditPatch::set_component(
        entity,
        "Transform",
        Some(before),
        crate::document_space::encode_transform(
            rustlua_ecs::spatial_transform::LocalTransform::from_3d(after),
        ),
    )
}

/// 把 `before` → `after` 的差别写成一条字段级补丁；**压不动就返回 `None`**。
///
/// 这里认识的只有 [`FieldChange`] 那两类可写字段（实体名、组件载荷的增/改/删），但**判据不是
/// 白名单**：先把差集写成补丁，再把补丁打到 `before` 的副本上，要求结果与 `after` **逐字段相同**。
/// 于是 `Scene::version` / `Scene::name` / 两级 `metadata` / 实体 `resources` / 父子关系这些
/// 补丁表达不了的东西不需要在这里逐个列——它们一变，回读就对不上，调用方退回快照形态。
/// 以后给 `Scene` 加字段也不用同步改这里。
///
/// 代价：一次 `Scene` 克隆 + 一次全字段比较，都在**提交点**上（快照形态本来就要克隆两份文档），
/// 不在每帧热路。
pub fn scene_field_patch(
    label: impl Into<String>,
    kind: CommandKind,
    before: &Scene,
    after: &Scene,
) -> Option<EditPatch> {
    if before == after {
        return None;
    }
    // 实体集合必须一模一样：增删实体是结构变更，需要的是"被删掉的那棵子树"，不属于字段级通道。
    if before.entities.len() != after.entities.len() {
        return None;
    }
    let label = label.into();
    let mut changes = Vec::new();
    for (id, before_entity) in &before.entities {
        let after_entity = after.entities.get(id)?;
        // 名字之外，结构/归属/标记/资源任一处不同就整体退回快照（回读也会拒，这里只是早退）。
        if before_entity.id != after_entity.id
            || before_entity.parent != after_entity.parent
            || before_entity.children != after_entity.children
            || before_entity.metadata != after_entity.metadata
            || before_entity.resources != after_entity.resources
        {
            return None;
        }
        if before_entity.name != after_entity.name {
            changes.push(FieldChange::Name {
                entity: *id,
                before: before_entity.name.clone(),
                after: after_entity.name.clone(),
            });
        }
        for (component, before_value) in &before_entity.components {
            match after_entity.components.get(component) {
                Some(after_value) if after_value != before_value => {
                    changes.push(FieldChange::Component {
                        entity: *id,
                        component: component.clone(),
                        before: Some(before_value.clone()),
                        after: Some(after_value.clone()),
                    });
                }
                Some(_) => {}
                None => changes.push(FieldChange::Component {
                    entity: *id,
                    component: component.clone(),
                    before: Some(before_value.clone()),
                    after: None,
                }),
            }
        }
        for (component, after_value) in &after_entity.components {
            if !before_entity.components.contains_key(component) {
                changes.push(FieldChange::Component {
                    entity: *id,
                    component: component.clone(),
                    before: None,
                    after: Some(after_value.clone()),
                });
            }
        }
    }
    if changes.is_empty() {
        // 只差在场景级字段上（name / version / metadata…）——那不是字段级通道。
        return None;
    }
    let mut rehearsal = before.clone();
    for change in &changes {
        change.apply(&mut rehearsal, true);
    }
    (rehearsal == *after).then(|| EditPatch::new(label, kind, changes))
}

/// 补丁历史（原型）。语义与 `CommandHistory` 对齐，但每条记录的成本只与**改动**成正比。
#[derive(Clone, Debug, Default)]
pub struct PatchHistory {
    undo: Vec<EditPatch>,
    redo: Vec<EditPatch>,
    max_entries: usize,
    dropped: u64,
}

impl PatchHistory {
    pub fn new(max_entries: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            max_entries: max_entries.max(1),
            dropped: 0,
        }
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn dropped_entries(&self) -> u64 {
        self.dropped
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// 保留的补丁总字节数（原型存在的理由就是把它压下来）。
    pub fn retained_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(EditPatch::retained_bytes)
            .sum()
    }

    /// 最近一条历史标签（Undo 菜单/状态栏用）。
    pub fn last_label(&self) -> Option<&str> {
        self.undo.last().map(|patch| patch.label.as_str())
    }

    /// 记录并应用一次编辑。空操作**不入栈**（与快照历史一致），返回是否真的改动了文档。
    ///
    /// 相邻合并与快照历史同规则：连续的 `Transform` / `Transform3D` 同类命令只更新最新的 `after`
    /// （拖动过程中的连续小步不该占满历史）。
    pub fn apply(&mut self, scene: &mut Scene, patch: EditPatch) -> bool {
        if !patch.is_effective() {
            return false;
        }
        // 先落到文档，再决定怎么记：合并路径只留最早的 `before` + 最新的 `after`。
        patch.apply(scene, true);
        let mergeable = matches!(
            patch.kind,
            CommandKind::Transform(_) | CommandKind::Transform3D(_)
        );
        if mergeable
            && let Some(previous) = self.undo.last_mut()
            && previous.kind == patch.kind
        {
            previous.merge_next(&patch);
            self.redo.clear();
            return true;
        }
        self.redo.clear();
        self.undo.push(patch);
        self.enforce_budget();
        true
    }

    fn enforce_budget(&mut self) {
        let excess = self.undo.len().saturating_sub(self.max_entries);
        if excess > 0 {
            self.undo.drain(..excess);
            self.dropped += excess as u64;
        }
    }

    /// 撤销一条；返回被撤销的标签。
    pub fn undo(&mut self, scene: &mut Scene) -> Option<String> {
        let patch = self.undo.pop()?;
        patch.apply(scene, false);
        let label = patch.label.clone();
        self.redo.push(patch);
        Some(label)
    }

    /// 重做一条；返回被重做的标签。
    pub fn redo(&mut self, scene: &mut Scene) -> Option<String> {
        let patch = self.redo.pop()?;
        patch.apply(scene, true);
        let label = patch.label.clone();
        self.undo.push(patch);
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandHistory, SceneDocument};

    fn scene_with_components(count: u64) -> (SceneDocument, Vec<SceneEntityId>) {
        let mut document = SceneDocument::new_scene("patch");
        let mut ids = Vec::new();
        for index in 0..count {
            let id = SceneEntityId(index + 1);
            document
                .scene
                .add_entity(id, format!("Node{index}"), None)
                .expect("加实体");
            document
                .scene
                .entities
                .get_mut(&id)
                .expect("实体存在")
                .components
                .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
            ids.push(id);
        }
        (document, ids)
    }

    /// 同一串编辑分别走"补丁历史"与"快照历史"，最终文本必须**逐字节相同**。
    ///
    /// 这就是原型必须证明的那条不变量（docs/55 §8 硬门槛：Undo 不等价直接阻断）。
    #[test]
    fn patch_history_is_equivalent_to_snapshot_history() {
        let (mut patch_doc, ids) = scene_with_components(8);
        let (mut snapshot_doc, _) = scene_with_components(8);
        let mut patch_history = PatchHistory::new(200);
        let mut snapshot_history = CommandHistory::new();

        let edits: Vec<(SceneEntityId, String, String)> = (0..24)
            .map(|step| {
                let id = ids[step % ids.len()];
                let before = patch_doc.scene.entities[&id].components["Quad"].clone();
                (
                    id,
                    before,
                    format!("size={},{};color=1,1,1,1", 100 + step, 200 + step),
                )
            })
            .collect();

        for (id, before, after) in &edits {
            patch_history.apply(
                &mut patch_doc.scene,
                EditPatch::set_component(*id, "Quad", Some(before.clone()), after.clone()),
            );
            let after_for_snapshot = after.clone();
            snapshot_history
                .execute(
                    &mut snapshot_doc,
                    "设置组件",
                    crate::CommandKind::Property,
                    |document| {
                        document
                            .scene
                            .entities
                            .get_mut(id)
                            .expect("实体存在")
                            .components
                            .insert("Quad".to_owned(), after_for_snapshot.clone());
                        Ok::<(), ()>(())
                    },
                )
                .expect("快照编辑");
        }

        assert_eq!(
            patch_doc.scene.to_text(),
            snapshot_doc.scene.to_text(),
            "应用同一串编辑之后，两条历史的文档必须逐字节相同"
        );
        assert_eq!(patch_history.undo_len(), snapshot_history.undo_len());
        let final_text = patch_doc.scene.to_text();

        // 全部撤销：两份文档都要回到起点。
        let (fresh, _) = scene_with_components(8);
        let original = fresh.scene.to_text();
        while patch_history.can_undo() {
            patch_history.undo(&mut patch_doc.scene).expect("undo");
        }
        while snapshot_history.can_undo() {
            snapshot_history.undo(&mut snapshot_doc).expect("快照撤销");
        }
        assert_eq!(patch_doc.scene.to_text(), original, "补丁撤销必须回到原文");
        assert_eq!(
            snapshot_doc.scene.to_text(),
            original,
            "快照撤销必须回到原文"
        );

        // 全部重做：两份文档都要回到终态。
        while patch_history.can_redo() {
            patch_history.redo(&mut patch_doc.scene).expect("redo");
        }
        while snapshot_history.can_redo() {
            snapshot_history.redo(&mut snapshot_doc).expect("快照重做");
        }
        assert_eq!(
            patch_doc.scene.to_text(),
            final_text,
            "补丁重做必须回到终态"
        );
        assert_eq!(
            snapshot_doc.scene.to_text(),
            final_text,
            "快照重做必须回到终态"
        );
    }

    #[test]
    fn no_op_patches_are_not_recorded() {
        let (mut document, ids) = scene_with_components(4);
        let mut history = PatchHistory::new(200);
        let current = document.scene.entities[&ids[0]].components["Quad"].clone();
        let applied = history.apply(
            &mut document.scene,
            EditPatch::set_component(ids[0], "Quad", Some(current.clone()), current),
        );
        assert!(!applied, "写同值必须被判定为空操作");
        assert_eq!(history.undo_len(), 0);
        assert_eq!(history.retained_bytes(), 0);
    }

    #[test]
    fn adjacent_transforms_merge_and_budget_drops_the_oldest() {
        let (mut document, ids) = scene_with_components(4);
        let mut history = PatchHistory::new(2);
        let id = ids[0];
        // 连续三次同目标 Transform：合并成一条。
        for step in 1..=3 {
            let before = document.scene.entities[&id].components["Quad"].clone();
            history.apply(
                &mut document.scene,
                EditPatch::new(
                    "Move",
                    crate::CommandKind::Transform(id),
                    vec![FieldChange::Component {
                        entity: id,
                        component: "Quad".to_owned(),
                        before: Some(before),
                        after: Some(format!("size={step},1;color=1,1,1,1")),
                    }],
                ),
            );
        }
        assert_eq!(history.undo_len(), 1, "相邻同类变换必须合并为一条");

        // 预算：再推两条不同类的编辑，最旧的一条被丢弃并计数。
        for step in 0..2 {
            let before = document.scene.entities[&id].components["Quad"].clone();
            history.apply(
                &mut document.scene,
                EditPatch::set_component(
                    id,
                    "Quad",
                    Some(before),
                    format!("size=9{step},1;color=1,1,1,1"),
                ),
            );
        }
        assert_eq!(history.undo_len(), 2, "预算上限生效");
        assert_eq!(history.dropped_entries(), 1, "丢弃条数要被记录");
    }

    #[test]
    fn patches_cost_a_fraction_of_full_snapshots() {
        let (mut document, ids) = scene_with_components(1_000);
        let snapshot_bytes = document.scene.to_text().len();
        let mut history = PatchHistory::new(200);
        for step in 0..200 {
            let id = ids[step % ids.len()];
            let before = document.scene.entities[&id].components["Quad"].clone();
            history.apply(
                &mut document.scene,
                EditPatch::set_component(
                    id,
                    "Quad",
                    Some(before),
                    format!("size={},7;color=1,1,1,1", 100 + step),
                ),
            );
        }
        let patch_bytes = history.retained_bytes();
        // 200 条快照历史保留的是 200 × 整份文档（before+after 由 Arc/克隆决定，这里按下界算）。
        let snapshot_retained = snapshot_bytes * 200;
        assert!(
            patch_bytes * 50 < snapshot_retained,
            "补丁历史应当比快照历史小一个数量级以上：patch={patch_bytes} snapshot_lower_bound={snapshot_retained}（单份文档 {snapshot_bytes} 字节）"
        );
    }
}
