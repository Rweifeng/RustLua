//! 跨面板**全局 Undo**（M79 P1-2，docs/25 §22）。
//!
//! 三条口径，都是刻意划的线：
//!
//! 1. **每个面板保留自己的撤销栈，全局只统一"下一个该问谁"**。各栈怎么撤只有它自己说得清：
//!    场景栈要先收敛 Inspector 挂起编辑、退出活动事务（`undo_with_transaction_guard`）；
//!    UI 工作室按**文件字节**判陈旧（外部改过盘就不敢撤）；材质/动画会话有各自的合并与
//!    "回读校验通过才算一步"规则；图有 64 步上限与手势合并。把它们塞进同一条栈，等于给
//!    五种语义取并集——那会做错每一件。账本只记**顺序**，撤的动作交回归属自己。
//! 2. **顺序靠"编辑签名"推导，不靠调用点自觉**。签名 = `(累计提交次数, undo 栈 + redo 栈深度)`：
//!    第一项只在真正新增一步时变大（撤销/重做只挪栈、合并中的连续拖动不产生新步，都不计数），
//!    所以"这个归属刚刚又提交了"不会被漏看；第二项在换文档时随栈清空回落到 0 → 从账本上摘掉。
//!    只用深度会在两个常见序列上出错：撤销一步后紧接着重新编辑会回到同一个深度，栈到达上限后
//!    深度再也不变。这样**没有"忘记记账"这条路**（生产侧十几处提交点只需在各自的压栈处 +1，
//!    不会因为新增提交点而漏撤）。签名由 [`EditorApp::poll_undo_owners`] **每帧轮询**一次
//!    （`draw_frame` 开头），消费按键时再刷一遍——轮询越密，两次轮询之间就越可能只有一个归属
//!    变化，"最近提交"的判定越接近真实顺序。代价要说清楚：**同一帧内**两个面板都提交时先后
//!    无从判断，此时按声明顺序兜底（场景优先）——一次点击只能碰一个面板，真撞上了丢的是顺序，
//!    不是历史。
//! 3. **重做优先回到刚才撤销掉的那个归属**（Unity 的 Redo 是上一次 Undo 的逆），
//!    它没有可重做的了才按账本顺序找下一个。

use crate::EditorApp;
use editor::CommandResult;

/// 一个归属的编辑签名：`(累计提交次数, undo 栈 + redo 栈深度)`。口径见模块头第 2 条。
type Signature = (u64, usize);

/// 有独立撤销栈的归属。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub(crate) enum UndoOwner {
    /// 场景文档（`CommandService`/`CommandHistory`）。
    Scene,
    /// UI 工作室的「接受」栈（按文件字节回滚 `.ui.json`）。
    UiStudio,
    /// 可视化脚图画布（`GraphDocument`）。
    Graph,
    /// 材质面板会话（`MaterialEditSession`）。
    Material,
    /// 时间线面板会话（`AnimEditSession`）。
    Timeline,
}

impl UndoOwner {
    /// 声明顺序 = 签名同时变化时的兜底优先级（场景在前：它是默认工作区）。
    ///
    /// 数组长度（签名表、快照）都由 `ALL.len()` 推出来：**新增一个归属时不必再手数一遍**，
    /// 也只有把这里和 [`UndoOwner::index`] 一起改漏时才会编译不过，而不是静默错位。
    pub(crate) const ALL: [Self; 5] = [
        Self::Scene,
        Self::UiStudio,
        Self::Graph,
        Self::Material,
        Self::Timeline,
    ];

    fn index(self) -> usize {
        match self {
            Self::Scene => 0,
            Self::UiStudio => 1,
            Self::Graph => 2,
            Self::Material => 3,
            Self::Timeline => 4,
        }
    }

    /// 状态栏里"撤的是谁"的说法。
    fn label(self) -> &'static str {
        match self {
            Self::Scene => "场景",
            Self::UiStudio => "UI 工作室",
            Self::Graph => "可视化脚本",
            Self::Material => "材质",
            Self::Timeline => "动画",
        }
    }
}

/// 全局 Undo 账本：只记顺序，不碰任何文档。
#[derive(Debug, Default)]
pub(crate) struct UndoLedger {
    /// 上次见到各归属时的签名（索引 = `UndoOwner::index`）。
    signatures: [Signature; UndoOwner::ALL.len()],
    /// 归属的提交顺序，**尾部 = 最近一次提交**。
    order: Vec<UndoOwner>,
    /// 上一次撤销落在谁身上（重做优先回给它）。
    last_undone: Option<UndoOwner>,
}

impl UndoLedger {
    /// 用当前签名刷新账本，返回「先问谁」的候选序列（最近提交在前）。
    ///
    /// 幂等：签名没变的归属原地不动，所以每帧轮询与按键时再刷一次都只是几次整数比较。
    ///
    /// 快照按**声明顺序**（场景在前）传入，这里倒着过一遍：于是同一帧内两个面板同时
    /// 提交时，越靠前的归属越晚入队、也就越先被问到——兜底优先级是「场景 > UI 工作室 >
    /// 图 > 材质 > 动画」。
    fn route(&mut self, signatures: &[(UndoOwner, Signature)]) -> Vec<UndoOwner> {
        for (owner, signature) in signatures.iter().copied().rev() {
            let slot = &mut self.signatures[owner.index()];
            let previous = *slot;
            *slot = signature;
            if signature == previous {
                continue;
            }
            self.forget(owner);
            // 深度归零 = 这份历史已经不在了（换文档/换素材/关面板），不该再被问到。
            if signature.1 > 0 {
                self.order.push(owner);
            }
        }
        self.order.iter().rev().copied().collect()
    }

    /// 把某归属从顺序里摘掉（栈被清空、或签名回落时）。
    fn forget(&mut self, owner: UndoOwner) {
        self.order.retain(|candidate| *candidate != owner);
    }

    /// 换工程/换文档：整本账作废（各栈随后也会被清，这里只保证不留下上一份文档的归属）。
    fn clear(&mut self) {
        self.signatures = [(0, 0); UndoOwner::ALL.len()];
        self.order.clear();
        self.last_undone = None;
    }

    fn note_undone(&mut self, owner: UndoOwner) {
        self.last_undone = Some(owner);
    }

    fn take_last_undone(&mut self) -> Option<UndoOwner> {
        let owner = self.last_undone;
        self.last_undone = None;
        owner
    }
}

impl EditorApp {
    /// 五个归属当前的编辑签名（声明顺序 = 兜底优先级，见模块头第 2 条）。
    fn undo_signatures(&mut self) -> [(UndoOwner, Signature); UndoOwner::ALL.len()] {
        let studio = self
            .ui_studio_panel
            .studio
            .as_ref()
            .map_or((0, 0), editor::ui_studio::UiStudio::undo_signature);
        [
            (UndoOwner::Scene, self.history.undo_signature()),
            (UndoOwner::UiStudio, studio),
            (UndoOwner::Graph, self.graph.undo_signature()),
            (UndoOwner::Material, self.material.undo_signature()),
            (UndoOwner::Timeline, self.timeline.undo_signature()),
        ]
    }

    /// 每帧轮询一次：谁的历史深度变了，谁就是"最近提交的那个"。
    ///
    /// 调用点见 `main.rs` 的 `draw_frame` 开头。签名没变时这里只做几次整数比较，
    /// 不分配也不挪历史；生产侧的提交点因此**一行都不用改**（口径见模块头第 2 条）。
    pub(crate) fn poll_undo_owners(&mut self) {
        let signatures = self.undo_signatures();
        self.undo_ledger.route(&signatures);
    }

    fn owner_can_undo(&mut self, owner: UndoOwner) -> bool {
        match owner {
            UndoOwner::Scene => self.history.can_undo(),
            UndoOwner::UiStudio => self
                .ui_studio_panel
                .studio
                .as_ref()
                .is_some_and(editor::ui_studio::UiStudio::can_undo),
            UndoOwner::Graph => self.graph.undo_can_undo(),
            UndoOwner::Material => self.material.undo_can_undo(),
            UndoOwner::Timeline => self.timeline.undo_can_undo(),
        }
    }

    fn owner_can_redo(&mut self, owner: UndoOwner) -> bool {
        match owner {
            UndoOwner::Scene => self.history.can_redo(),
            UndoOwner::UiStudio => self
                .ui_studio_panel
                .studio
                .as_ref()
                .is_some_and(editor::ui_studio::UiStudio::can_redo),
            UndoOwner::Graph => self.graph.undo_can_redo(),
            UndoOwner::Material => self.material.undo_can_redo(),
            UndoOwner::Timeline => self.timeline.undo_can_redo(),
        }
    }

    /// 撤某归属的一步。动作交回归属自己（口径见模块头第 1 条）。
    fn owner_undo(&mut self, owner: UndoOwner) {
        match owner {
            UndoOwner::Scene => {
                // E07/E08 网关：先取消活动事务并收敛挂起编辑，再撤销。
                self.undo_with_transaction_guard();
            }
            UndoOwner::UiStudio => {
                if let Some(studio) = self.ui_studio_panel.studio.as_mut() {
                    match studio.undo() {
                        Ok(true) => {
                            let revision = studio.revision();
                            self.status = format!("已撤销 UI 工作室接受（版本 {revision}）");
                            self.ui_studio_panel.canvas_textures.clear();
                        }
                        Ok(false) => self.status = "UI 工作室没有可撤销的接受".to_owned(),
                        Err(error) => self.status = format!("撤销 UI 工作室失败：{error}"),
                    }
                }
            }
            UndoOwner::Graph => {
                if self.graph.undo_step() {
                    self.status = format!("已撤销：{}（图编辑）", owner.label());
                }
            }
            UndoOwner::Material | UndoOwner::Timeline => {
                let moved = match owner {
                    UndoOwner::Material => self.material.undo_step(),
                    _ => self.timeline.undo_step(),
                };
                if moved {
                    self.status = format!("已撤销：{}（未保存）", owner.label());
                }
            }
        }
    }

    /// 重做某归属的一步。
    fn owner_redo(&mut self, owner: UndoOwner) {
        match owner {
            UndoOwner::Scene => {
                self.finish_inspector_edits_if_dirty();
                self.redo();
            }
            UndoOwner::UiStudio => {
                if let Some(studio) = self.ui_studio_panel.studio.as_mut() {
                    match studio.redo() {
                        Ok(true) => {
                            let revision = studio.revision();
                            self.status = format!("已重做 UI 工作室接受（版本 {revision}）");
                            self.ui_studio_panel.canvas_textures.clear();
                        }
                        Ok(false) => self.status = "UI 工作室没有可重做的接受".to_owned(),
                        Err(error) => self.status = format!("重做 UI 工作室失败：{error}"),
                    }
                }
            }
            UndoOwner::Graph => {
                if self.graph.redo_step() {
                    self.status = format!("已重做：{}（图编辑）", owner.label());
                }
            }
            UndoOwner::Material | UndoOwner::Timeline => {
                let moved = match owner {
                    UndoOwner::Material => self.material.redo_step(),
                    _ => self.timeline.redo_step(),
                };
                if moved {
                    self.status = format!("已重做：{}（未保存）", owner.label());
                }
            }
        }
    }

    /// 全局 Undo：按账本顺序问下去，第一个还有东西可撤的归属吃掉这次按键。
    pub(crate) fn undo_global(&mut self) -> CommandResult {
        let signatures = self.undo_signatures();
        // 先拿到候选列表再逐个问：`route` 借用了 `self.undo_ledger`，而 `owner_can_undo`
        // 要可变借用整个 `self`，两件事不能写在同一条链上。
        let candidates = self.undo_ledger.route(&signatures);
        let Some(owner) = candidates
            .into_iter()
            .find(|owner| self.owner_can_undo(*owner))
        else {
            self.status = "没有可撤销的操作".to_owned();
            return CommandResult::NoOp;
        };
        self.undo_ledger.note_undone(owner);
        self.owner_undo(owner);
        CommandResult::Applied
    }

    /// 全局 Redo：先试刚才撤掉的那个归属，再按账本顺序找。
    pub(crate) fn redo_global(&mut self) -> CommandResult {
        let signatures = self.undo_signatures();
        let mut candidates = self.undo_ledger.route(&signatures);
        if let Some(last_undone) = self.undo_ledger.take_last_undone() {
            candidates.retain(|owner| *owner != last_undone);
            candidates.insert(0, last_undone);
        }
        let Some(owner) = candidates
            .into_iter()
            .find(|owner| self.owner_can_redo(*owner))
        else {
            self.status = "没有可重做的操作".to_owned();
            return CommandResult::NoOp;
        };
        self.owner_redo(owner);
        CommandResult::Applied
    }

    /// 换工程/换文档时作废账本（调用点见 `import.rs` 的换工程路径）。
    pub(crate) fn clear_undo_ledger(&mut self) {
        self.undo_ledger.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(ledger: &mut UndoLedger, signatures: &[(UndoOwner, Signature)]) -> Vec<UndoOwner> {
        ledger.route(signatures)
    }

    /// 五个归属的签名快照，**按声明顺序**给出（与 `EditorApp::undo_signatures` 一致）。
    /// 每一项是 `(累计提交次数, 深度)`；`(0, 0)` = 这个归属当前没有历史。
    fn snap(
        scene: Signature,
        studio: Signature,
        graph: Signature,
        material: Signature,
        timeline: Signature,
    ) -> [(UndoOwner, Signature); 5] {
        [
            (UndoOwner::Scene, scene),
            (UndoOwner::UiStudio, studio),
            (UndoOwner::Graph, graph),
            (UndoOwner::Material, material),
            (UndoOwner::Timeline, timeline),
        ]
    }

    /// 最近提交的那个先被问到（这就是"全局单栈"的全部含义）。
    #[test]
    fn the_most_recent_owner_is_asked_first() {
        let mut ledger = UndoLedger::default();
        let first = route(&mut ledger, &snap((1, 1), (0, 0), (0, 0), (0, 0), (0, 0)));
        assert_eq!(first, vec![UndoOwner::Scene]);
        // 材质随后提交了一步：下一次撤销先问材质。
        let second = route(&mut ledger, &snap((1, 1), (0, 0), (0, 0), (1, 1), (0, 0)));
        assert_eq!(second, vec![UndoOwner::Material, UndoOwner::Scene]);
    }

    /// 撤销/重做只把步子在两条栈之间挪动：提交次数不变、深度也不变 → 顺序不得被
    /// "撤销"这个动作改写。否则撤一步就会把归属重新顶到队尾，第二次撤销又去问同一个面板。
    #[test]
    fn undoing_does_not_reorder_the_ledger() {
        let mut ledger = UndoLedger::default();
        // 帧 1：场景提交过 4 步。
        assert_eq!(
            route(&mut ledger, &snap((4, 4), (0, 0), (0, 0), (0, 0), (0, 0))),
            vec![UndoOwner::Scene]
        );
        // 帧 2：材质提交过 2 步（undo 2 + redo 0 = 深度 2）—— 它现在是最近归属。
        let staged = snap((4, 4), (0, 0), (0, 0), (2, 2), (0, 0));
        assert_eq!(
            route(&mut ledger, &staged),
            vec![UndoOwner::Material, UndoOwner::Scene]
        );
        // 撤掉材质一步：undo 1 + redo 1 仍是深度 2、计数仍是 2 ⇒ 签名没动 ⇒ 顺序不变。
        assert_eq!(
            route(&mut ledger, &staged),
            vec![UndoOwner::Material, UndoOwner::Scene]
        );
    }

    /// 反面对照（也是引入提交计数的原因）：**撤一步之后紧接着再编辑**，深度会回到
    /// 同一个数。只看深度的签名在这个序列上完全不动，用户的这一步就会被别的归属挤掉；
    /// 提交次数让它仍然是"最近提交"。
    #[test]
    fn re_editing_after_an_undo_regains_priority() {
        let mut ledger = UndoLedger::default();
        // 帧 1：场景一步。
        route(&mut ledger, &snap((1, 1), (0, 0), (0, 0), (0, 0), (0, 0)));
        // 帧 2：材质两步，成为最近归属。
        assert_eq!(
            route(&mut ledger, &snap((1, 1), (0, 0), (0, 0), (2, 2), (0, 0))),
            vec![UndoOwner::Material, UndoOwner::Scene]
        );
        // 撤掉材质的一步（签名不动），期间场景又提交一步 ⇒ 最近归属换回场景。
        assert_eq!(
            route(&mut ledger, &snap((2, 2), (0, 0), (0, 0), (2, 2), (0, 0))),
            vec![UndoOwner::Scene, UndoOwner::Material]
        );
        // 关键一步：材质重新编辑——redo 被清、undo +1，**深度回到 2 没变**，只有计数在动。
        assert_eq!(
            route(&mut ledger, &snap((2, 2), (0, 0), (0, 0), (3, 2), (0, 0))),
            vec![UndoOwner::Material, UndoOwner::Scene],
            "撤销后重新编辑的归属必须重新成为最近提交"
        );
    }

    /// 反面对照：深度回落（换文档把栈清了）必须把归属摘掉，
    /// 否则账本会一直"先问一个已经没有历史的面板"，把该轮到场景的那步吃掉。
    #[test]
    fn a_cleared_stack_leaves_the_ledger() {
        let mut ledger = UndoLedger::default();
        route(&mut ledger, &snap((2, 2), (0, 0), (1, 1), (0, 0), (0, 0)));
        let after_switch = route(&mut ledger, &snap((0, 0), (0, 0), (0, 0), (0, 0), (0, 0)));
        assert!(
            after_switch.is_empty(),
            "清空后不该还有归属在账上：{after_switch:?}"
        );
        // 重新提交才回到账本上（计数是接着涨的，深度负责把它请回来）。
        let back = route(&mut ledger, &snap((3, 1), (0, 0), (0, 0), (0, 0), (0, 0)));
        assert_eq!(back, vec![UndoOwner::Scene]);
    }

    /// 同一帧里两个面板都提交（一次轮询看到两个签名同时变化）时按声明顺序兜底：场景优先。
    /// 这条不是"正确行为"，是**已知代价**的显式登记——见模块头第 2 条。
    #[test]
    fn simultaneous_pushes_fall_back_to_the_declared_order() {
        let mut ledger = UndoLedger::default();
        let candidates = route(&mut ledger, &snap((1, 1), (0, 0), (0, 0), (1, 1), (0, 0)));
        assert_eq!(candidates, vec![UndoOwner::Scene, UndoOwner::Material]);
        // 反面对照：分两批轮询（真实的"隔帧编辑"）时顺序就恢复了——先提交的场景被顶到后面。
        let mut staged = UndoLedger::default();
        staged.route(&snap((1, 1), (0, 0), (0, 0), (0, 0), (0, 0)));
        assert_eq!(
            staged.route(&snap((1, 1), (0, 0), (0, 0), (1, 1), (0, 0))),
            vec![UndoOwner::Material, UndoOwner::Scene]
        );
    }

    /// `clear` 是换工程路径上的兜底：签名数组、顺序、last_undone 一起作废。
    #[test]
    fn clearing_forgets_the_previous_project() {
        let mut ledger = UndoLedger::default();
        route(&mut ledger, &snap((2, 2), (0, 0), (0, 0), (0, 0), (1, 1)));
        ledger.note_undone(UndoOwner::Timeline);
        ledger.clear();
        assert!(ledger.order.is_empty());
        assert_eq!(ledger.signatures, [(0, 0); 5]);
        assert_eq!(ledger.take_last_undone(), None);
    }
}
