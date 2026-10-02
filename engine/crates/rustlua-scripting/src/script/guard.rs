//! 脚本错误的处置策略与回调错误预算。
//!
//! 背景：宿主此前把 Lua 回调的 `Err` 当控制流用——`update()` 报错 ⇒ 系统返回
//! `SystemError` ⇒ 调度器放弃本步剩余系统 ⇒ `game.exe` 终止进程（`game_app.rs`）。
//! 于是"某个 Lua 分支写错一行"等于"玩家这一局到此为止"，而且物理、粒子、场景切换
//! 这些与脚本无关的系统也一起被跳过。本模块把脚本错误变成**数据**：记进结构化诊断、
//! 计数，再由策略决定是否继续跑（见 [`ScriptErrorPolicy`]）。
//!
//! 预算状态装在 `RefCell` 里、而不是把 `call_update` 改成 `&mut self`：脚本回调入口有
//! 约 200 处调用点（测试与示例），签名不变换来的收益远大于语义上的整洁。

use std::collections::{BTreeSet, HashMap};

use rustlua_ecs::entity::EntityId;

/// 连续错误达到该值即停用该回调。60 步 ≈ 1 秒 @60Hz。
///
/// 取"1 秒"而不是"3 帧"：只在特定输入下才出现的错误（读表越界、nil 字段）应当有机会
/// 自愈——成功一次即清零，停用只针对"每帧都炸"的那种。
pub const DEFAULT_CALLBACK_ERROR_LIMIT: u32 = 60;

/// 脚本错误的处置策略。由宿主显式选择，**默认严格**。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptErrorPolicy {
    /// 首个错误即上报（`call_*` 返回 `Err`）。
    ///
    /// 这是**验收与 CI 路径**的默认档：被吞掉的错误等于没被测到。`run_script_steps`
    /// 的既有调用点（26 个无头样例、soak、性能基线）全部 `unwrap()`，因此它们仍然
    /// 会把脚本错误变成失败。
    #[default]
    Strict,
    /// 容错：错误记入结构化诊断与计数器，本步继续跑。
    ///
    /// 交互宿主（`game.exe`、编辑器 Play）用这一档。连续错误达到上限的回调会被
    /// **停用**（不再调用，也不再刷日志），而不是让进程去死。
    Contain,
}

/// 可容错回调的槽位。每槽位一份独立预算，一个回调的连续错误不会牵连另一个。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BudgetSlot {
    /// 工程级 `update()`。
    Update,
    /// UI 点击分发（`UiEvents` 绑定的任意 Lua 函数）。
    UiClicks,
    /// 每实体脚本钩子。
    Entity(EntityId, EntityHook),
}

/// 每实体脚本的两个钩子。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntityHook {
    Spawn,
    Update,
}

impl EntityHook {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Spawn => "on_spawn",
            Self::Update => "on_update",
        }
    }
}

/// 一次错误的处置结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainOutcome {
    /// 记完账继续调用该回调。
    Continue,
    /// 该回调已停用（本次错误使其达到上限，或此前已停用）。
    Disabled,
}

/// 单个回调的连续错误预算。成功一次即清零。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CallbackBudget {
    consecutive: u32,
    disabled: bool,
}

impl CallbackBudget {
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    #[must_use]
    pub const fn consecutive(&self) -> u32 {
        self.consecutive
    }

    fn on_success(&mut self) {
        self.consecutive = 0;
    }

    /// 记一次错误，返回"是否因此停用"。
    fn on_error(&mut self, limit: u32) -> bool {
        self.consecutive = self.consecutive.saturating_add(1);
        if self.consecutive >= limit.max(1) {
            self.disabled = true;
        }
        self.disabled
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 容错状态：策略 + 各回调预算 + 累计计数。
#[derive(Debug)]
pub struct ScriptGuard {
    policy: ScriptErrorPolicy,
    error_limit: u32,
    total_errors: u64,
    update: CallbackBudget,
    ui_clicks: CallbackBudget,
    entity_hooks: HashMap<(EntityId, EntityHook), CallbackBudget>,
    /// 已停用回调的可读键（宿主与诊断面板用）。用集合是为了"每槽位只报一次"。
    disabled: BTreeSet<String>,
}

impl Default for ScriptGuard {
    fn default() -> Self {
        Self {
            policy: ScriptErrorPolicy::default(),
            error_limit: DEFAULT_CALLBACK_ERROR_LIMIT,
            total_errors: 0,
            update: CallbackBudget::default(),
            ui_clicks: CallbackBudget::default(),
            entity_hooks: HashMap::new(),
            disabled: BTreeSet::new(),
        }
    }
}

impl ScriptGuard {
    /// 把新策略转换为新容错器：计数与预算全部清零。
    ///
    /// 生命周期与"一次 Play/一次加载"对齐即可，因此策略切换（编辑器 Play/Stop、
    /// 热重载）顺手重置——上一局停用的回调不该让下一局也哑着。
    #[must_use]
    pub fn new(policy: ScriptErrorPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn policy(&self) -> ScriptErrorPolicy {
        self.policy
    }

    /// 覆盖连续错误上限（验收/测试用；生产走 [`DEFAULT_CALLBACK_ERROR_LIMIT`]）。
    pub fn set_error_limit(&mut self, limit: u32) {
        self.error_limit = limit.max(1);
    }

    #[must_use]
    pub const fn total_errors(&self) -> u64 {
        self.total_errors
    }

    #[must_use]
    pub fn is_disabled(&self, slot: BudgetSlot) -> bool {
        self.budget(slot).is_disabled()
    }

    /// 已停用回调的可读键（按字典序，稳定输出便于断言）。
    #[must_use]
    pub fn disabled_keys(&self) -> Vec<String> {
        self.disabled.iter().cloned().collect()
    }

    /// 记一次**成功**：清零该槽位的连续错误计数。
    pub fn record_success(&mut self, slot: BudgetSlot) {
        self.budget(slot).on_success();
    }

    /// 记一次错误：累计计数，并在该槽位停用时登记可读键。
    pub fn record_error(&mut self, slot: BudgetSlot) -> ContainOutcome {
        self.total_errors = self.total_errors.saturating_add(1);
        if self.budget(slot).on_error(self.error_limit) {
            self.disabled.insert(slot_key(slot));
            return ContainOutcome::Disabled;
        }
        ContainOutcome::Continue
    }

    /// 丢弃已消亡实体的预算。
    ///
    /// 每实体预算以 `EntityId` 为键，实体反复生成/销毁会长期累积；`EntityId` 的
    /// generation 每次都不同，所以旧键永远不会再被命中——不剪就是恒定泄漏。
    pub fn prune_entities<'a>(&mut self, live: impl Iterator<Item = &'a EntityId>) {
        let mut live_ids: BTreeSet<EntityId> = live.copied().collect();
        self.entity_hooks
            .retain(|(id, _), _| live_ids.remove(id) || live_ids.contains(id));
        // 上面那段 retain 只保证"留下的键在 live 里"；已消亡实体的可读键同样要撤下，
        // 否则诊断面板会一直显示一个早就不存在的实体。
        let stale: Vec<String> = self
            .disabled
            .iter()
            .filter(|key| key.starts_with("entity:"))
            .filter(|key| !self.entity_hooks.keys().any(|(id, _)| key.contains(&id.to_string())))
            .cloned()
            .collect();
        for key in stale {
            self.disabled.remove(&key);
        }
    }

    /// 全量重置（热重载/新一局）：预算与计数清零，停用列表清空。
    pub fn reset(&mut self) {
        let policy = self.policy;
        let error_limit = self.error_limit;
        *self = Self {
            policy,
            error_limit,
            ..Self::default()
        };
    }

    fn budget(&self, slot: BudgetSlot) -> &CallbackBudget {
        match slot {
            BudgetSlot::Update => &self.update,
            BudgetSlot::UiClicks => &self.ui_clicks,
            BudgetSlot::Entity(id, hook) => self
                .entity_hooks
                .get(&(id, hook))
                .unwrap_or(&EMPTY_BUDGET),
        }
    }

    fn budget_mut(&mut self, slot: BudgetSlot) -> &mut CallbackBudget {
        match slot {
            BudgetSlot::Update => &mut self.update,
            BudgetSlot::UiClicks => &mut self.ui_clicks,
            BudgetSlot::Entity(id, hook) => self.entity_hooks.entry((id, hook)).or_default(),
        }
    }
}

/// 未登记槽位的只读替身（未出错即未停用、连续计数为 0）。
static EMPTY_BUDGET: CallbackBudget = CallbackBudget {
    consecutive: 0,
    disabled: false,
};

fn slot_key(slot: BudgetSlot) -> String {
    match slot {
        BudgetSlot::Update => "update()".to_owned(),
        BudgetSlot::UiClicks => "ui.click".to_owned(),
        BudgetSlot::Entity(id, hook) => format!("entity:{}:{}", id.index(), hook.name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> EntityId {
        EntityId::new(index, 1)
    }

    /// 默认策略必须是 Strict：验收路径的既有语义不因本模块加入而改变。
    #[test]
    fn default_policy_is_strict() {
        assert_eq!(ScriptErrorPolicy::default(), ScriptErrorPolicy::Strict);
        assert_eq!(ScriptGuard::default().policy(), ScriptErrorPolicy::Strict);
    }

    /// 连续错误到上限才停用；中间成功一次即清零（"偶发错误可自愈"）。
    #[test]
    fn budget_disables_only_after_consecutive_errors() {
        let mut guard = ScriptGuard::new(ScriptErrorPolicy::Contain);
        guard.set_error_limit(3);
        assert_eq!(guard.record_error(BudgetSlot::Update), ContainOutcome::Continue);
        assert_eq!(guard.record_error(BudgetSlot::Update), ContainOutcome::Continue);
        guard.record_success(BudgetSlot::Update);
        assert_eq!(guard.record_error(BudgetSlot::Update), ContainOutcome::Continue);
        assert!(!guard.is_disabled(BudgetSlot::Update));
        assert_eq!(guard.record_error(BudgetSlot::Update), ContainOutcome::Continue);
        assert_eq!(guard.record_error(BudgetSlot::Update), ContainOutcome::Disabled);
        assert!(guard.is_disabled(BudgetSlot::Update));
        assert_eq!(guard.disabled_keys(), vec!["update()".to_owned()]);
    }

    /// 槽位互不牵连：UI 点击连炸不该把 `update()` 一起停掉。
    #[test]
    fn budgets_are_independent_per_slot() {
        let mut guard = ScriptGuard::new(ScriptErrorPolicy::Contain);
        guard.set_error_limit(1);
        guard.record_error(BudgetSlot::UiClicks);
        assert!(guard.is_disabled(BudgetSlot::UiClicks));
        assert!(!guard.is_disabled(BudgetSlot::Update));
        assert!(!guard.is_disabled(BudgetSlot::Entity(entity(7), EntityHook::Update)));
        assert_eq!(guard.disabled_keys(), vec!["ui.click".to_owned()]);
    }

    /// 相同实体不同钩子各自计账。
    #[test]
    fn entity_hooks_have_separate_budgets() {
        let mut guard = ScriptGuard::new(ScriptErrorPolicy::Contain);
        guard.set_error_limit(2);
        guard.record_error(BudgetSlot::Entity(entity(1), EntityHook::Spawn));
        guard.record_error(BudgetSlot::Entity(entity(1), EntityHook::Update));
        assert_eq!(guard.total_errors(), 2);
        guard.record_error(BudgetSlot::Entity(entity(1), EntityHook::Spawn));
        assert!(guard.is_disabled(BudgetSlot::Entity(entity(1), EntityHook::Spawn)));
        assert!(!guard.is_disabled(BudgetSlot::Entity(entity(1), EntityHook::Update)));
    }

    /// 消亡实体的预算与停用键都要被剪掉，否则是恒定泄漏。
    #[test]
    fn pruning_drops_dead_entities() {
        let mut guard = ScriptGuard::new(ScriptErrorPolicy::Contain);
        guard.set_error_limit(1);
        guard.record_error(BudgetSlot::Entity(entity(1), EntityHook::Update));
        guard.record_error(BudgetSlot::Entity(entity(2), EntityHook::Update));
        assert_eq!(guard.disabled_keys().len(), 2);
        let live = [entity(1)];
        guard.prune_entities(live.iter());
        assert!(guard.is_disabled(BudgetSlot::Entity(entity(1), EntityHook::Update)));
        assert!(!guard.is_disabled(BudgetSlot::Entity(entity(2), EntityHook::Update)));
        assert_eq!(guard.disabled_keys(), vec!["entity:1:on_update".to_owned()]);
    }

    /// 重置保留策略与上限，其余清零。
    #[test]
    fn reset_keeps_policy_and_clears_state() {
        let mut guard = ScriptGuard::new(ScriptErrorPolicy::Contain);
        guard.set_error_limit(1);
        guard.record_error(BudgetSlot::Update);
        guard.reset();
        assert_eq!(guard.policy(), ScriptErrorPolicy::Contain);
        assert_eq!(guard.total_errors(), 0);
        assert!(!guard.is_disabled(BudgetSlot::Update));
        assert!(guard.disabled_keys().is_empty());
    }
}
