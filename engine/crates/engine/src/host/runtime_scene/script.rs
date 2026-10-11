//! 脚本桥：`with_script`、错误策略、诊断与热重载（WP12 阶段 3c 从 `mod.rs` 原样搬出）。
//!
//! 全部是 `pub` 方法，没有一处需要放宽可见性：`RuntimeScene` 只持有 `LuaRuntime` 的所有权，
//! 错误策略/诊断计数这些状态仍在那台运行时里。

use rustlua_scripting::script::{LuaRuntime, ReloadStatus, ScriptDiagnostic, ScriptErrorPolicy};

use super::RuntimeScene;

impl RuntimeScene {
    /// Grants temporary access to the underlying Lua runtime — the escape
    /// hatch for registered frame systems (and tests) that need to drive
    /// script APIs from a `&mut RuntimeScene` context.
    pub fn with_script<R>(&mut self, visit: impl FnOnce(&mut LuaRuntime) -> R) -> R {
        visit(&mut self.script)
    }

    /// 设置脚本错误处置策略（见 [`ScriptErrorPolicy`]）。
    ///
    /// **默认严格**：脚本报错 ⇒ `run_script_steps` 返回 `Err`。无头样例、soak 与性能
    /// 基线全都 `unwrap()` 这个结果，因此验收路径"脚本一错就判红"的语义不因容错能力
    /// 的加入而改变。交互宿主（`game.exe`、编辑器 Play）显式切到
    /// [`ScriptErrorPolicy::Contain`]：错误记进诊断与计数，游戏继续跑，而不是让玩家
    /// 的一局因为某个 Lua 分支写错而中断——此前那条路径会直接终止进程，并连带跳过
    /// 本步的物理/粒子/场景切换。
    pub fn set_script_error_policy(&mut self, policy: ScriptErrorPolicy) {
        self.script.set_error_policy(policy);
    }

    /// 当前脚本错误处置策略。
    #[must_use]
    pub fn script_error_policy(&self) -> ScriptErrorPolicy {
        self.script.error_policy()
    }

    /// 自本次会话开始被**容错**的脚本错误总数。
    ///
    /// 容错不等于静默：宿主与验收脚本用这个计数把"脚本出过错"变成可断言的信号
    /// （严格档下错误走 `Err`，因此该计数只在容错档增长）。
    #[must_use]
    pub fn script_error_total(&self) -> u64 {
        self.script.script_error_total()
    }

    /// 因连续报错被停用的脚本回调（可读键，按字典序）。
    #[must_use]
    pub fn script_disabled_callbacks(&self) -> Vec<String> {
        self.script.disabled_callbacks()
    }

    /// 脚本侧结构化诊断（Console 面板的数据源）。
    #[must_use]
    pub fn script_diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.script.diagnostics()
    }

    /// docs/54 §4.1：热重载**脚本层的全部内容**——工程级脚本 + 每实体脚本。
    ///
    /// 宿主此前只调 `reload_if_changed()`，那只会看到 `scripts/main.lua`；实体脚本改了却"状态
    /// 未变"会让"热重载不生效"变成一个没有线索的问题。这个入口一次返回两件事：
    /// 工程级脚本的状态与**被重载的实体脚本文件数**。
    pub fn reload_scripts(&mut self) -> anyhow::Result<(ReloadStatus, usize)> {
        let status = self.script.reload_if_changed()?;
        let entity_scripts = self.script.reload_entity_scripts()?;
        Ok((status, entity_scripts))
    }
}
