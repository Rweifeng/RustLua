use crate::SceneDocument;
use engine::core::scene::SceneEntityId;
use std::{error::Error, fmt};

/// 一条活动中的事务：把一次连续交互（拖拽 / 数值拖动 / 连续绘制…）合并为
/// 一条可 Undo 的 `EditCommand`。事务期间只更新 preview（调用方自行在
/// 工作区/画面上更新），磁盘与 History 均不写入，直到 `commit_transaction`。
#[derive(Clone, Debug)]
pub(crate) struct EditTransaction {
    /// 事务标签（Undo/Redo 菜单里显示）。
    pub label: String,
    /// 命令种类（决定与上一条命令是否相邻合并，见 E08 接入时确认）。
    pub kind: CommandKind,
    /// 目标对象（拖拽实体等；默认无）。
    pub target: Option<SceneEntityId>,
    /// 事务开始前的整文档快照：commit 时作为 EditCommand.before，
    /// cancel 时直接还原到这里。
    pub before: SceneDocument,
}

/// A reversible editor operation backed by document snapshots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Generic,
    Transform(SceneEntityId),
    Transform3D(SceneEntityId),
    Create,
    Delete,
    Rename,
    Reparent,
    Property,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCommand {
    pub label: String,
    pub kind: CommandKind,
    before: SceneDocument,
    after: SceneDocument,
}

impl EditCommand {
    pub fn new(
        label: impl Into<String>,
        kind: CommandKind,
        before: SceneDocument,
        after: SceneDocument,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            before,
            after,
        }
    }
    pub fn before(&self) -> &SceneDocument {
        &self.before
    }
    pub fn after(&self) -> &SceneDocument {
        &self.after
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommandHistory {
    undo: Vec<EditCommand>,
    redo: Vec<EditCommand>,
    /// 活动事务（E07）。一次连续交互只允许一条；为空 = 未在事务中。
    transaction: Option<EditTransaction>,
}

/// 事务 API 错误：违反事务状态机（嵌套 begin / 无活动事务时提交或取消）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionError {
    AlreadyActive,
    NotActive,
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlreadyActive => "a transaction is already active",
            Self::NotActive => "no active transaction",
        })
    }
}
impl Error for TransactionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryError {
    NothingToUndo,
    NothingToRedo,
}
impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NothingToUndo => "nothing to undo",
            Self::NothingToRedo => "nothing to redo",
        })
    }
}
impl Error for HistoryError {}

impl CommandHistory {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Executes an edit; no-op edits are not recorded. New edits clear redo.
    pub fn execute<F, E>(
        &mut self,
        document: &mut SceneDocument,
        label: impl Into<String>,
        kind: CommandKind,
        edit: F,
    ) -> Result<bool, E>
    where
        F: FnOnce(&mut SceneDocument) -> Result<(), E>,
    {
        let before = document.clone();
        edit(document)?;
        if *document == before {
            return Ok(false);
        }
        let after = document.clone();
        self.redo.clear();
        if let Some(previous) = self.undo.last_mut()
            && previous.kind == kind
            && matches!(
                kind,
                CommandKind::Transform(_) | CommandKind::Transform3D(_)
            )
        {
            previous.after = after;
        } else {
            self.undo.push(EditCommand::new(label, kind, before, after));
        }
        Ok(true)
    }

    pub fn push(&mut self, command: EditCommand) {
        if command.before != command.after {
            self.redo.clear();
            self.undo.push(command);
        }
    }

    /// E07：是否处于活动事务中。任意离散 `execute`/`push` 在事务外照常工作。
    pub fn is_transaction_active(&self) -> bool {
        self.transaction.is_some()
    }

    /// E07：开始一条事务（E08 之前由 EditorApp 顶层网关调用）。
    ///
    /// - 禁止嵌套：已有活动事务时返回 `TransactionError::AlreadyActive`，快照不落盘。
    /// - 事务开始仅记录 `before` 快照；文档此刻不得被修改（若已修改，无法被单条
    ///   Undo 收回，校验靠调用点约定 + 回归测试覆盖）。
    /// - 文档为 dirty 时（历史已含未保存编辑）也会记录事务快照，不影响提交/取消。
    pub fn begin_transaction(
        &mut self,
        document: &SceneDocument,
        label: impl Into<String>,
        kind: CommandKind,
        target: Option<SceneEntityId>,
    ) -> Result<(), TransactionError> {
        if self.transaction.is_some() {
            return Err(TransactionError::AlreadyActive);
        }
        self.transaction = Some(EditTransaction {
            label: label.into(),
            kind,
            target,
            before: document.clone(),
        });
        Ok(())
    }

    /// E07：提交活动事务。
    ///
    /// - `after` 为事务结束后的最新文档；若与 `before` 无内容差异（空操作）则不追加
    ///   History，返回 `Ok(false)`，也不标记 dirty（提交语义由 EditorApp 统一处理）。
    /// - 有差异时清空 redo 并追加一条 `EditCommand`（label/kind/target 来自事务元数据，
    ///   target 放入 kind：Transform/Transform3D 可继续与上一条同对象命令相邻合并）。
    /// - 提交后事务被清除。
    pub fn commit_transaction(
        &mut self,
        after: &SceneDocument,
    ) -> Result<bool, TransactionError> {
        let Some(tx) = self.transaction.take() else {
            return Err(TransactionError::NotActive);
        };
        if tx.before == *after {
            return Ok(false);
        }
        self.push_transaction_command(tx, after);
        Ok(true)
    }

    /// E07：取消活动事务 —— 文档还原到 `before` 快照（调用方随后替换当前文档），
    /// 不追加 History、不清 dirty 标记。
    pub fn cancel_transaction(
        &mut self,
        document: &mut SceneDocument,
    ) -> Result<(), TransactionError> {
        let Some(tx) = self.transaction.take() else {
            return Err(TransactionError::NotActive);
        };
        *document = tx.before;
        Ok(())
    }

    /// E07：取出并消费活动事务（应用退出 / 保存兜底）。若正处于事务中，最后一条
    /// preview 仍会被提交 —— 见 `save_scene` 前的自动提交策略。无事务时不动作。
    pub fn finish_transaction(&mut self, document: &SceneDocument) {
        let Some(tx) = self.transaction.take() else {
            return;
        };
        if tx.before == *document {
            return;
        }
        self.push_transaction_command(tx, document);
    }

    /// 将一条已消费的事务写入 undo 栈：target 注入 kind（保持 Transform/3D 合并语义）。
    fn push_transaction_command(&mut self, tx: EditTransaction, after: &SceneDocument) {
        let kind = match tx.target {
            Some(target) => match tx.kind {
                CommandKind::Transform(_) => CommandKind::Transform(target),
                CommandKind::Transform3D(_) => CommandKind::Transform3D(target),
                other => other,
            },
            None => tx.kind,
        };
        self.redo.clear();
        self.undo
            .push(EditCommand::new(tx.label, kind, tx.before, after.clone()));
    }

    pub fn undo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.undo.pop().ok_or(HistoryError::NothingToUndo)?;
        *document = command.before.clone();
        self.redo.push(command);
        Ok(self.redo.last().expect("just pushed").label.as_str())
    }
    pub fn redo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.redo.pop().ok_or(HistoryError::NothingToRedo)?;
        *document = command.after.clone();
        self.undo.push(command);
        Ok(self.undo.last().expect("just pushed").label.as_str())
    }
}

/// E07 顶层事务门面：在 EditorApp 内与 `history` 分离持有。
///
/// E07 阶段（尚未接入拖拽前）EditorApp 内部离散命令与事务命令并存，`before`
/// 快照取自 begin 时的脏文档；因此一条事务 Undo 会回到 begin 前的内容（等价于
/// 与事务前最后一条离散命令合并）。文档从打开到未保存编辑被隔离时（E08），
/// 单条快照模型的边界即足够精确。见 history.rs 单测
/// `commit_merges_undo_with_preceding_transactional_edits_when_dirty`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditTransactionState {
    /// 是否为一次活动事务。由 EditorApp 的唯一网关控制，读取用于帧结束自动提交
    /// 与撤销自动取消的决策。
    pub active: bool,
    /// 事务是否已在本帧被提交/取消处理过（防止帧内重复消费）。
    pub consumed: bool,
}

impl EditTransactionState {
    pub fn is_active(&self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc() -> SceneDocument {
        SceneDocument::new("main")
    }
    #[test]
    fn undo_redo_and_new_edit_branch() {
        let mut d = doc();
        let mut h = CommandHistory::new();
        h.execute(&mut d, "create", CommandKind::Create, |d| {
            d.scene
                .add_entity(SceneEntityId(1), "A", None)
                .map_err(|_| ())
        })
        .unwrap();
        h.undo(&mut d).unwrap();
        assert!(d.scene.entities.is_empty());
        h.redo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        h.execute(&mut d, "other", CommandKind::Generic, |d| {
            d.scene
                .add_entity(SceneEntityId(2), "B", None)
                .map_err(|_| ())
        })
        .unwrap();
        assert!(!h.can_redo());
    }
    #[test]
    fn consecutive_transforms_merge() {
        let mut d = doc();
        d.scene.add_entity(SceneEntityId(1), "A", None).unwrap();
        let mut h = CommandHistory::new();
        for value in 1..=100 {
            h.execute(
                &mut d,
                "transform",
                CommandKind::Transform(SceneEntityId(1)),
                |d| {
                    d.scene
                        .entities
                        .get_mut(&SceneEntityId(1))
                        .unwrap()
                        .components
                        .insert("x".into(), value.to_string());
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        }
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut d).unwrap();
        assert!(
            !d.scene.entities[&SceneEntityId(1)]
                .components
                .contains_key("x")
        );
    }
    #[test]
    fn consecutive_transform3d_edits_merge() {
        let mut d = doc();
        d.scene.add_entity(SceneEntityId(7), "A", None).unwrap();
        let mut h = CommandHistory::new();
        for value in 1..=50 {
            h.execute(
                &mut d,
                "3d transform",
                CommandKind::Transform3D(SceneEntityId(7)),
                |d| {
                    d.scene
                        .entities
                        .get_mut(&SceneEntityId(7))
                        .unwrap()
                        .components
                        .insert("Transform3D".into(), format!("position={value},0,0"));
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        }
        assert_eq!(h.undo_len(), 1, "consecutive 3D transforms merge");
    }

    fn entity_component(id: SceneEntityId, key: &str, value: &str) -> SceneDocument {
        let mut d = doc();
        d.scene.add_entity(id, "A", None).unwrap();
        d.scene.entities.get_mut(&id).unwrap().components
            .insert(key.to_owned(), value.to_owned());
        d.mark_clean();
        d
    }

    // E07：一次连续编辑 = begin → 多次更新 → commit，恰好产生一条 Undo。
    #[test]
    fn committed_transaction_produces_exactly_one_undo_entry() {
        let id = SceneEntityId(1);
        let mut d = entity_component(id, "x", "0");
        let mut h = CommandHistory::new();
        h.begin_transaction(&d, "Drag", CommandKind::Transform(id), Some(id))
            .unwrap();
        for value in 1..=30 {
            d.scene.entities.get_mut(&id).unwrap().components
                .insert("x".into(), value.to_string());
            d.mark_dirty();
        }
        assert!(h.is_transaction_active());
        assert_eq!(h.undo_len(), 0, "事务期间不得追加历史");
        assert!(
            h.commit_transaction(&d).unwrap(),
            "有差异的提交应返回 true"
        );
        assert!(!h.is_transaction_active());
        assert_eq!(h.undo_len(), 1, "一次连续编辑只产生一条 Undo");

        h.undo(&mut d).unwrap();
        assert_eq!(
            d.scene.entities[&id].components["x"],
            "0",
            "Undo 应整体回到事务开始前"
        );
        assert_eq!(h.redo_len(), 1);
        h.redo(&mut d).unwrap();
        assert_eq!(
            d.scene.entities[&id].components["x"],
            "30",
            "Redo 应整体回到事务结束后"
        );
    }

    // E07：未改动的提交是空操作 —— 不追加历史，返回 false。
    #[test]
    fn commit_without_changes_is_a_noop() {
        let d = doc();
        let mut h = CommandHistory::new();
        h.begin_transaction(&d, "Empty Drag", CommandKind::Transform(SceneEntityId(1)), None)
            .unwrap();
        assert!(!h.commit_transaction(&d).unwrap());
        assert!(!h.is_transaction_active());
        assert_eq!(h.undo_len(), 0);
        assert_eq!(h.redo_len(), 0);
    }

    // E07：取消恢复到事务开始前，不产生历史。
    #[test]
    fn cancel_transaction_restores_before_without_history() {
        let id = SceneEntityId(1);
        let mut d = entity_component(id, "x", "0");
        let mut h = CommandHistory::new();
        let before = d.clone();
        h.begin_transaction(&d, "Drag", CommandKind::Transform(id), Some(id))
            .unwrap();
        d.scene.entities.get_mut(&id).unwrap().components
            .insert("x".into(), "99".to_owned());
        h.cancel_transaction(&mut d).unwrap();
        assert!(!h.is_transaction_active());
        assert_eq!(d, before, "取消必须恢复事务开始前状态");
        assert_eq!(h.undo_len(), 0, "取消不产生历史");
    }

    // E07：事务进行中再 begin 属嵌套，被拒绝且原事务不受影响。
    #[test]
    fn nested_begin_transaction_is_rejected() {
        let id = SceneEntityId(1);
        let mut d = entity_component(id, "x", "0");
        let mut h = CommandHistory::new();
        h.begin_transaction(&d, "First", CommandKind::Transform(id), Some(id))
            .unwrap();
        assert_eq!(
            h.begin_transaction(&d, "Second", CommandKind::Generic, None),
            Err(TransactionError::AlreadyActive)
        );
        assert!(h.is_transaction_active());
        d.scene.entities.get_mut(&id).unwrap().components
            .insert("x".into(), "5".to_owned());
        assert!(h.commit_transaction(&d).unwrap());
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut d).unwrap();
        assert_eq!(d.scene.entities[&id].components["x"], "0");
    }

    // E07：无活动事务时提交/取消报错，文档不受影响。
    #[test]
    fn commit_or_cancel_without_active_transaction_errors() {
        let d = doc();
        let mut h = CommandHistory::new();
        assert_eq!(
            h.commit_transaction(&d),
            Err(TransactionError::NotActive)
        );
        assert_eq!(
            h.cancel_transaction(&mut d.clone()),
            Err(TransactionError::NotActive)
        );
    }

    // E07：事务期间在文档上执行离散命令等同踩脏 before 快照；退出兜底会把
    // 事务外编辑与事务合并为一条（沿用 before）。单条快照模型的既定取舍。
    #[test]
    fn commit_merges_undo_with_preceding_transactional_edits_when_dirty() {
        let id = SceneEntityId(1);
        let mut d = doc();
        d.scene.add_entity(id, "A", None).unwrap();
        d.mark_clean();
        let mut h = CommandHistory::new();
        let pristine = d.clone();
        // 离散编辑（历史快照 1）。
        h.execute(&mut d, "Rename", CommandKind::Generic, |d| {
            d.scene.entities.get_mut(&id).unwrap().name = "B".into();
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(h.undo_len(), 1);
        // 事务开始（此时 dirty 来自上一编辑）。
        h.begin_transaction(&d, "Drag", CommandKind::Transform(id), Some(id))
            .unwrap();
        d.scene.entities.get_mut(&id).unwrap().components
            .insert("x".into(), "9".to_owned());
        assert!(h.commit_transaction(&d).unwrap());
        assert_eq!(h.undo_len(), 2);
        // Undo 事务 → 回到 rename 后；Undo rename → 回到 pristine。
        h.undo(&mut d).unwrap();
        assert_eq!(d.scene.entities[&id].name, "B");
        h.undo(&mut d).unwrap();
        assert_eq!(d, pristine);
    }

    // E07：事务期间保持 dirty（E08 语义：预览不写磁盘）时，自动保存兜底
    // finish_transaction 仍产出单条历史，后续 undo/redo 语义正确。
    #[test]
    fn dirty_transaction_finish_flushes_single_entry() {
        let id = SceneEntityId(1);
        let mut d = entity_component(id, "x", "0");
        let mut h = CommandHistory::new();
        h.begin_transaction(&d, "Drag", CommandKind::Transform(id), Some(id))
            .unwrap();
        d.scene.entities.get_mut(&id).unwrap().components
            .insert("x".into(), "7".to_owned());
        d.mark_dirty();
        assert!(h.is_transaction_active());
        h.finish_transaction(&d);
        assert!(!h.is_transaction_active());
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut d).unwrap();
        assert_eq!(d.scene.entities[&id].components["x"], "0");
        assert!(!d.is_dirty(), "Undo 后回到事务前内容");
    }
}
