//! 脚本桥：`with_script`、错误策略、诊断与热重载（WP12 阶段 3c 从 `mod.rs` 原样搬出）。
//!
//! 全部是 `pub` 方法，没有一处需要放宽可见性：`RuntimeScene` 只持有 `LuaRuntime` 的所有权，
//! 错误策略/诊断计数这些状态仍在那台运行时里。

use rustlua_scripting::script::{
    CrashSnapshot, EntitySummary, LuaRuntime, ReloadStatus, ScriptDiagnostic, ScriptErrorPolicy,
};

use super::RuntimeScene;

impl RuntimeScene {
    /// Grants temporary access to the underlying Lua runtime — the escape
    /// hatch for registered frame systems (and tests) that need to drive
    /// script APIs from a `&mut RuntimeScene` context.
    pub fn with_script<R>(&mut self, visit: impl FnOnce(&mut LuaRuntime) -> R) -> R {
        visit(&mut self.script)
    }

    /// P3-E06-4：Lua VM 当前占用的堆字节（`mlua::Lua::used_memory`）。
    ///
    /// 这是真实的 VM 分配量，不是估算；不含 Rust 侧贴图/网格缓存。只读借用，不触发 GC。
    #[must_use]
    pub fn lua_memory_bytes(&self) -> u64 {
        self.script.lua().used_memory() as u64
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

    /// 运行期未捕获错误的现场快照（最新的在后）。
    ///
    /// 快照在错误**现场**抓（栈 + 局部变量 + 实体摘要），因此只能在守卫的消息处理
    /// 函数里生成——一旦栈展开，这两个信息来源就没了。这里只做转发，不改变
    /// [`ScriptErrorPolicy`] 的任何语义：严格档照样 `Err`，容错档照样继续跑。
    #[must_use]
    pub fn crash_snapshots(&self) -> Vec<CrashSnapshot> {
        self.script.crash_snapshots()
    }

    /// 最近一条错误现场（无错误时为 `None`）。
    #[must_use]
    pub fn latest_crash(&self) -> Option<CrashSnapshot> {
        self.script.latest_crash()
    }

    /// 快照序号游标：宿主记住上一次处理到的序号，据此只处理新增的那些
    /// （序号只增不减，`clear` 之前不会重复）。
    #[must_use]
    pub fn crash_sequence(&self) -> u64 {
        self.script.crash_sequence()
    }

    /// 当时的实体状态摘要（场景实体数 + 运行期实体数 + 带脚本的实体清单）。
    #[must_use]
    pub fn entity_summary(&self) -> EntitySummary {
        self.script.entity_summary()
    }

    /// 清空快照队列（测试/验收用）。
    pub fn clear_crash_snapshots(&self) {
        self.script.clear_crash_snapshots();
    }

    /// docs/54 §4.1：热重载**脚本层的全部内容**——工程级脚本 + 每实体脚本。
    ///
    /// 宿主此前只调 `reload_if_changed()`，那只会看到 `scripts/main.lua`；实体脚本改了却"状态
    /// 未变"会让"热重载不生效"变成一个没有线索的问题。这个入口一次返回两件事：
    /// 工程级脚本的状态与**被重载的实体脚本文件数**。
    ///
    /// **没有工程级入口时不报错**（纯实体脚本工程、引擎诊断模式）：那种情况下问
    /// `reload_if_changed` 必然得到 "no project script is loaded"，而宿主真正想知道的是
    /// "实体脚本变了没有"。静默返回 `Unchanged` 并继续重载实体脚本，否则这类工程的
    /// 每一次去抖检查都会变成一条假红字。
    pub fn reload_scripts(&mut self) -> anyhow::Result<(ReloadStatus, usize)> {
        let status = if self.script.has_project_script() {
            self.script.reload_if_changed()?
        } else {
            ReloadStatus::Unchanged
        };
        let entity_scripts = self.script.reload_entity_scripts()?;
        Ok((status, entity_scripts))
    }
}
