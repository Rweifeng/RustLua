use crate::SceneDocument;
use crate::patch_history::{EditPatch, scene_field_patch};
use rustlua_scene::scene::{Scene, SceneEntityId};
use std::path::PathBuf;

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

/// 一条编辑的种类：决定相邻命令能否合并，以及 Undo 菜单怎么归类。
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

/// 一条可撤销操作。
///
/// 同一条栈上共存**两种存储形态**（M79 P1-2b：ER-03 的补丁原型上生产）：字段级编辑存
/// **补丁**（成本只与改动成正比），结构变更仍存**整文档快照**（增删实体要带"被删掉的那棵
/// 子树"，补丁通道不假装支持）。两种形态对外语义逐字相同：`undo`/`redo` 之后场景内容、
/// `path`、`dirty`、`revision`、`disk_signature` 都和纯快照实现一项一项对得上——
/// 后四项由 `DocumentAnchor` 显式记住，不是跟着整份文档免费带的。
#[derive(Clone, Debug)]
pub struct EditCommand {
    pub label: String,
    pub kind: CommandKind,
    storage: CommandStorage,
}

/// 一条命令实际保留的东西。
#[derive(Clone, Debug)]
enum CommandStorage {
    Snapshots {
        before: SceneDocument,
        after: SceneDocument,
    },
    Patch {
        patch: EditPatch,
        before: DocumentAnchor,
        after: DocumentAnchor,
    },
}

/// 文档里**除场景内容以外**的那几个字段。
///
/// 快照形态下它们跟着文档一起走；换成补丁形态必须自己记住，否则撤销会把 `dirty` 与
/// `revision` 改掉——那两条分别决定 E06 换文档闸门和外部改动检测，不能变。
#[derive(Clone, Debug, PartialEq, Eq)]
struct DocumentAnchor {
    path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
    disk_signature: Option<crate::file_watcher::FileSignature>,
}

impl DocumentAnchor {
    fn of(document: &SceneDocument) -> Self {
        Self {
            path: document.path.clone(),
            dirty: document.dirty,
            revision: document.revision,
            disk_signature: document.disk_signature,
        }
    }

    /// 把锚点写回一份已经就地打好补丁的文档（场景内容不动）。
    fn restore_into(&self, document: &mut SceneDocument) {
        document.path = self.path.clone();
        document.dirty = self.dirty;
        document.revision = self.revision;
        document.disk_signature = self.disk_signature;
    }
}

impl EditCommand {
    /// 由前后两份文档构造一条命令。`compress` 为真时先试字段级补丁，压不动才留快照。
    ///
    /// `compress` 由 [`CommandHistory`] 决定（生产默认开）；留成显式参数，是为了让
    /// "同一串编辑在两种形态下必须逐项相同"这条差分门能各跑一遍，而不是只测一条路。
    pub fn new(
        label: impl Into<String>,
        kind: CommandKind,
        before: SceneDocument,
        after: SceneDocument,
        compress: bool,
    ) -> Self {
        let label = label.into();
        let patch = if compress {
            scene_field_patch(label.clone(), kind, &before.scene, &after.scene)
        } else {
            None
        };
        let storage = match patch {
            Some(patch) => CommandStorage::Patch {
                patch,
                before: DocumentAnchor::of(&before),
                after: DocumentAnchor::of(&after),
            },
            None => CommandStorage::Snapshots { before, after },
        };
        Self {
            label,
            kind,
            storage,
        }
    }

    /// 这一步是补丁形态还是快照形态（门与性能基线用它认领"确实压下去了"）。
    pub fn is_patch(&self) -> bool {
        matches!(self.storage, CommandStorage::Patch { .. })
    }

    /// 快照形态才有"整份 before / after 文档"可给；补丁形态只记住改动，这就是它省内存的地方。
    pub fn before_snapshot(&self) -> Option<&SceneDocument> {
        match &self.storage {
            CommandStorage::Snapshots { before, .. } => Some(before),
            CommandStorage::Patch { .. } => None,
        }
    }

    pub fn after_snapshot(&self) -> Option<&SceneDocument> {
        match &self.storage {
            CommandStorage::Snapshots { after, .. } => Some(after),
            CommandStorage::Patch { .. } => None,
        }
    }

    /// 这条命令**保留**的字节数（估算，口径见 [`CommandHistory::retained_bytes_estimate`]）。
    fn retained_bytes_estimate(&self) -> usize {
        self.label.len()
            + std::mem::size_of::<Self>()
            + match &self.storage {
                CommandStorage::Snapshots { before, after } => {
                    before.scene.to_text().len() + after.scene.to_text().len()
                }
                CommandStorage::Patch { patch, .. } => patch.retained_bytes(),
            }
    }

    /// 这条命令是否会改变文档（快照形态比两份文档；补丁形态由构造保证非空）。
    fn is_noop(&self) -> bool {
        match &self.storage {
            CommandStorage::Snapshots { before, after } => before == after,
            CommandStorage::Patch { .. } => false,
        }
    }

    /// 撤销这一步：快照形态整份还原，补丁形态就地反向打补丁再贴回文档级锚点。
    fn undo(&self, document: &mut SceneDocument) {
        match &self.storage {
            CommandStorage::Snapshots { before, .. } => *document = before.clone(),
            CommandStorage::Patch { patch, before, .. } => {
                patch.apply_to(&mut document.scene, false);
                before.restore_into(document);
            }
        }
    }

    fn redo(&self, document: &mut SceneDocument) {
        match &self.storage {
            CommandStorage::Snapshots { after, .. } => *document = after.clone(),
            CommandStorage::Patch { patch, after, .. } => {
                patch.apply_to(&mut document.scene, true);
                after.restore_into(document);
            }
        }
    }

    /// 相邻同类变换的合并：把终点推到最新。
    ///
    /// 快照形态直接换掉 `after` 文档。补丁形态用**这一步自己的差量**并入（同字段更新终点、
    /// 新触碰到的字段原样追加），于是合并成本仍然只与改动条数成正比；这一步的差量压不出来
    /// （说明拖动里夹了结构改动）时返回 `false`，由调用方**另起一条**——绝不在补丁形态下
    /// 整份替换文档，那样省下的内存又回来了。
    fn merge_after(
        &mut self,
        label: String,
        kind: CommandKind,
        before: &Scene,
        after: &SceneDocument,
    ) -> bool {
        match &mut self.storage {
            CommandStorage::Snapshots { after: slot, .. } => {
                *slot = after.clone();
                true
            }
            CommandStorage::Patch {
                patch,
                after: anchor,
                ..
            } => {
                let Some(step) = scene_field_patch(label, kind, before, &after.scene) else {
                    return false;
                };
                patch.merge_next(&step);
                *anchor = DocumentAnchor::of(after);
                true
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommandHistory {
    undo: Vec<EditCommand>,
    redo: Vec<EditCommand>,
    /// 活动事务（E07）。一次连续交互只允许一条；为空 = 未在事务中。
    transaction: Option<EditTransaction>,
    /// 累计**编辑**次数（撤销/重做不计数，合并中的连续变换也计数）。
    /// 只服务跨面板全局 Undo 的签名，见 [`CommandHistory::undo_signature`]。
    edits: u64,
    /// 补丁通道的**否定**开关：默认 `false`，也就是所有构造点（`new`、`default`、
    /// `CommandHistory::new()` 重建）都走字段级补丁。写成否定形式是刻意的——
    /// 若字段是"肯定"的 `patch_compression`，`#[derive(Default)]` 带出来的历史会静默
    /// 退回整文档快照，而这里没有任何测试会因此变红。
    no_patch_compression: bool,
}

/// 事务 API 错误：违反事务状态机（嵌套 begin / 无活动事务时提交或取消）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransactionError {
    #[error("已有活动事务")]
    AlreadyActive,
    #[error("没有活动事务")]
    NotActive,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HistoryError {
    #[error("没有可撤销的操作")]
    NothingToUndo,
    #[error("没有可重做的操作")]
    NothingToRedo,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// 关掉补丁通道的历史（只给差分门用：同一串编辑两条路各跑一遍再逐项比对）。
    ///
    /// 默认是**开**的，而且关的语义写成"否定"（`no_patch_compression`）而不是"肯定"，
    /// 这样 `CommandHistory::default()`、`#[derive(Default)]` 带出来的历史也都是开着补丁的——
    /// 反过来的话，任何一个走 `default()` 的构造点都会静默退回整文档快照。
    #[must_use]
    pub fn without_patch_compression() -> Self {
        Self {
            no_patch_compression: true,
            ..Self::default()
        }
    }

    /// 补丁通道是否开着（生产代码不碰它，门与性能基线用它声明自己测的是哪条路）。
    pub fn patch_compression_enabled(&self) -> bool {
        !self.no_patch_compression
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
    /// 跨面板全局 Undo 的**编辑签名**：`(累计编辑次数, undo 栈 + redo 栈深度)`。
    ///
    /// 两项各自管一件事，缺一不可：
    /// * 第一项只在真正新增一步时变大（撤销/重做不碰它）⇒ "这个归属刚刚又提交了"；
    /// * 第二项在 [`Self::clear`] 时回落到 0 ⇒ "这份文档的历史已经作废，别再问它"。
    ///
    /// 只用深度是不够的：栈到达上限后再提交，长度不变；撤销一步紧接着重新编辑，
    /// 深度还会回到同一个数。两种情况下"最近提交"都会被漏记，所以必须有计数器。
    /// 消费方见 editor bin 的 `actions/undo_ledger.rs`。
    #[must_use]
    pub fn undo_signature(&self) -> (u64, usize) {
        (self.edits, self.undo.len() + self.redo.len())
    }

    /// 新增一步编辑（不是撤销/重做的挪动）。合并中的连续变换同样计数：用户确实在改它。
    fn note_edit(&mut self) {
        self.edits = self.edits.wrapping_add(1);
    }
    /// Keep at most `max_entries` undo snapshots, dropping the oldest checkpoints first.
    pub fn enforce_undo_limit(&mut self, max_entries: usize) -> usize {
        let limit = max_entries.max(1);
        let excess = self.undo.len().saturating_sub(limit);
        if excess > 0 {
            self.undo.drain(..excess);
        }
        excess
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
        let label = label.into();
        let after = document.clone();
        self.redo.clear();
        self.note_edit();
        let mergeable = matches!(
            kind,
            CommandKind::Transform(_) | CommandKind::Transform3D(_)
        );
        if mergeable
            && self
                .undo
                .last()
                .is_some_and(|previous| previous.kind == kind)
        {
            // 拖动过程中的连续小步并成一条（菜单上只留第一次那个标签）。
            let merged = self.undo.last_mut().expect("刚检查过栈顶").merge_after(
                label.clone(),
                kind,
                &before.scene,
                &after,
            );
            if merged {
                return Ok(true);
            }
            // 并不动：这一步夹了补丁表达不了的改动，另起一条让快照去装它。
        }
        self.undo.push(EditCommand::new(
            label,
            kind,
            before,
            after,
            self.patch_compression_enabled(),
        ));
        Ok(true)
    }

    pub fn push(&mut self, command: EditCommand) {
        if !command.is_noop() {
            self.redo.clear();
            self.note_edit();
            self.undo.push(command);
        }
    }

    /// 两条栈上保留的字节数（**估算**）。
    ///
    /// 快照形态按"整份文档的文本长度"计（`to_text` 一遍，O(文档)），这比真实占用**偏小**：
    /// 实际还要 `SceneDocument` 的其它字段与堆上分配器的开销。补丁形态直接用
    /// [`EditPatch::retained_bytes`]。所以两侧都只当**下界**读，比值才有意义——
    /// 这条方法存在的理由是量化，不是记账。性能基线 `patch_history_production` 用它。
    #[must_use]
    pub fn retained_bytes_estimate(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(EditCommand::retained_bytes_estimate)
            .sum()
    }

    /// `(补丁条数, 快照条数)`：门用它认领"字段级编辑确实被压下去了"，而不是只测了个回退路径。
    #[must_use]
    pub fn storage_counts(&self) -> (usize, usize) {
        let patch = self
            .undo
            .iter()
            .chain(self.redo.iter())
            .filter(|command| command.is_patch())
            .count();
        let total = self.undo.len() + self.redo.len();
        (patch, total - patch)
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
    pub fn commit_transaction(&mut self, after: &SceneDocument) -> Result<bool, TransactionError> {
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
        self.note_edit();
        self.undo.push(EditCommand::new(
            tx.label,
            kind,
            tx.before,
            after.clone(),
            self.patch_compression_enabled(),
        ));
    }

    pub fn undo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.undo.pop().ok_or(HistoryError::NothingToUndo)?;
        command.undo(document);
        self.redo.push(command);
        Ok(self.redo.last().expect("just pushed").label.as_str())
    }
    pub fn redo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.redo.pop().ok_or(HistoryError::NothingToRedo)?;
        command.redo(document);
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
                        .insert("Transform".into(), format!("position={value},0,0"));
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
        d.scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
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
            d.scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("x".into(), value.to_string());
            d.mark_dirty();
        }
        assert!(h.is_transaction_active());
        assert_eq!(h.undo_len(), 0, "事务期间不得追加历史");
        assert!(h.commit_transaction(&d).unwrap(), "有差异的提交应返回 true");
        assert!(!h.is_transaction_active());
        assert_eq!(h.undo_len(), 1, "一次连续编辑只产生一条 Undo");

        h.undo(&mut d).unwrap();
        assert_eq!(
            d.scene.entities[&id].components["x"], "0",
            "Undo 应整体回到事务开始前"
        );
        assert_eq!(h.redo_len(), 1);
        h.redo(&mut d).unwrap();
        assert_eq!(
            d.scene.entities[&id].components["x"], "30",
            "Redo 应整体回到事务结束后"
        );
    }

    // E07：未改动的提交是空操作 —— 不追加历史，返回 false。
    #[test]
    fn commit_without_changes_is_a_noop() {
        let d = doc();
        let mut h = CommandHistory::new();
        h.begin_transaction(
            &d,
            "Empty Drag",
            CommandKind::Transform(SceneEntityId(1)),
            None,
        )
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
        d.scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
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
        d.scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
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
        assert_eq!(h.commit_transaction(&d), Err(TransactionError::NotActive));
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
        d.scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
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
        d.scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
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

    // ── M79 P1-2b：补丁通道与快照通道必须逐项同语义 ──────────────────────────

    /// 一份"文档级四项都离开默认值"的起点。
    ///
    /// `path` / `dirty` / `revision` / `disk_signature` 各有非默认取值，差分门才真的校到
    /// [`DocumentAnchor`]——补丁形态只保存改动，这四项是它必须自己记住的东西。
    fn anchored_document() -> SceneDocument {
        let id = SceneEntityId(1);
        let mut document = entity_component(id, "x", "0");
        document.path = Some(PathBuf::from("assets/scenes/main.scene"));
        document.disk_signature = Some(crate::file_watcher::FileSignature {
            len: 4096,
            modified: None,
        });
        document.mark_dirty();
        document
    }

    /// 差分探针能走的编辑。前三类是字段级（应被压成补丁），后三类是结构/场景级（必须退回快照）。
    #[derive(Clone, Copy, Debug)]
    enum Probe {
        Tint,
        Rename,
        AddComponent,
        Drag,
        DropComponent,
        AddEntity,
        Reparent,
        SceneField,
    }

    /// 一条明确排好的序列：两个字段级步骤之间夹一次拖动（含合并），结构变更放在最后，
    /// 于是 undo 阶段先撤快照、再撤补丁——两条通道的交接面正好被走满。
    const PROBE_SEQUENCE: &[Probe] = &[
        Probe::Tint,
        Probe::Rename,
        Probe::AddComponent,
        Probe::Drag,
        Probe::Drag,
        Probe::DropComponent,
        Probe::AddEntity,
        Probe::Reparent,
        Probe::SceneField,
    ];

    fn run_probe(history: &mut CommandHistory, document: &mut SceneDocument, step: Probe) {
        let id = SceneEntityId(1);
        let second = SceneEntityId(2);
        match step {
            Probe::Tint => {
                history
                    .execute(document, "tint", CommandKind::Property, |d| {
                        d.scene
                            .entities
                            .get_mut(&id)
                            .unwrap()
                            .components
                            .insert("tint".into(), "0.25".into());
                        d.mark_dirty();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::Rename => {
                history
                    .execute(document, "rename", CommandKind::Rename, |d| {
                        d.scene.entities.get_mut(&id).unwrap().name = "Hero".into();
                        d.mark_dirty();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::AddComponent => {
                history
                    .execute(document, "glow", CommandKind::Property, |d| {
                        d.scene
                            .entities
                            .get_mut(&id)
                            .unwrap()
                            .components
                            .insert("glow".into(), "1".into());
                        d.mark_dirty();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::Drag => {
                // 连续两次同 kind 的 Transform 写入：第二次必须并进第一次那一条。
                let value = document.scene.entities[&id].components["x"].clone();
                let next = value.parse::<u32>().expect("探针写入的是数字") + 1;
                history
                    .execute(document, "drag", CommandKind::Transform(id), |d| {
                        d.scene
                            .entities
                            .get_mut(&id)
                            .unwrap()
                            .components
                            .insert("x".into(), next.to_string());
                        d.mark_dirty();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::DropComponent => {
                history
                    .execute(document, "drop glow", CommandKind::Property, |d| {
                        d.scene
                            .entities
                            .get_mut(&id)
                            .unwrap()
                            .components
                            .remove("glow");
                        d.mark_dirty();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::AddEntity => {
                history
                    .execute(document, "create", CommandKind::Create, |d| {
                        d.scene.add_entity(second, "Side", None).map_err(|_| ())
                    })
                    .unwrap();
            }
            Probe::Reparent => {
                history
                    .execute(document, "reparent", CommandKind::Reparent, |d| {
                        d.scene.entities.get_mut(&second).unwrap().parent = Some(id);
                        d.scene.entities.get_mut(&id).unwrap().children.push(second);
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
            Probe::SceneField => {
                // 只改场景级字段：补丁表达不了（回读对不上），必须留快照。
                history
                    .execute(document, "scene name", CommandKind::Generic, |d| {
                        d.scene.name = "renamed".into();
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            }
        }
    }

    /// 逐字段比较两份文档。
    ///
    /// 这里**不能**只写 `assert_eq!(a, b)`：`SceneDocument` 的 `PartialEq` 刻意排除
    /// `revision` 与 `disk_signature`（免得纯修订号变化把 history 的空操作检测搅浑），
    /// 而这两个被排除的字段正是补丁形态必须自己记住的——门要校的就是它们。
    fn assert_documents_identical(stage: &str, patch: &SceneDocument, snapshot: &SceneDocument) {
        assert_eq!(
            patch.scene.to_text(),
            snapshot.scene.to_text(),
            "{stage}：场景内容不一致"
        );
        assert_eq!(patch.path, snapshot.path, "{stage}：path 不一致");
        assert_eq!(patch.dirty, snapshot.dirty, "{stage}：dirty 不一致");
        assert_eq!(
            patch.revision, snapshot.revision,
            "{stage}：revision 不一致"
        );
        assert_eq!(
            patch.disk_signature, snapshot.disk_signature,
            "{stage}：disk_signature 不一致"
        );
    }

    /// 主门：同一串编辑在两种形态下走一遍，每步之后、每一步撤销之后、每一步重做之后都必须
    /// 逐字段相同（含栈深与 #9 的全局账本签名）。
    #[test]
    fn patch_channel_matches_the_snapshot_channel_edit_by_edit() {
        let start = anchored_document();
        let mut patch_document = start.clone();
        let mut snapshot_document = start.clone();
        let mut patch_history = CommandHistory::new();
        let mut snapshot_history = CommandHistory::without_patch_compression();
        assert!(
            patch_history.patch_compression_enabled(),
            "生产默认必须是开补丁"
        );
        assert!(!snapshot_history.patch_compression_enabled());

        for step in PROBE_SEQUENCE {
            run_probe(&mut patch_history, &mut patch_document, *step);
            run_probe(&mut snapshot_history, &mut snapshot_document, *step);
            assert_documents_identical(
                &format!("{step:?} 提交后"),
                &patch_document,
                &snapshot_document,
            );
            assert_eq!(
                patch_history.undo_len(),
                snapshot_history.undo_len(),
                "{step:?}：两种形态的栈深必须一致，否则 Undo 菜单会多一步或少一步"
            );
            assert_eq!(
                patch_history.undo_signature(),
                snapshot_history.undo_signature(),
                "{step:?}：#9 跨面板全局账本靠这个签名认领\"最近提交\"，形态不能改变它"
            );
        }

        // 字段级编辑确实被压下去了（否则这条门只是在把回退路径测两遍）。
        assert_eq!(
            patch_history.storage_counts(),
            (5, 3),
            "5 条字段级（tint/rename/glow/合并后的 drag/drop glow）+ 3 条结构或场景级"
        );
        assert_eq!(
            snapshot_history.storage_counts(),
            (0, 8),
            "关掉补丁通道就该全是快照"
        );
        assert!(
            patch_history.retained_bytes_estimate() < snapshot_history.retained_bytes_estimate(),
            "同串编辑的保留字节：补丁 {} < 快照 {}",
            patch_history.retained_bytes_estimate(),
            snapshot_history.retained_bytes_estimate()
        );

        let final_document = patch_document.clone();
        while patch_history.can_undo() {
            let label = patch_history.undo(&mut patch_document).unwrap().to_owned();
            let snapshot_label = snapshot_history.undo(&mut snapshot_document).unwrap();
            assert_eq!(label, snapshot_label, "Undo 菜单的标签序列必须一致");
            assert_documents_identical(
                &format!("undo {label} 后"),
                &patch_document,
                &snapshot_document,
            );
        }
        assert_documents_identical("撤销到底", &patch_document, &start);
        while patch_history.can_redo() {
            let label = patch_history.redo(&mut patch_document).unwrap().to_owned();
            let snapshot_label = snapshot_history.redo(&mut snapshot_document).unwrap();
            assert_eq!(label, snapshot_label, "Redo 菜单的标签序列必须一致");
            assert_documents_identical(
                &format!("redo {label} 后"),
                &patch_document,
                &snapshot_document,
            );
        }
        assert_documents_identical("重做到底", &patch_document, &final_document);
    }

    /// 拖动中途新触碰一个字段：仍然只占一条历史，撤销要把**两个**字段一起送回起点。
    ///
    /// 这条是 [`crate::patch_history::same_slot`] 存在的理由。按"同实体"配对（原型最初的写法，
    /// 只比枚举判别式）会把 `Quad` 那一步并到 `Transform` 那条上，先前记录的终点被覆盖——
    /// 撤销只回了一个字段，另一个字段的新值白丢了。那是丢编辑，不是合并。
    #[test]
    fn a_drag_that_newly_touches_another_component_still_reverts_both() {
        let id = SceneEntityId(1);
        let mut document = entity_component(id, "x", "0");
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity
            .components
            .insert("Transform".into(), "position=0,0,0".into());
        entity
            .components
            .insert("Quad".into(), "color=0,0,0,1".into());
        let mut history = CommandHistory::new();
        for value in 1..=2 {
            history
                .execute(&mut document, "drag", CommandKind::Transform(id), |d| {
                    d.scene
                        .entities
                        .get_mut(&id)
                        .unwrap()
                        .components
                        .insert("Transform".into(), format!("position={value},0,0"));
                    Ok::<_, ()>(())
                })
                .unwrap();
        }
        // 同一次拖动里 Inspector 又碰了另一个组件：并入同一条，但作为**独立字段**记录。
        history
            .execute(&mut document, "drag", CommandKind::Transform(id), |d| {
                d.scene
                    .entities
                    .get_mut(&id)
                    .unwrap()
                    .components
                    .insert("Quad".into(), "color=1,0,0,1".into());
                Ok::<_, ()>(())
            })
            .unwrap();

        assert_eq!(history.undo_len(), 1, "相邻同类拖动只该占一条");
        assert_eq!(history.storage_counts(), (1, 0), "合并后仍是补丁形态");
        history.undo(&mut document).unwrap();
        let entity = &document.scene.entities[&id];
        assert_eq!(
            entity.components["Transform"], "position=0,0,0",
            "Transform 要回到最早那次之前"
        );
        assert_eq!(
            entity.components["Quad"], "color=0,0,0,1",
            "中途新触碰的 Quad 也必须一起回去——它挂在同一条补丁上"
        );
        history.redo(&mut document).unwrap();
        let entity = &document.scene.entities[&id];
        assert_eq!(entity.components["Transform"], "position=2,0,0");
        assert_eq!(entity.components["Quad"], "color=1,0,0,1");
    }

    /// **补丁形态的既定口径**：撤销只送回它自己碰过的字段，带外写入的其它字段存活。
    ///
    /// 快照形态会把带外写入一起抹掉。这里两种行为都断言下来，是因为这个差别是真的，
    /// 而 [`patch_channel_matches_the_snapshot_channel_edit_by_edit`] 只保证"全部编辑都经
    /// history 网关"时两者一致（生产里的写入面就是 E06/E07 网关，见 editor bin 的
    /// `request_document_change`）。选补丁这个口径的理由：撤销顺手吃掉别处的改动，
    /// 比留下它更难被发现。
    #[test]
    fn patch_undo_leaves_fields_it_never_touched_alone() {
        let id = SceneEntityId(1);
        let seed = {
            let mut document = entity_component(id, "x", "0");
            document
                .scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("sidecar".into(), "0".into());
            document
        };

        // 补丁形态。
        let mut document = seed.clone();
        let mut history = CommandHistory::new();
        history
            .execute(&mut document, "x=1", CommandKind::Property, |d| {
                d.scene
                    .entities
                    .get_mut(&id)
                    .unwrap()
                    .components
                    .insert("x".into(), "1".into());
                Ok::<_, ()>(())
            })
            .unwrap();
        document
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("sidecar".into(), "带外写入".into());
        history.undo(&mut document).unwrap();
        let entity = &document.scene.entities[&id];
        assert_eq!(entity.components["x"], "0", "碰过的字段要回去");
        assert_eq!(
            entity.components["sidecar"], "带外写入",
            "没碰过的字段必须存活——补丁形态的撤销是外科式的"
        );

        // 快照形态：同一串动作，带外写入被整份还原抹掉。
        let mut document = seed.clone();
        let mut history = CommandHistory::without_patch_compression();
        history
            .execute(&mut document, "x=1", CommandKind::Property, |d| {
                d.scene
                    .entities
                    .get_mut(&id)
                    .unwrap()
                    .components
                    .insert("x".into(), "1".into());
                Ok::<_, ()>(())
            })
            .unwrap();
        document
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert("sidecar".into(), "带外写入".into());
        history.undo(&mut document).unwrap();
        assert_eq!(
            document.scene.entities[&id].components["sidecar"], "0",
            "快照形态连带外写入一起还原（这条口径差异由上面的补丁形态认领）"
        );
    }

    /// 内存：长编辑会话里补丁形态只保留改动本身。
    ///
    /// 断言的是"数量级"而不是具体字节数（具体数字由 `docs/25` §24 与 perf 基线
    /// `patch_history_production` 报，那里才有真场景夹具）。
    #[test]
    fn a_field_edit_session_retains_an_order_of_magnitude_less_than_snapshots() {
        fn big_document() -> SceneDocument {
            let mut document = SceneDocument::new("big");
            for index in 0..80u64 {
                let entity = SceneEntityId(index);
                document.scene.add_entity(entity, "E", None).unwrap();
                document
                    .scene
                    .entities
                    .get_mut(&entity)
                    .unwrap()
                    .components
                    .insert("x".into(), index.to_string());
            }
            document.mark_clean();
            document
        }
        fn write_one_x(history: &mut CommandHistory, document: &mut SceneDocument, value: u32) {
            let target = SceneEntityId(7);
            history
                .execute(document, "x", CommandKind::Property, |d| {
                    d.scene
                        .entities
                        .get_mut(&target)
                        .unwrap()
                        .components
                        .insert("x".into(), value.to_string());
                    d.mark_dirty();
                    Ok::<_, ()>(())
                })
                .unwrap();
        }

        let mut patch_document = big_document();
        let mut snapshot_document = big_document();
        let mut patch_history = CommandHistory::new();
        let mut snapshot_history = CommandHistory::without_patch_compression();
        for value in 0..20 {
            write_one_x(&mut patch_history, &mut patch_document, value);
            write_one_x(&mut snapshot_history, &mut snapshot_document, value);
        }
        assert_eq!(
            patch_document.scene.to_text(),
            snapshot_document.scene.to_text()
        );
        assert_eq!(
            patch_history.undo_len(),
            20,
            "Generic 不相邻合并，20 步各占一条"
        );
        assert_eq!(patch_history.storage_counts(), (20, 0));
        let patch_bytes = patch_history.retained_bytes_estimate();
        let snapshot_bytes = snapshot_history.retained_bytes_estimate();
        assert!(
            patch_bytes * 8 < snapshot_bytes,
            "80 实体、20 步单字段编辑：补丁 {patch_bytes} B 应当比快照 {snapshot_bytes} B 小一个数量级"
        );
    }
}
