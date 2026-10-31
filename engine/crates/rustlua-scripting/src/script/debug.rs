//! 无头 Lua 调试核心：行级断点、单步、暂停快照（行号 / 局部变量 / 调用栈）。
//!
//! ## 为什么是"同步暂停"
//!
//! 宿主的脚本调用是同步的（wasm 上只有一个线程），既有的心跳/预算钩子也按这个前提写成
//! `set_global_hook`。真正的阻塞式暂停要另起 worker 线程，而那一层属于宿主（编辑器 Play），
//! 不属于脚本运行时。所以这里把"暂停点"定义成**钩子内的一次同步回调**：
//!
//! - 命中断点或单步条件时，钩子已经在 Lua 执行流里原地停住；
//! - [`PauseSnapshot`] 在这一刻把行号、局部变量与调用栈**完整物化**（之后 Lua 侧继续跑也不会变）；
//! - 宿主回调返回 [`DebugAction`] 决定下一步：继续、步入、步过、步出。
//!
//! 无头测试用一个只记录快照的回调，就能断言"暂停帧 + 行号 + 局部变量"；编辑器 Play 以后可以在
//! 回调里阻塞等待 UI 的"继续"按钮。两者共用同一份钩子逻辑。
//!
//! ## 与指令预算共用一个钩子
//!
//! mlua 每个状态只有一个全局钩子，所以这里复用 [`super::progress::install`] 安装的那个：
//! 行事件交给本模块，计数事件仍归心跳/预算。**未启用调试时不加行触发**，正常运行的钩子
//! 成本与改造前一致。
//!
//! ## 栈层级的取法
//!
//! 钩子本身是 C 函数，`debug.getinfo` 的前几层会混入 C 帧（钩子、mlua 的调用包装）。
//! 因此栈与局部变量都**按 `what != "C"` 过滤**，第一层 Lua 帧就是当前执行的函数；
//! 局部变量按这一层的 `debug.getlocal` 读取，未命名临时量（名字以 `(` 开头）不展示。

use std::{
    cell::RefCell,
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    rc::Rc,
};

use mlua::{Debug, Function, Lua, Table, Value, VmState};

use super::LuaBridgeError;

/// 一个源码行断点。
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Breakpoint {
    /// 源码路径。文件加载时 chunk 名是实际路径（见 `load_candidate`），这里按同一口径比较。
    pub path: PathBuf,
    /// 1 起的行号。
    pub line: u32,
}

impl Breakpoint {
    pub fn new(path: impl Into<PathBuf>, line: u32) -> Self {
        Self {
            path: path.into(),
            line,
        }
    }
}

/// 暂停原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseReason {
    /// 命中了断点。
    Breakpoint,
    /// 单步抵达了下一行。
    Step,
}

/// 暂停后宿主给出的下一步。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugAction {
    /// 继续跑到下一个断点。
    Continue,
    /// 单步进入：下一行（含被调用函数内部）就停。
    StepInto,
    /// 单步跳过：同层或更浅的下一行才停。
    StepOver,
    /// 单步跳出：回到调用者后停。
    StepOut,
}

/// 暂停位置。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    /// 规范化后的绝对路径（去掉 Lua 的 `@` 前缀，统一 `/` 分隔）。
    pub path: PathBuf,
    /// 1 起的行号。
    pub line: u32,
}

/// 一帧调用栈。顺序是**当前帧在前**。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackFrame {
    /// 函数名（`debug.getinfo` 的 `name`；匿名函数退回 `what`）。
    pub function: String,
    /// 该帧正在执行的源码位置。
    pub location: SourceLocation,
}

/// 一个局部变量。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalVariable {
    pub name: String,
    /// Lua 类型名（`nil`/`number`/`string`/`table`/…）。
    pub kind: String,
    /// 值的可读文本（表/函数显示为 `<table>`/`<function>`，不展开）。
    pub value: String,
}

/// 一次暂停的完整现场。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PauseSnapshot {
    pub reason: PauseReason,
    /// 暂停处（当前帧）。
    pub location: SourceLocation,
    /// 调用栈，当前帧在前。
    pub frames: Vec<StackFrame>,
    /// 当前帧的局部变量（按 `debug.getlocal` 的顺序）。
    pub locals: Vec<LocalVariable>,
}

impl PauseSnapshot {
    /// 当前帧的函数名。
    #[must_use]
    pub fn function(&self) -> &str {
        self.frames
            .first()
            .map_or("<unknown>", |frame| frame.function.as_str())
    }

    /// 按名字取局部变量（测试与宿主展示共用）。
    #[must_use]
    pub fn local(&self, name: &str) -> Option<&LocalVariable> {
        self.locals.iter().find(|local| local.name == name)
    }
}

/// 宿主回调：拿到暂停快照，返回下一步。
pub type DebugHandler = Box<dyn FnMut(PauseSnapshot) -> DebugAction>;

/// VM 的私有 `debug` 表在 registry 里的键（见 `runtime::new_vm`；脚本的 `_G.debug` 是 nil）。
pub(crate) const DEBUG_REGISTRY_KEY: &str = "__rustlua_debug";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepKind {
    Into,
    Over,
    Out,
}

#[derive(Clone, Debug)]
struct StepState {
    kind: StepKind,
    /// 暂停时的位置与深度：用来判断"是否已经离开这一行/这一层"。
    source: String,
    line: u32,
    depth: usize,
}

#[derive(Default)]
struct DebugState {
    breakpoints: BTreeSet<Breakpoint>,
    handler: Option<DebugHandler>,
    step: Option<StepState>,
    /// 已记录的源码字符串 → 规范化路径，避免每次行事件都做一次 replace。
    sources: Vec<(String, PathBuf)>,
    /// 暂停次数（宿主/测试可读；也是"钩子到底有没有接上"的直接读数）。
    pauses: u64,
}

thread_local! {
    /// 调试状态装在 `thread_local`：与 `progress` 同一理由——一个 Lua 状态只在一个线程上用。
    static STATE: RefCell<DebugState> = RefCell::new(DebugState::default());
}

/// 调试是否已启用（有断点或有回调）。未启用时 `progress::install` 不装行触发。
pub(crate) fn is_active() -> bool {
    STATE.with(|state| {
        let state = state.borrow();
        !state.breakpoints.is_empty() || state.handler.is_some()
    })
}

/// 设置断点（覆盖式）。空集合等于关闭断点。
pub fn set_breakpoints(breakpoints: impl IntoIterator<Item = Breakpoint>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.breakpoints = breakpoints.into_iter().collect();
        state.sources.clear();
    });
}

/// 当前断点（按路径与行号排序）。
#[must_use]
pub fn breakpoints() -> Vec<Breakpoint> {
    STATE.with(|state| state.borrow().breakpoints.iter().cloned().collect())
}

/// 安装/替换暂停回调。
pub fn set_handler(handler: impl FnMut(PauseSnapshot) -> DebugAction + 'static) {
    STATE.with(|state| state.borrow_mut().handler = Some(Box::new(handler)));
}

/// 清空断点、回调与单步状态，并清掉源码缓存。
pub fn clear() {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.breakpoints.clear();
        state.handler = None;
        state.step = None;
        state.sources.clear();
    });
}

/// 已发生的暂停次数。
#[must_use]
pub fn pause_count() -> u64 {
    STATE.with(|state| state.borrow().pauses)
}

/// 行事件入口：由 [`super::progress`] 的钩子调用。
///
/// 返回 `VmState::Continue`：暂停是同步回调，不是 VM 级挂起；单步条件不满足时直接放行。
pub(crate) fn on_line(lua: &Lua, debug: &Debug) -> mlua::Result<VmState> {
    let info = debug.source();
    let source = info
        .source
        .as_deref()
        .or(info.short_src.as_deref())
        .unwrap_or("<unknown>");
    let source = normalize_source(source);
    let line = debug.current_line().map_or(0, |line| line as u32);
    if line == 0 {
        return Ok(VmState::Continue);
    }
    let path = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some((_, path)) = state.sources.iter().find(|(key, _)| key == &source) {
            return path.clone();
        }
        let path = PathBuf::from(&source);
        // 一个工程最多几百个源文件；这里线性存一份就够，不做 LRU。
        if state.sources.len() >= 1024 {
            state.sources.clear();
        }
        state.sources.push((source.clone(), path.clone()));
        path
    });

    let location = SourceLocation {
        path: path.clone(),
        line,
    };

    // 先回答"这一行要不要停"：断点优先；否则看单步条件。
    let stop = STATE.with(|state| {
        let state = state.borrow();
        if state
            .breakpoints
            .iter()
            .any(|point| point.line == line && path_matches(&point.path, &location.path))
        {
            return Some(PauseReason::Breakpoint);
        }
        let step = state.step.as_ref()?;
        let same_line = step.line == line && step.source == source;
        match step.kind {
            StepKind::Into => (!same_line).then_some(PauseReason::Step),
            StepKind::Over | StepKind::Out => {
                let depth = lua_depth(lua).unwrap_or(step.depth);
                let reached = match step.kind {
                    StepKind::Over => depth <= step.depth,
                    StepKind::Out => depth < step.depth,
                    StepKind::Into => unreachable!("handled above"),
                };
                (reached && !same_line).then_some(PauseReason::Step)
            }
        }
    });
    let Some(reason) = stop else {
        return Ok(VmState::Continue);
    };

    let depth = lua_depth(lua).unwrap_or(0);
    let snapshot = capture_snapshot(lua, reason, &location);
    let action = STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.pauses = state.pauses.saturating_add(1);
        state.step = None;
        let Some(handler) = state.handler.as_mut() else {
            return DebugAction::Continue;
        };
        // 回调里允许借用宿主状态，但不允许再进调试 API（会二次借用本 `RefCell`）。
        // `catch_unwind` 只是把"回调 panic"收敛成一次继续执行，避免把调试点变成新的崩溃点。
        match catch_unwind(AssertUnwindSafe(|| handler(snapshot))) {
            Ok(action) => action,
            Err(_) => DebugAction::Continue,
        }
    });
    if let DebugAction::StepInto | DebugAction::StepOver | DebugAction::StepOut = action {
        let kind = match action {
            DebugAction::StepInto => StepKind::Into,
            DebugAction::StepOver => StepKind::Over,
            _ => StepKind::Out,
        };
        STATE.with(|state| {
            state.borrow_mut().step = Some(StepState {
                kind,
                source: source.clone(),
                line,
                depth,
            });
        });
    }
    Ok(VmState::Continue)
}

/// 物化暂停现场：调用栈（按 Lua 帧过滤）+ 当前帧局部变量。
fn capture_snapshot(lua: &Lua, reason: PauseReason, location: &SourceLocation) -> PauseSnapshot {
    let Ok(debug_table) = lua.named_registry_value::<Table>(DEBUG_REGISTRY_KEY) else {
        return PauseSnapshot {
            reason,
            location: location.clone(),
            frames: vec![StackFrame {
                function: "<unknown>".to_owned(),
                location: location.clone(),
            }],
            locals: Vec::new(),
        };
    };
    let getinfo: Option<Function> = debug_table.get("getinfo").ok();
    let getlocal: Option<Function> = debug_table.get("getlocal").ok();

    let mut frames = Vec::new();
    let mut top_level = None;
    if let Some(getinfo) = getinfo {
        for level in 1..=64 {
            let Ok(info) = getinfo.call::<Table>((level, "nSl")) else {
                break;
            };
            let what = info.get::<String>("what").unwrap_or_default();
            if what == "C" {
                continue;
            }
            if top_level.is_none() {
                top_level = Some(level);
            }
            let source = info.get::<String>("source").unwrap_or_default();
            let line = info.get::<i64>("currentline").unwrap_or(0).max(0) as u32;
            let name = info.get::<Option<String>>("name").ok().flatten();
            let function = match name {
                Some(name) if !name.is_empty() => name,
                _ => match what.as_str() {
                    "main" => "<main>".to_owned(),
                    _ => "<anonymous>".to_owned(),
                },
            };
            frames.push(StackFrame {
                function,
                location: SourceLocation {
                    path: PathBuf::from(normalize_source(&source)),
                    line,
                },
            });
        }
    }
    if frames.is_empty() {
        frames.push(StackFrame {
            function: "<unknown>".to_owned(),
            location: location.clone(),
        });
    }

    let mut locals = Vec::new();
    if let (Some(getlocal), Some(level)) = (getlocal, top_level) {
        for index in 1..=128 {
            let Ok(values) = getlocal.call::<mlua::MultiValue>((level, index)) else {
                break;
            };
            let mut iter = values.into_iter();
            let (Some(Value::String(name)), value) = (iter.next(), iter.next()) else {
                break;
            };
            let name = name.to_string_lossy().to_owned();
            // Lua 5.4 会把栈上的临时量以 `(temporary)` 之类的名字报出来，对读者没有意义。
            if name.starts_with('(') {
                continue;
            }
            let value = value.unwrap_or(Value::Nil);
            locals.push(LocalVariable {
                name,
                kind: lua_type_name(&value).to_owned(),
                value: super::values::format_lua_value(value),
            });
        }
    }

    PauseSnapshot {
        reason,
        location: location.clone(),
        frames,
        locals,
    }
}

/// 当前栈上 Lua 帧的层数（当前帧算 1）。
fn lua_depth(lua: &Lua) -> Option<usize> {
    let debug_table: Table = lua.named_registry_value(DEBUG_REGISTRY_KEY).ok()?;
    let getinfo: Function = debug_table.get("getinfo").ok()?;
    let mut depth = 0;
    for level in 1..=64 {
        let Ok(info) = getinfo.call::<Table>((level, "S")) else {
            break;
        };
        if info.get::<String>("what").unwrap_or_default() != "C" {
            depth += 1;
        }
    }
    (depth > 0).then_some(depth)
}

fn lua_type_name(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nil",
        Value::Boolean(_) => "boolean",
        Value::Integer(_) | Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Table(_) => "table",
        Value::Function(_) => "function",
        Value::Thread(_) => "thread",
        Value::UserData(_) | Value::LightUserData(_) => "userdata",
        Value::Error(_) => "error",
        Value::Other(_) => "other",
    }
}

/// Lua 的 chunk 名带 `@`（文件）/`=`（内部块）前缀，对读者与比较都没有意义。
fn normalize_source(source: &str) -> String {
    let source = source
        .strip_prefix('@')
        .or_else(|| source.strip_prefix('='))
        .unwrap_or(source);
    source.replace('\\', "/")
}

/// 断点路径与源码路径的比较：绝对路径相等，或断点是源码的后缀（便于用工程相对路径下断点）。
fn path_matches(breakpoint: &Path, source: &Path) -> bool {
    let breakpoint = normalize_path(breakpoint);
    let source = normalize_path(source);
    if breakpoint == source {
        return true;
    }
    // `source.ends_with("/" + breakpoint)`：断点 `scripts/main.lua` 命中 `E:/proj/scripts/main.lua`。
    source.ends_with(&format!("/{breakpoint}"))
}

fn normalize_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value
    }
}

impl super::LuaRuntime {
    /// 设置断点并刷新钩子（行触发只在启用调试时安装）。
    pub fn set_debug_breakpoints(
        &self,
        breakpoints: impl IntoIterator<Item = Breakpoint>,
    ) -> Result<(), LuaBridgeError> {
        set_breakpoints(breakpoints);
        self.refresh_debug_hook()
    }

    /// 当前断点。
    #[must_use]
    pub fn debug_breakpoints(&self) -> Vec<Breakpoint> {
        breakpoints()
    }

    /// 安装暂停回调（命中后同步调用）。回调里**不要**再调用本结构体的调试 API。
    pub fn set_debug_handler(
        &self,
        handler: impl FnMut(PauseSnapshot) -> DebugAction + 'static,
    ) -> Result<(), LuaBridgeError> {
        set_handler(handler);
        self.refresh_debug_hook()
    }

    /// 已发生的暂停次数。
    #[must_use]
    pub fn debug_pause_count(&self) -> u64 {
        pause_count()
    }

    /// 关闭调试：清断点/回调/单步，并把钩子退回"只做心跳与预算"。
    pub fn clear_debugger(&self) -> Result<(), LuaBridgeError> {
        clear();
        self.refresh_debug_hook()
    }

    /// 按当前调试开关重装钩子。候选 VM（热重载）在 `load_candidate` 里也会自行重装。
    fn refresh_debug_hook(&self) -> Result<(), LuaBridgeError> {
        super::progress::install(&self.lua, super::progress::budget())
            .map_err(|error| LuaBridgeError::Init(error.to_string()))
    }
}

/// `Rc` 只是为了让测试能共享一份记录器；生产路径不经过这里。
pub type SharedPauses = Rc<RefCell<Vec<PauseSnapshot>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::LuaRuntime;

    fn runtime() -> LuaRuntime {
        LuaRuntime::with_installers(crate::script::domain::gameplay::input_lua::registry())
            .expect("runtime")
    }

    fn script(contents: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("rustlua-debug-{id}-{unique}.lua"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// 验收门：断点命中后，暂停帧带着行号与局部变量表。
    #[test]
    fn breakpoint_pauses_with_line_and_locals() {
        let path = script(
            "local a = 1\n\
             local b = 2\n\
             local c = a + b\n\
             function update() return c end\n",
        );
        let runtime = runtime();
        let pauses: SharedPauses = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&pauses);
        runtime
            .set_debug_handler(move |snapshot| {
                recorder.borrow_mut().push(snapshot);
                DebugAction::Continue
            })
            .unwrap();
        runtime
            .set_debug_breakpoints([Breakpoint::new(path.clone(), 3)])
            .unwrap();

        let mut runtime = runtime;
        runtime.execute_project_script(&path).unwrap();

        let pauses = pauses.borrow();
        assert_eq!(pauses.len(), 1, "断点必须命中且只命中一次：{pauses:?}");
        let pause = &pauses[0];
        assert_eq!(pause.reason, PauseReason::Breakpoint);
        assert_eq!(pause.location.line, 3);
        assert_eq!(normalize_path(&pause.location.path), normalize_path(&path));
        assert_eq!(
            pause.local("a").map(|local| local.value.as_str()),
            Some("1")
        );
        assert_eq!(
            pause.local("b").map(|local| local.value.as_str()),
            Some("2")
        );
        assert_eq!(
            pause.frames.first().map(|frame| frame.location.line),
            Some(3),
            "当前帧要指在断点行"
        );
        let _ = std::fs::remove_file(path);
        runtime.clear_debugger().unwrap();
    }

    /// 验收门：单步之后行号前进，且原因变成 `Step`。
    #[test]
    fn step_advances_to_the_next_line() {
        let path = script(
            "local a = 1\n\
             local b = 2\n\
             local c = a + b\n\
             local d = c + 1\n\
             function update() return d end\n",
        );
        let runtime = runtime();
        let pauses: SharedPauses = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&pauses);
        runtime
            .set_debug_handler(move |snapshot| {
                let mut pauses = recorder.borrow_mut();
                if pauses.is_empty() {
                    pauses.push(snapshot);
                    return DebugAction::StepInto;
                }
                pauses.push(snapshot);
                DebugAction::Continue
            })
            .unwrap();
        runtime
            .set_debug_breakpoints([Breakpoint::new(path.clone(), 3)])
            .unwrap();

        let mut runtime = runtime;
        runtime.execute_project_script(&path).unwrap();

        let pauses = pauses.borrow();
        assert_eq!(pauses.len(), 2, "断点一次 + 单步一次：{pauses:?}");
        assert_eq!(pauses[0].location.line, 3);
        assert_eq!(pauses[1].reason, PauseReason::Step);
        assert!(
            pauses[1].location.line > pauses[0].location.line,
            "单步后行号必须前进：{} -> {}",
            pauses[0].location.line,
            pauses[1].location.line
        );
        let _ = std::fs::remove_file(path);
        runtime.clear_debugger().unwrap();
    }

    /// 验收门：栈帧点击落到的文件与偏移正确（内层命中时能看到外层调用点）。
    #[test]
    fn stack_frames_carry_file_and_offset() {
        // 调用点刻意不用尾调用（`return inner(x)`）：尾调用会丢掉调用帧与函数名，
        // 那测的是 Lua 的尾调用语义，不是栈帧本身。
        let path = script(
            "local function inner(x)\n\
             \tlocal y = x + 1\n\
             \treturn y\n\
             end\n\
             local function outer()\n\
             \tlocal result = inner(41)\n\
             \treturn result\n\
             end\n\
             function update()\n\
             \tlocal result = outer()\n\
             \treturn result\n\
             end\n",
        );
        let runtime = runtime();
        let pauses: SharedPauses = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&pauses);
        runtime
            .set_debug_handler(move |snapshot| {
                recorder.borrow_mut().push(snapshot);
                DebugAction::Continue
            })
            .unwrap();
        runtime
            .set_debug_breakpoints([Breakpoint::new(path.clone(), 2)])
            .unwrap();

        let mut runtime = runtime;
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();

        let pauses = pauses.borrow();
        assert_eq!(pauses.len(), 1, "断点只在内层命中一次：{pauses:?}");
        let pause = &pauses[0];
        assert_eq!(pause.function(), "inner");
        assert_eq!(pause.frames[0].location.line, 2);
        assert!(
            pause.frames.len() >= 3,
            "应有 inner/outer/update 至少三帧：{:?}",
            pause.frames
        );
        let outer = &pause.frames[1];
        assert_eq!(normalize_path(&outer.location.path), normalize_path(&path));
        assert_eq!(outer.location.line, 6, "外层帧要落在调用 inner 的那一行");
        assert_eq!(
            pause.local("x").map(|local| local.value.as_str()),
            Some("41")
        );
        let _ = std::fs::remove_file(path);
        runtime.clear_debugger().unwrap();
    }

    /// 反面对照：没有断点（也没有单步）时钩子照常跑，但**一次都不暂停**。
    #[test]
    fn no_breakpoints_means_no_pause() {
        let path = script(
            "local a = 1\n\
             local b = 2\n\
             local c = a + b\n\
             function update() return c end\n",
        );
        let runtime = runtime();
        let pauses: SharedPauses = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&pauses);
        runtime
            .set_debug_handler(move |snapshot| {
                recorder.borrow_mut().push(snapshot);
                DebugAction::Continue
            })
            .unwrap();
        // 刻意不设断点。
        runtime.set_debug_breakpoints([]).unwrap();

        let mut runtime = runtime;
        runtime.execute_project_script(&path).unwrap();

        assert!(pauses.borrow().is_empty(), "无断点不该暂停");
        assert_eq!(runtime.debug_pause_count(), 0);
        let _ = std::fs::remove_file(path);
        runtime.clear_debugger().unwrap();
    }

    /// 反面对照：清掉调试钩子后，同样的断点配置不再生效。
    #[test]
    fn clearing_the_debugger_removes_the_hook() {
        let path = script(
            "local a = 1\n\
             local b = 2\n\
             local c = a + b\n\
             function update() return c end\n",
        );
        let runtime = runtime();
        let pauses: SharedPauses = Rc::new(RefCell::new(Vec::new()));
        let recorder = Rc::clone(&pauses);
        runtime
            .set_debug_handler(move |snapshot| {
                recorder.borrow_mut().push(snapshot);
                DebugAction::Continue
            })
            .unwrap();
        runtime
            .set_debug_breakpoints([Breakpoint::new(path.clone(), 3)])
            .unwrap();
        runtime.clear_debugger().unwrap();

        let mut runtime = runtime;
        runtime.execute_project_script(&path).unwrap();

        assert!(
            pauses.borrow().is_empty(),
            "钩子被清掉后断点不应再命中（这一条就是'删掉注册即判红'的反面）"
        );
        let _ = std::fs::remove_file(path);
    }
}
