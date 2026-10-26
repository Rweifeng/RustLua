//! 预制体 in-context 编辑（M79 P1-4，对标 Unity 的 Prefab Mode）。
//!
//! 进入时把当前编辑文档换成源 `.prefab`，而**发起它的那份场景文档**（连同未保存改动与
//! 它自己的撤销栈）整份暂存在 [`PrefabStage`] 里；退出时原样放回，并在**一条 Undo** 里
//! 用新源重建场景中的全部同源实例。
//!
//! 三条口径决定了这里为什么长成这样：
//!
//! 1. **「打开源预制体」早就有了，但它会替换当前文档**。于是想临时改一下预制体，就得先
//!    回答「场景要不要保存」这个问题（E06 脏文档闸门），改完还得自己找到那份场景重新打开。
//!    in-context 要的是"不打断"，所以它不借道 [`PendingChange::OpenResource`]，而是自己
//!    把文档换成预制体、把场景收进口袋。
//! 2. **阶段里改的是源，不是实例**。实例上的覆盖不需要额外记账，但 diff 要拿对基线：
//!    退出时按**进入时那份源**（[`PrefabStage::baseline`]）算出"这个实例本来就和源
//!    不一样的地方"，再把它们叠到**新源**上。
//!    拿新源当基线是错的：刚被改掉的那个字段，实例还留着老值，于是它会被当成"实例自己的
//!    覆盖"重新按上去——源改动被实例的老值顶回，等于同步失败（回归门
//!    `stage_save_and_return_syncs_every_instance_with_one_undo` 第一次跑就是这么红的）。
//!    用对基线后，"在阶段里改 tint、返回后每个实例仍保持自己的位置与名字"才成立，
//!    且被改掉的字段会自动从覆盖列表里消失——与逐字段「应用」共用同一条落盘+重建路径。
//! 3. **退出必须原子**。先用新源把每个实例的重建**预演**一遍，全部可行才落盘并返回；
//!    否则会出现「预制体文件已经改了、场景实例还没跟上」的半状态（在阶段里删掉某个实例
//!    仍引用的子节点，正是这条路径）。预演失败时留在阶段里，用户改得回来。

use super::prefab_rebuild::{
    PrefabWriteRequest, collect_prefab_rebuild_state, collect_prefab_rebuild_state_with_baseline,
    prefab_root_ids,
};
use super::*;
use crate::SceneViewMode;

/// 一次 in-context 编辑的暂存态：要回的那份场景，以及回到它身边时得一并还原的东西。
///
/// 之所以要暂存撤销栈：`self.history` 是**当前编辑文档**的历史。阶段里它属于预制体
/// （阶段内的每步编辑都只影响阶段文档），若返回时不清不楚地共用一条栈，用户回到场景
/// 按 Ctrl+Z 就会把阶段里的改动"撤"进场景文档，或直接丢掉场景原本的历史。
#[derive(Clone, Debug)]
pub(crate) struct PrefabStage {
    /// 源预制体的工程内相对路径（与实例元数据 `prefab.source` 逐字一致）。
    pub(crate) source: String,
    /// 进入时正在编辑的场景文档（**含未保存改动**，退出时原样放回）。
    pub(crate) context: SceneDocument,
    /// 场景文档自己的撤销栈：进入时摘下来放这儿，退出时放回去。
    pub(crate) context_history: editor::CommandHistory,
    /// 进入时的选区：返回后恢复（阶段里选的是预制体的实体，与场景 ID 不同一）。
    pub(crate) context_selection: editor::SelectionModel,
    /// 进入时的 Scene 视图模式：阶段里强制预制体模式，返回后还原。
    pub(crate) previous_view_mode: SceneViewMode,
    /// 进入时那个实例的根：写回并重建后按它重新 diff 覆盖、重新选中。
    pub(crate) instance_root: SceneEntityId,
    /// 进入时磁盘上的那份源：退出时**覆盖 diff 的基线**（口径 2）。
    ///
    /// 存的是副本而不是"回读一次文件"：阶段里 `save` 会先把这份文件覆写掉，
    /// 到时磁盘上已经是新源，回读拿到的是错的基线。
    pub(crate) baseline: Prefab,
}

impl EditorApp {
    /// 当前是否处于 in-context 预制体编辑（面板画横幅、命令层判前置都读这个）。
    pub(crate) fn prefab_stage(&self) -> Option<&PrefabStage> {
        self.prefab_stage.as_ref()
    }

    /// 阶段文档里有没有尚未保存的改动：换文档保护用它判断"这一步有没有东西可丢"。
    /// 不在阶段里时恒为 `false`。
    pub(crate) fn prefab_stage_unsaved(&self) -> bool {
        self.prefab_stage.is_some()
            && self
                .document
                .current_scene
                .as_ref()
                .is_some_and(SceneDocument::is_dirty)
    }

    /// 横幅要显示的事实：源路径 + 返回时会被同步更新的实例数量。
    ///
    /// 数量取自**暂存的场景文档**（阶段期间它是冻结的，所以这个数字在整个阶段里稳定），
    /// 让用户在进入的那一刻就知道"改这一处会动到场景里的几个东西"。
    pub(crate) fn prefab_stage_banner(&self) -> Option<(&str, usize)> {
        let stage = self.prefab_stage.as_ref()?;
        let instances = prefab_root_ids(&stage.context.scene, &stage.source).len();
        Some((stage.source.as_str(), instances))
    }

    /// 进入 in-context：把当前文档换成 `entity` 的源预制体，场景文档整份暂存。
    ///
    /// 拒绝路径按"最具体"的顺序给原因（播放中 → 已在阶段里 → 当前文档本身就是预制体 →
    /// 不是根实例 → 源加载失败 → 嵌套 → 退出时会失败的结构），每条都能被断言，
    /// 而不是只在状态栏留一句"没成功"。
    pub(crate) fn enter_prefab_stage(&mut self, entity: SceneEntityId) -> CommandResult {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        if self.prefab_stage.is_some() {
            self.status = "已在预制体编辑上下文中，请先返回场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        // 资源面板直接双击 `.prefab` 打开的就是预制体文档：那份文档本身就是源，
        // 再"进入上下文"只会多套一层口袋，改完也没有场景可回。
        if self
            .current_scene()
            .is_some_and(SceneDocument::is_prefab_document)
        {
            self.status = "当前编辑的已经是预制体文档，直接修改后用 Ctrl+S 保存即可".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        if !self.is_prefab_root(entity) {
            self.status = "只有预制体根实例能进入 in-context 编辑".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        let Some((prefab_source, _)) = self.prefab_instance_source(entity) else {
            self.status = "当前实体没有预制体来源".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        let prefab = match Prefab::load(&path) {
            Ok(prefab) => prefab,
            Err(error) => {
                self.status = format!("加载源预制体失败：{error}");
                return CommandResult::Rejected(self.status.clone());
            }
        };
        let nested = prefab.nested_sources();
        if !nested.is_empty() {
            self.status = format!(
                "源预制体里还引用了别的预制体（{}），in-context 编辑暂不支持嵌套预制体",
                nested.join("、")
            );
            return CommandResult::Rejected(self.status.clone());
        }
        // 进入前**预演**一次重建：子树里混进非同源节点（手工往实例下挂过别的预制体）时
        // 退出必然失败。与其让人进去再出来时报错，不如现在就说清楚。
        let Some(scene) = self.current_scene() else {
            self.status = "没有打开的场景".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        if let Err(error) =
            collect_prefab_rebuild_state(&scene.scene, entity, &prefab_source, &prefab)
        {
            self.status = format!("该实例无法安全进入 in-context 编辑：{error}");
            return CommandResult::Rejected(self.status.clone());
        }
        let instances = prefab_root_ids(&scene.scene, &prefab_source).len();
        let previous_view_mode = self.scene_view_mode;

        // E07：若正处于活动事务（拖动还没提交），先把这一步收进场景历史成为一条，
        // 随后整条栈随场景文档一起暂存——返回场景后它还在，Undo 能一路撤到这一步之前。
        if self.edit_transaction.is_active() && self.history.is_transaction_active() {
            if let Some(document) = self.document.current_scene.as_ref() {
                self.history.finish_transaction(document);
            }
            self.edit_transaction.active = false;
            self.edit_transaction.consumed = true;
        }
        let stage_document = match SceneDocument::open(&path) {
            Ok(document) => document,
            Err(error) => {
                self.status = format!("打开源预制体失败：{error}");
                return CommandResult::Rejected(self.status.clone());
            }
        };
        let Some(context) = self.document.current_scene.take() else {
            // 上面已经确认过有场景；这条兜底只是不让"没有当前文档"变成隐式状态
            // （此时刚 open 出来的预制体文档随作用域结束被丢弃，什么都没改）。
            self.status = "进入预制体编辑失败：没有可暂存的场景文档".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let context_history = self.history.history().clone();
        let context_selection = self.selection.clone();
        // 阶段的历史从零开始：阶段内的 Ctrl+Z 只应撤到阶段内的步骤，返回场景后
        // 这条栈整条作废（与「打开另一份文档」的既有口径一致）。
        *self.history.history_mut() = editor::CommandHistory::new();
        self.set_current_scene_document(stage_document);
        self.autosave.reset();
        self.scene_view_mode = SceneViewMode::Prefab;
        self.revalidate_scene_tool_for_view_mode();
        self.prefab_stage = Some(PrefabStage {
            source: prefab_source.clone(),
            context,
            context_history,
            context_selection,
            previous_view_mode,
            instance_root: entity,
            // 一次克隆换"基线不会被后续写盘覆盖"——阶段是点击触发的路径，不在每帧热路上。
            baseline: prefab.clone(),
        });
        self.on_document_changed(DocumentChange::Opened);
        // 进去就选中预制体根：Inspector 不必让用户再点一次层级树才有内容。
        let stage_root = prefab.root;
        self.selection
            .set_single(stage_root, editor::SelectionSource::Command);
        self.sync_inspector(stage_root);
        self.status =
            format!("正在编辑预制体：{prefab_source}（返回时会同步场景里的 {instances} 个实例）");
        CommandResult::Applied
    }

    /// 退出 in-context。`save = false` 丢弃阶段改动、源文件一个字节都不动；
    /// `save = true` 写回源预制体，并在场景里用一条 Undo 重建全部同源实例。
    pub(crate) fn close_prefab_stage(&mut self, save: bool) -> CommandResult {
        let Some(stage) = self.prefab_stage.take() else {
            self.status = "当前不在预制体编辑上下文中".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        if !save {
            self.restore_prefab_stage_context(&stage);
            self.status = format!("已退出预制体编辑，源文件未改动：{}", stage.source);
            return CommandResult::Applied;
        }
        // 1) 拿到"将要落盘的那份新源"——与 Ctrl+S 走同一条转换（`prefab_asset`），
        //    否则「保存并返回」与直接保存会写出两份形状不同的资产。
        let prefab = match self
            .document
            .current_scene
            .as_ref()
            .map(SceneDocument::prefab_asset)
        {
            None => {
                self.restore_prefab_stage_context(&stage);
                self.status = "退出预制体编辑失败：阶段文档已丢失".to_owned();
                return CommandResult::Rejected(self.status.clone());
            }
            Some(Ok(prefab)) => prefab,
            Some(Err(error)) => {
                // 留在阶段里：多出一个根实体是这一条最常见的原因，用户改得回来。
                self.prefab_stage = Some(stage);
                self.status = format!("已阻止退出：{error}");
                return CommandResult::Rejected(self.status.clone());
            }
        };
        let path = strip_verbatim_prefix(&self.project.root).join(stage.source.as_str());
        // 2) 预演：覆盖按**进入时的基线**算、结构按**新源**校验，把退出时一定会失败的形状
        //    （阶段里删掉了实例仍引用的子节点）挡在落盘之前。
        for root in prefab_root_ids(&stage.context.scene, &stage.source) {
            if let Err(error) = collect_prefab_rebuild_state_with_baseline(
                &stage.context.scene,
                root,
                &stage.source,
                &prefab,
                Some(&stage.baseline),
            ) {
                self.prefab_stage = Some(stage);
                self.status = format!(
                    "已阻止退出：场景里的实例无法按新源重建（{error}）。可改用「放弃并返回」，或先在阶段里把结构补回来。"
                );
                return CommandResult::Rejected(self.status.clone());
            }
        }
        // 3) 回到场景，再用既有网关落盘 + 一条 Undo 重建全部实例（与「写回源并更新实例」
        //    同一条路径，覆盖按新源重新 diff）。
        self.restore_prefab_stage_context(&stage);
        let new_selected_root = std::rc::Rc::new(std::cell::Cell::new(None));
        let selected_result = new_selected_root.clone();
        let source = stage.source.clone();
        let instance_root = stage.instance_root;
        let Some(scene) = self.document.current_scene.as_mut() else {
            self.status = "退出预制体编辑失败：场景文档已丢失".to_owned();
            return CommandResult::Rejected(self.status.clone());
        };
        let result = Self::write_prefab_and_rebuild_instances(
            &mut self.history,
            scene,
            PrefabWriteRequest {
                source: &source,
                path: &path,
                prefab,
                selected: instance_root,
                label: "退出预制体编辑并更新实例",
                baseline: Some(&stage.baseline),
            },
            &new_selected_root,
        );
        match result {
            Ok(()) => {
                self.refresh_resources();
                if let Some(new_root) = selected_result.get() {
                    self.selection
                        .set_single(new_root, editor::SelectionSource::Inspector);
                }
                self.on_document_changed(DocumentChange::PrefabApply);
                self.status = format!("已写回预制体源并同步场景实例：{source}");
                CommandResult::Applied
            }
            Err(error) => {
                // 预演已把可预见的失败挡在落盘之前；真走到这一条说明文件写了但实例没重建，
                // 于是明确给出下一步：选中实例用「重新应用源预制体」按磁盘上的新源重建。
                self.status = format!(
                    "预制体已写回，但场景实例更新失败：{error}。请在场景里选中实例后用「重新应用源预制体」重试。"
                );
                self.set_scene_view_diagnostic(self.status.clone());
                CommandResult::Rejected(self.status.clone())
            }
        }
    }

    /// 把「回到场景」一次做完：文档、撤销栈、视图模式、选区、面板收敛。
    ///
    /// 三条退出路径（放弃 / 保存 / 保存的兜底失败）共用它，避免哪一条漏了缓存失效。
    /// 代价：这里克隆了一次场景文档（阶段是点击触发的路径，不在每帧热路上；
    /// `sync_inspector` 早就用同样形状的克隆）。
    fn restore_prefab_stage_context(&mut self, stage: &PrefabStage) {
        self.set_current_scene_document(stage.context.clone());
        *self.history.history_mut() = stage.context_history.clone();
        self.autosave.reset();
        self.scene_view_mode = stage.previous_view_mode;
        self.revalidate_scene_tool_for_view_mode();
        self.on_document_changed(DocumentChange::Opened);
        self.selection = stage.context_selection.clone();
        let active = self.document.current_scene.clone();
        self.selection.retain_valid(|id| {
            active
                .as_ref()
                .is_some_and(|doc| doc.scene.entities.contains_key(&id))
        });
        if let Some(selected) = self.selection.primary() {
            self.sync_inspector(selected);
        }
    }
}
