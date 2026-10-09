//! 脚本执行的**心跳**与**指令预算**。
//!
//! ## 为什么要有这个
//!
//! 宿主跑工程脚本是**同步**的（wasm 上更是只有一个线程），所以一个
//! `while true do end` 就能把整页拖死，而现场什么线索都不留下：M73 第 10 轮读到的正是这个——
//! 页面停在 `Loading`，状态属性里只剩"进了 boot 这一步"。
//!
//! 这里用 Lua 的**全局钩子**把"执行到哪了"变成可读数据，并把"不会停"变成一条**带位置**的报错：
//! 每 [`HOOK_STEP`] 条指令记一次 `<源>:<行>` 与累计指令数；超过预算就抛 Lua 错误，
//! 于是 `execute_project_script` 照常返回错误、页面活着，而读数里带着"卡在哪一行"。
//!
//! 用全局钩子（而不是线程钩子）是因为**协程也算**：`while true do end` 写在协程里同样要能被拦住。
//!
//! ## 预算取值的理由
//!
//! 见 [`DEFAULT_INSTRUCTION_BUDGET`]：正常工程的启动在**千万**条指令量级，默认给两个数量级余量。
//! 死循环不是"慢"，是"不会停"——两者的区别不需要靠调参去猜。

use std::cell::{Cell, RefCell};

use mlua::{Debug, HookTriggers, Lua, VmState};

use super::{LuaBridgeError, LuaRuntime};

/// 默认预算：10 亿条指令。留了两个数量级余量，见模块注释。
pub const DEFAULT_INSTRUCTION_BUDGET: u64 = 1_000_000_000;

/// 钩子每多少条指令触发一次。
///
/// 取 2 万：一帧的脚本工作量（约十万条）只多几次回调，成本可忽略；而卡住时进度粒度是
/// "两万条指令"，足够定位到具体那一行。取太小会变成"为了诊断把性能搭进去"。
const HOOK_STEP: u32 = 20_000;

/// 脚本执行进度 = 最近一次心跳。
///
/// **心跳是粗粒度的**：按 [`HOOK_STEP`] 条指令触发一次，所以"跑了不到 2 万条指令"的脚本
/// 不会留下心跳（[`script_progress`] 返回 `None`）。这是刻意的取舍——心跳要能常开，
/// 就不能按行计费；代价是它只能回答"跑了不少于 N 条、当前停在哪儿"，不能回答"跑了几条"。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScriptProgress {
    /// 本周期内已执行的指令数（**下界**：钩子按 `HOOK_STEP` 计数）。
    pub instructions: u64,
    /// 触发心跳时所在的源（Lua 的短源名，通常带 `@` 前缀）。
    pub source: String,
    /// 触发心跳时的行号（Lua `curr_line`，1 起）。
    pub line: i32,
}

impl std::fmt::Display for ScriptProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `@` 是 Lua 给"从文件加载的块"加的前缀，对读日志的人没有意义。
        let source = self.source.strip_prefix('@').unwrap_or(&self.source);
        write!(f, "{}:{} 指令={}", source, self.line, self.instructions)
    }
}

thread_local! {
    /// 进度装在 `thread_local`：一个 Lua 状态本来就只在一个线程上用（wasm 上只有一个线程），
    /// 这里没有跨线程共享的需求，也就不必引锁。
    static PROGRESS: RefCell<ScriptProgress> = const {
        RefCell::new(ScriptProgress {
            instructions: 0,
            source: String::new(),
            line: 0,
        })
    };
    /// 当前预算（0 = 不设预算）。同样只在装钩子的那个线程上读写。
    static BUDGET: Cell<u64> = const { Cell::new(DEFAULT_INSTRUCTION_BUDGET) };
}

/// 最近一次心跳。**一次都没心跳过时返回 `None`**："没跑过"与"跑了 0 条"是两件事，
/// 读数里混在一起会让"脚本没被调用"看起来像"脚本跑了但没干活"。
pub fn script_progress() -> Option<ScriptProgress> {
    PROGRESS.with(|progress| {
        let progress = progress.borrow();
        (progress.instructions > 0).then(|| progress.clone())
    })
}

/// 本周期清零。每个"卡住就致命"的入口（载入脚本、实例化场景、逻辑步）开头调一次。
///
/// 必须按**入口**清零而不是只清一次：预算是"这一段不许无限跑"，不是"这一局不许跑够 10 亿条"。
/// 后者会把跑上几小时的正经游戏误杀。
pub(crate) fn arm() {
    PROGRESS.with(|progress| {
        let mut progress = progress.borrow_mut();
        progress.instructions = 0;
        progress.source.clear();
        progress.line = 0;
    });
}

/// 当前预算（0 = 不设预算）。新造的 Lua 状态（热重载候选、每实体脚本）用它接着装同一个钩子——
/// 否则"换了一份状态"就成了绕过护栏的后门。
pub(crate) fn budget() -> u64 {
    BUDGET.with(Cell::get)
}

/// 给一个 Lua 状态装钩子（`set_global_hook` 覆盖全状态，协程也算）。
///
/// 重复安装是允许的：后一次覆盖前一次，预算随之更新。
pub(crate) fn install(lua: &Lua, budget: u64) -> mlua::Result<()> {
    BUDGET.with(|slot| slot.set(budget));
    arm();
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(HOOK_STEP),
        |_lua: &Lua, debug: &Debug| {
            let total = PROGRESS.with(|progress| {
                let mut progress = progress.borrow_mut();
                progress.instructions += u64::from(HOOK_STEP);
                let source = debug.source();
                progress.source = source
                    .short_src
                    .as_deref()
                    .or(source.source.as_deref())
                    .unwrap_or("<unknown>")
                    .to_owned();
                progress.line = debug.current_line().map_or(0, |line| line as i32);
                progress.instructions
            });
            let budget = BUDGET.with(Cell::get);
            if budget > 0 && total > budget {
                return Err(mlua::Error::RuntimeError(format!(
                    "脚本超过指令预算 {budget}（最后心跳 {}）",
                    script_progress().unwrap_or_default()
                )));
            }
            Ok(VmState::Continue)
        },
    )
}

impl LuaRuntime {
    /// 改这条运行时的指令预算（`0` = 不设预算）。同时装心跳钩子。
    ///
    /// 取 `&self`：Lua 句柄本身是共享的（`Lua` 内部带锁/`Rc`），装钩子不需要独占。
    pub fn set_instruction_budget(&self, budget: u64) -> Result<(), LuaBridgeError> {
        install(&self.lua, budget).map_err(|error| LuaBridgeError::Api(error.to_string()))
    }

    /// 本周期清零（宿主在"可能卡住的一段"开头调用；引擎入口也会自己调）。
    pub fn arm_instruction_budget(&self) {
        arm();
    }

    /// 当前预算（`0` = 不设预算）。
    pub fn instruction_budget(&self) -> u64 {
        BUDGET.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不会停的脚本必须变成**一条带位置的报错**，而不是把调用方挂住。
    ///
    /// 预算调小是为了让测试几毫秒结束：这里验的是"拦得住 + 报得出位置"，不是阈值本身。
    #[test]
    fn runaway_script_trips_the_budget_with_a_location() {
        let runtime = LuaRuntime::new().expect("runtime");
        runtime.set_instruction_budget(100_000).expect("budget");
        let error = runtime
            .lua
            .load("local n = 0\nwhile true do n = n + 1 end")
            .exec()
            .expect_err("死循环必须被预算拦下");
        let message = error.to_string();
        assert!(
            message.contains("指令预算"),
            "报错要说明是预算拦下的，实际：{message}"
        );
        let progress = script_progress().expect("拦下时应当已经有过心跳");
        assert!(
            progress.instructions > 100_000,
            "心跳计数要超过预算，实际：{progress}"
        );
        assert!(
            progress.line > 0,
            "心跳要带行号（卡在哪一行），实际：{progress}"
        );
    }

    /// 正常脚本：不留报错，但**留下心跳**——"跑到过哪一段"本身是诊断信息。
    ///
    /// 这条脚本刻意跑够 [`HOOK_STEP`] 条指令：心跳是粗粒度的，不到一个步长的脚本不留心跳
    /// （那是设计，不是缺陷，见 [`ScriptProgress`] 的注释）。
    #[test]
    fn normal_script_leaves_progress_without_error() {
        let runtime = LuaRuntime::new().expect("runtime");
        runtime.set_instruction_budget(10_000_000).expect("budget");
        let source = "local sum = 0\nfor i = 1, 500000 do sum = sum + i end\nreturn sum";
        runtime.lua.load(source).exec().expect("正常脚本不该被拦");
        let progress = script_progress().expect("跑过就不该没有心跳");
        assert!(progress.instructions > 0);
        assert!(progress.line > 0, "心跳要带行号，实际：{progress}");
    }

    /// 预算为 0 = 明确不设预算：这条口子必须留着（宿主可以自己决定要不要护栏），
    /// 否则"引擎替所有宿主做了决定"。
    #[test]
    fn zero_budget_disables_the_guard() {
        let runtime = LuaRuntime::new().expect("runtime");
        runtime.set_instruction_budget(0).expect("budget");
        assert_eq!(runtime.instruction_budget(), 0);
    }
}
