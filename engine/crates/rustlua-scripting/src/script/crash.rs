//! 运行期未捕获错误的**现场快照**：栈、局部变量、心跳与实体摘要。
//!
//! ## 为什么用 xpcall 而不是"错误后回看"
//!
//! Lua 错误一旦穿过 `pcall`/宿主，栈就已经展开，局部变量随之消失。所以现场只能在
//! **错误发生的那一刻**抓。这里在 VM 建好时装一个 Lua 侧守卫：
//!
//! ```text
//! guard(f, ...) = xpcall(f, handler, ...)
//! ```
//!
//! `handler` 是 Rust 函数，作为 message handler 在栈**尚未展开**时被调用；它在里面用
//! 与断点快照同一份 [`super::debug`] 栈解析读出调用栈与最内层 Lua 帧的局部变量，
//! 然后原样把错误（带 `debug.traceback`）作为结果交回去——所以错误文本与可捕获性
//! 都没有改变，脚本里 `pcall` 吞掉的错误也**到不了**这里（那是脚本自己的决定）。
//!
//! ## 只给宿主"看得到"的东西
//!
//! 守卫与 handler 都放在 registry（脚本 `_G` 看不到），`debug` 库依旧从 `_G` 摘掉。
//! 守卫用创建时捕获的 `xpcall` 上值，脚本重定义 `_G.xpcall` 也绕不过去。
//!
//! ## 生命周期
//!
//! 一次错误在 handler 里落进 `PENDING`，由最外层调用点（[`LuaRuntime::finalize_crash`]）
//! 补上回调身份与致命标记后推进 [`recent`]。宿主（编辑器 Play）据此落盘与弹窗；
//! 无头路径不消费也不受影响（快照只是内存里最近 16 条）。

use std::{cell::RefCell, collections::VecDeque, path::PathBuf};

use mlua::{Function, Lua, MultiValue, Table, Value};

use super::debug::{
    DEBUG_REGISTRY_KEY, LocalVariable, SourceLocation, StackFrame, capture_stack_and_locals,
};
use super::{LuaBridgeError, ScriptProgress};

/// 守卫函数在 registry 里的键（脚本不可见）。
pub(crate) const GUARD_REGISTRY_KEY: &str = "__rustlua_crash_guard";
/// message handler 在 registry 里的键（守卫的 upvalue，脚本不可见）。
const HANDLER_REGISTRY_KEY: &str = "__rustlua_crash_handler";
/// 守卫 chunk 的调试名。栈解析出来的帧会带上它（守卫是普通 Lua 函数），
/// 落快照时要从栈尾剪掉——那是宿主的调用壳，不是作者的代码。
const GUARD_CHUNK_NAME: &str = "__rustlua_crash_guard";

/// 内存里保留的最近快照条数。落盘由宿主负责；这里只够"最后一帧错误"与刚过去的几条。
const MAX_RECENT: usize = 16;
/// 实体摘要里列出的脚本实体上限（摘要不是全量转储）。
const MAX_SCRIPTED_ENTITIES: usize = 16;

/// 一个脚本实体的摘要行。
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptedEntity {
    /// `index:generation`，与诊断键同一口径。
    pub id: String,
    pub name: Option<String>,
    /// 工程相对脚本路径。
    pub file: String,
    /// 2D 局部位置（没有 Transform 时为 `None`）。
    pub position: Option<[f32; 2]>,
}

/// 错误发生时世界的**摘要**（不是快照，不能据此还原世界）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntitySummary {
    /// 场景声明的实体数（不含脚本运行期 spawn 的）。
    pub scene_entities: usize,
    /// 运行时 ECS 世界里的实体总数。
    pub runtime_entities: usize,
    /// 带 `EntityScript` 的实体（条数有上限，上限是同文件的私有常量 `MAX_SCRIPTED_ENTITIES`）。
    pub scripted: Vec<ScriptedEntity>,
}

/// 一次未捕获错误的完整现场。
#[derive(Clone, Debug, PartialEq)]
pub struct CrashSnapshot {
    /// 进程内单调序号（宿主据此只消费新快照）。
    pub sequence: u64,
    /// 抓取时刻（Unix 毫秒；wasm 上固定 0）。
    pub captured_at_ms: u64,
    /// 出错回调的可读键（`update()` / `entity:1:g1:on_update` / `ui.click`）。
    pub callback: Option<String>,
    /// 是否致命：回调被停用或错误直接中止了这一步。
    pub fatal: bool,
    /// Lua 错误文本（含 `stack traceback`）。
    pub message: String,
    /// 最内层 Lua 帧的位置。
    pub location: Option<SourceLocation>,
    /// 调用栈，当前帧在前。
    pub frames: Vec<StackFrame>,
    /// 最内层帧的局部变量。
    pub locals: Vec<LocalVariable>,
    /// 错误时刻的心跳（指令数下界与位置）。
    pub progress: Option<ScriptProgress>,
    /// 世界摘要。
    pub entities: EntitySummary,
}

impl CrashSnapshot {
    /// 落盘/展示用的 JSON 形态（schema 版本随字段变化时递增）。
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let location = self.location.as_ref().map(|location| {
            serde_json::json!({
                "path": location.path.to_string_lossy().replace('\\', "/"),
                "line": location.line,
            })
        });
        let frames: Vec<serde_json::Value> = self
            .frames
            .iter()
            .map(|frame| {
                serde_json::json!({
                    "function": frame.function,
                    "path": frame.location.path.to_string_lossy().replace('\\', "/"),
                    "line": frame.location.line,
                })
            })
            .collect();
        let locals: Vec<serde_json::Value> = self
            .locals
            .iter()
            .map(|local| {
                serde_json::json!({
                    "name": local.name,
                    "kind": local.kind,
                    "value": local.value,
                })
            })
            .collect();
        let progress = self.progress.as_ref().map(|progress| {
            serde_json::json!({
                "instructions": progress.instructions,
                "source": progress.source,
                "line": progress.line,
            })
        });
        let scripted: Vec<serde_json::Value> = self
            .entities
            .scripted
            .iter()
            .map(|entity| {
                serde_json::json!({
                    "id": entity.id,
                    "name": entity.name,
                    "file": entity.file,
                    "position": entity.position,
                })
            })
            .collect();
        serde_json::json!({
            "schema": 1,
            "sequence": self.sequence,
            "captured_at_ms": self.captured_at_ms,
            "callback": self.callback,
            "fatal": self.fatal,
            "message": self.message,
            "location": location,
            "frames": frames,
            "locals": locals,
            "progress": progress,
            "entities": {
                "scene": self.entities.scene_entities,
                "runtime": self.entities.runtime_entities,
                "scripted": scripted,
            },
        })
    }

    /// 第一行错误文本（弹窗标题/控制台行用）。多行 traceback 留在 `message` 里。
    #[must_use]
    pub fn headline(&self) -> &str {
        self.message.lines().next().unwrap_or("").trim()
    }
}

/// handler 在栈还活着时抓到的半成品；回调身份与致命标记由最外层调用点补。
#[derive(Clone, Debug)]
struct PendingCapture {
    message: String,
    frames: Vec<StackFrame>,
    locals: Vec<LocalVariable>,
    progress: Option<ScriptProgress>,
    captured_at_ms: u64,
}

#[derive(Default)]
struct CrashStore {
    next_sequence: u64,
    pending: Option<PendingCapture>,
    recent: VecDeque<CrashSnapshot>,
}

thread_local! {
    /// 与 `progress`/`debug` 同一理由：一个 Lua 状态只在一个线程上用。
    static STORE: RefCell<CrashStore> = RefCell::new(CrashStore::default());
}

/// 给一个新建的 Lua 状态装崩溃守卫。
///
/// 必须在脚本执行前调用（`new_vm` 里），否则守卫创建之前的错误抓不到——那正是
/// `execute_project_script` 的加载期错误，属于宿主一定要看到的现场。
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let handler = lua.create_function(|lua: &Lua, error: Value| -> mlua::Result<Value> {
        let text = capture_error(lua, &error);
        Ok(Value::String(lua.create_string(&text)?))
    })?;
    lua.set_named_registry_value(HANDLER_REGISTRY_KEY, handler.clone())?;
    // 守卫在 Lua 里定义：`xpcall` 与 handler 都是上值，脚本改 `_G.xpcall` 也绕不过去。
    // 注意 `xpcall` 自 Lua 5.2 起支持透传参数（本仓是 5.4）。
    let guard: Function = lua
        .load(
            "local xpcall, handler = xpcall, ...\n\
             return function(f, ...) return xpcall(f, handler, ...) end",
        )
        .set_name(GUARD_CHUNK_NAME)
        .call(handler)?;
    lua.set_named_registry_value(GUARD_REGISTRY_KEY, guard)
}

/// 在**栈尚未展开**时抓现场，并返回交给上层的错误文本（带 traceback）。
///
/// message handler 里不能报错：任何一种读取失败都退化成"少一点信息"，而不是把原来的
/// Lua 错误替换成诊断代码自己的错误。
fn capture_error(lua: &Lua, error: &Value) -> String {
    let fallback = lua
        .coerce_string(error.clone())
        .ok()
        .flatten()
        .map(|text| text.to_string_lossy())
        .unwrap_or_else(|| "<非字符串错误>".to_owned());
    let Ok(debug_table) = lua.named_registry_value::<Table>(DEBUG_REGISTRY_KEY) else {
        return fallback;
    };
    // traceback 的 level 1 是 message handler 自己；2 才是出错函数。
    let text = debug_table
        .get::<Function>("traceback")
        .ok()
        .and_then(|traceback| traceback.call::<String>((error.clone(), 2)).ok())
        .unwrap_or(fallback);
    let captured = capture_stack_and_locals(lua);
    STORE.with(|store| {
        store.borrow_mut().pending = Some(PendingCapture {
            message: text.clone(),
            frames: captured.frames,
            locals: captured.locals,
            progress: super::progress::script_progress(),
            captured_at_ms: now_ms(),
        });
    });
    text
}

/// 走守卫调用一个 Lua 函数。
///
/// 返回 `Ok(Ok(第一个返回值))` / `Ok(Err(错误文本))`；`Err(mlua::Error)` 只表示守卫本身
/// 不可用（registry 缺失、参数转换失败），与脚本错误无关。
pub(crate) fn call_guarded(
    lua: &Lua,
    function: Function,
    args: impl mlua::IntoLuaMulti,
) -> mlua::Result<Result<Value, String>> {
    let guard: Function = lua.named_registry_value(GUARD_REGISTRY_KEY)?;
    let mut all: MultiValue = args.into_lua_multi(lua)?;
    all.push_front(Value::Function(function));
    let returned: MultiValue = guard.call(all)?;
    let mut values = returned.into_iter();
    match values.next() {
        Some(Value::Boolean(true)) => Ok(Ok(values.next().unwrap_or(Value::Nil))),
        Some(Value::Boolean(false)) => {
            let error = values.next().unwrap_or(Value::Nil);
            let text = match error {
                Value::String(text) => text.to_string_lossy(),
                other => lua
                    .coerce_string(other)
                    .ok()
                    .flatten()
                    .map(|text| text.to_string_lossy())
                    .unwrap_or_else(|| "<非字符串错误>".to_owned()),
            };
            Ok(Err(text))
        }
        _ => Err(mlua::Error::RuntimeError(
            "crash guard 返回值异常（应为 bool, ...）".to_owned(),
        )),
    }
}

/// 取走 handler 抓到的半成品（没有就返回 `None`）。
fn take_pending() -> Option<PendingCapture> {
    STORE.with(|store| store.borrow_mut().pending.take())
}

/// 推进最近快照队列并分配序号。
fn push_snapshot(snapshot: CrashSnapshot) {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.next_sequence = store.next_sequence.saturating_add(1);
        let sequence = store.next_sequence;
        let mut snapshot = snapshot;
        snapshot.sequence = sequence;
        if store.recent.len() >= MAX_RECENT {
            store.recent.pop_front();
        }
        store.recent.push_back(snapshot);
    });
}

/// 最近一次快照（无错误时为 `None`）。
#[must_use]
pub(crate) fn latest() -> Option<CrashSnapshot> {
    STORE.with(|store| store.borrow().recent.back().cloned())
}

/// 最近快照（按发生顺序，旧的在前）。
#[must_use]
pub(crate) fn recent() -> Vec<CrashSnapshot> {
    STORE.with(|store| store.borrow().recent.iter().cloned().collect())
}

/// 序号游标：宿主用它判断"有没有新快照"。
#[must_use]
pub(crate) fn sequence() -> u64 {
    STORE.with(|store| store.borrow().next_sequence)
}

/// 清空（测试/换工程用；编辑器 Play 每次新建预览会话时序号继续单调，不互相覆盖）。
pub(crate) fn clear() {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        store.pending = None;
        store.recent.clear();
    });
}

fn now_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

impl super::LuaRuntime {
    /// 把 handler 抓到的现场补成快照：回调身份、致命标记与实体摘要在这里落定。
    ///
    /// 返回新快照的序号；没有挂起现场（例如错误来自 guard 之外的路径）时返回 `None`。
    pub(super) fn finalize_crash(&self, callback: Option<&str>, fatal: bool) -> Option<u64> {
        let pending = take_pending()?;
        // 守卫本身是 Lua 函数，栈尾会带一帧宿主壳；剪掉它，快照里只剩作者的栈。
        let mut frames = pending.frames;
        if let Some(index) = frames
            .iter()
            .position(|frame| frame.location.path == std::path::Path::new(GUARD_CHUNK_NAME))
        {
            frames.truncate(index);
        }
        let location = frames.first().map(|frame| frame.location.clone());
        push_snapshot(CrashSnapshot {
            sequence: 0,
            captured_at_ms: pending.captured_at_ms,
            callback: callback.map(str::to_owned),
            fatal,
            message: pending.message,
            location,
            frames,
            locals: pending.locals,
            progress: pending.progress,
            entities: self.entity_summary(),
        });
        Some(sequence())
    }

    /// 错误发生时的世界摘要（脚本实体 + 计数）。只读，不触发传播。
    #[must_use]
    pub fn entity_summary(&self) -> EntitySummary {
        use super::domain::ecs::entity_script::EntityScript;
        use super::domain::ecs::transform::Transform;

        let entities = self.state.borrow().entity_state.clone();
        let scene_entities = entities.with(|entities| entities.scene_entities.len());
        self.with_world(|world| {
            let mut scripted = Vec::new();
            world.for_each::<EntityScript>(|id, script| {
                if scripted.len() >= MAX_SCRIPTED_ENTITIES {
                    return;
                }
                let name = entities.with(|entities| entities.names.first_name(id));
                let position = world
                    .get::<Transform>(id)
                    .ok()
                    .flatten()
                    .map(|transform| [transform.position.x, transform.position.y]);
                scripted.push(ScriptedEntity {
                    id: format!("{}:{}", id.index(), id.generation()),
                    name,
                    file: script.file.clone(),
                    position,
                });
            });
            EntitySummary {
                scene_entities,
                runtime_entities: world.entity_count(),
                scripted,
            }
        })
    }

    /// 最近 16 条错误现场（旧的在前）。
    #[must_use]
    pub fn crash_snapshots(&self) -> Vec<CrashSnapshot> {
        recent()
    }

    /// 最近一条错误现场。
    #[must_use]
    pub fn latest_crash(&self) -> Option<CrashSnapshot> {
        latest()
    }

    /// 快照序号游标（宿主据此只处理新快照）。
    #[must_use]
    pub fn crash_sequence(&self) -> u64 {
        sequence()
    }

    /// 清空本线程的快照队列（验收/测试用）。
    pub fn clear_crash_snapshots(&self) {
        clear();
    }
}

/// 让外层调用点把 `mlua::Error` 转成脚本错误：守卫路径不会走到这里，
/// 但取函数/参数转换失败仍然要按原口径上报。
pub(super) fn bridge_result(
    result: mlua::Result<Result<Value, String>>,
    path: PathBuf,
    prefix: Option<&str>,
) -> Result<Value, LuaBridgeError> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(reason)) => Err(LuaBridgeError::Runtime {
            path,
            reason: match prefix {
                Some(prefix) => format!("{prefix}: {reason}"),
                None => reason,
            },
        }),
        Err(error) => Err(LuaBridgeError::Runtime {
            path,
            reason: match prefix {
                Some(prefix) => format!("{prefix}: {error}"),
                None => error.to_string(),
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use crate::script::debug::{Breakpoint, DebugAction, PauseSnapshot};
    use crate::script::{LuaRuntime, ScriptErrorPolicy};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

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
        let path = std::env::temp_dir().join(format!("rustlua-crash-{id}-{unique}.lua"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// 正常运行不许被打扰：返回值逐字不变、也不产生快照（守卫的成功路径零副作用）。
    #[test]
    fn success_path_is_unchanged_and_captures_nothing() {
        let path = script("function update() return 'still-ok' end\n");
        let mut runtime = runtime();
        runtime.execute_project_script(&path).unwrap();
        let value = runtime.call_update().expect("update must succeed");
        assert_eq!(value, "still-ok");
        assert!(runtime.crash_snapshots().is_empty(), "成功路径不得留下快照");
        assert_eq!(runtime.crash_sequence(), 0);
        let _ = std::fs::remove_file(path);
    }

    /// 验收门：未捕获错误在**栈还活着**时抓到帧与局部变量。
    #[test]
    fn uncaught_error_captures_frames_and_locals() {
        let path = script(
            // 调用点刻意不用尾调用（`return deep(true)`）：尾调用会拆掉调用帧并丢掉函数名，
            // 那测的是 Lua 的尾调用语义，不是崩溃现场。断点快照的测试同理。
            "local function deep(flag)\n\
             \tlocal marker = 'boom-guard'\n\
             \tif flag then error('exploded: ' .. marker) end\n\
             \treturn marker\n\
             end\n\
             function update()\n\
             \tlocal result = deep(true)\n\
             \treturn result\n\
             end\n",
        );
        let mut runtime = runtime();
        runtime.execute_project_script(&path).unwrap();
        let error = runtime.call_update().expect_err("update 必须报错");
        let text = error.to_string();
        assert!(text.contains("exploded"), "错误文本要原样保留：{text}");

        let snapshots = runtime.crash_snapshots();
        assert_eq!(snapshots.len(), 1, "一次错误只留一条：{snapshots:?}");
        let snapshot = &snapshots[0];
        assert!(snapshot.fatal, "严格档下未被捕获的错误即致命");
        assert_eq!(snapshot.callback.as_deref(), Some("update()"));
        assert_eq!(
            snapshot.frames.first().map(|frame| frame.function.as_str()),
            Some("deep"),
            "栈顶必须是出错帧：{:?}",
            snapshot.frames
        );
        // 最外层那帧是入口回调 `update`：它由宿主的 xpcall 直接调用（C 帧），Lua 推不出
        // 函数名，所以显示 `<anonymous>`；调用者身份由 `callback` 字段给出（上面那行）。
        // 这里断言的是"调用者那一帧确实在栈上、且落在调用 deep 的行"。
        assert_eq!(
            snapshot.frames.get(1).map(|frame| frame.location.line),
            Some(7),
            "栈里要能看到调用者所在的源码行：{:?}",
            snapshot.frames
        );
        assert_eq!(
            snapshot
                .locals
                .iter()
                .find(|local| local.name == "marker")
                .map(|local| local.value.as_str()),
            Some("boom-guard"),
            "错误帧的局部变量必须保留：{:?}",
            snapshot.locals
        );
        assert!(
            snapshot.message.contains("stack traceback"),
            "消息里要带 traceback：{}",
            snapshot.message
        );
        assert!(snapshot.headline().contains("exploded"));
        // 脚本只定义了函数、没有 spawn 实体：摘要读到的世界就是空的（读数为真，不是恒真式）。
        assert_eq!(snapshot.entities.scene_entities, 0);
        assert_eq!(snapshot.entities.runtime_entities, 0);
        assert!(snapshot.entities.scripted.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// 反面对照：脚本自己 `pcall` 吞掉的错误不进守卫——那是脚本的选择，不是致命错误。
    #[test]
    fn errors_caught_inside_lua_do_not_produce_snapshots() {
        let path = script(
            "function update()\n\
             \tlocal ok = pcall(function() error('handled inside script') end)\n\
             \treturn tostring(ok)\n\
             end\n",
        );
        let mut runtime = runtime();
        runtime.execute_project_script(&path).unwrap();
        assert_eq!(runtime.call_update().unwrap(), "false");
        assert!(
            runtime.crash_snapshots().is_empty(),
            "被脚本 pcall 吞掉的错误不该出现在宿主现场里"
        );
        let _ = std::fs::remove_file(path);
    }

    /// 容错档：未到停用上限的错误不是致命；达到上限的那一次才是。
    #[test]
    fn only_the_disabling_error_is_fatal_in_contain_mode() {
        let path = script("function update() local nil_value = nil; return nil_value.field end\n");
        let mut runtime = runtime();
        runtime.set_error_policy(ScriptErrorPolicy::Contain);
        runtime.set_callback_error_limit(1);
        runtime.execute_project_script(&path).unwrap();
        let value = runtime.call_update().expect("容错档不向宿主返回错误");
        assert_eq!(value, "");
        let snapshots = runtime.crash_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].fatal, "上限为 1 时首个错误即停用，因此是致命");
        assert_eq!(runtime.disabled_callbacks(), vec!["update()".to_owned()]);
        let _ = std::fs::remove_file(path);
    }

    /// 容错档：未到上限的错误照抓，但**不**标记致命（编辑器据此只落盘不弹窗）。
    #[test]
    fn contained_errors_below_the_limit_are_not_fatal() {
        let path = script("function update() error('偶尔炸一次') end\n");
        let mut runtime = runtime();
        runtime.set_error_policy(ScriptErrorPolicy::Contain);
        runtime.set_callback_error_limit(60);
        runtime.execute_project_script(&path).unwrap();
        runtime.call_update().unwrap();
        let snapshots = runtime.crash_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(!snapshots[0].fatal, "未停用不该弹窗");
        assert!(snapshots[0].message.contains("偶尔炸一次"));
        let _ = std::fs::remove_file(path);
    }

    /// 实体摘要：计到场景/运行时实体，并带出脚本实体的名字与位置。
    #[test]
    fn entity_summary_lists_scripted_entities() {
        use crate::script::domain::ecs::entity_script::EntityScript;
        use crate::script::domain::ecs::transform::Transform;
        use crate::script::domain::math::math2d::Vec2;

        let mut runtime = runtime();
        let (id, other) = runtime.with_world_mut(|world| {
            let id = world.spawn();
            world
                .insert(
                    id,
                    EntityScript {
                        file: "scripts/hero.lua".to_owned(),
                        enabled: true,
                    },
                )
                .unwrap();
            world
                .insert(
                    id,
                    Transform {
                        position: Vec2::new(3.0, 4.0),
                        ..Transform::default()
                    },
                )
                .unwrap();
            let other = world.spawn();
            (id, other)
        });
        let runtime_entities = runtime.with_world(|world| world.entity_count());
        assert!(runtime_entities >= 2);
        runtime.register_entity_name(id, "hero");
        let summary = runtime.entity_summary();
        assert_eq!(summary.runtime_entities, runtime_entities);
        assert_eq!(summary.scene_entities, 0, "没有加载场景");
        let hero = summary
            .scripted
            .iter()
            .find(|entity| entity.file == "scripts/hero.lua")
            .expect("脚本实体必须出现在摘要里");
        assert_eq!(hero.name.as_deref(), Some("hero"));
        assert_eq!(hero.position, Some([3.0, 4.0]));
        assert!(
            summary
                .scripted
                .iter()
                .all(|entity| entity.id != format!("{}:{}", other.index(), other.generation()))
        );
    }

    /// 断点与崩溃守卫共存：命中断点不会干扰"错误仍在同一帧被抓住"。
    #[test]
    fn breakpoints_and_crash_capture_coexist() {
        let path = script(
            "local function inner(x)\n\
             \terror('after-breakpoint ' .. x)\n\
             end\n\
             function update() inner(7) end\n",
        );
        let mut runtime = runtime();
        let pauses: Rc<RefCell<Vec<PauseSnapshot>>> = Rc::new(RefCell::new(Vec::new()));
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
        runtime.execute_project_script(&path).unwrap();
        let _ = runtime.call_update().expect_err("仍然要报错");
        assert_eq!(pauses.borrow().len(), 1, "断点应命中一次");
        let snapshots = runtime.crash_snapshots();
        assert_eq!(snapshots.len(), 1);
        assert!(snapshots[0].message.contains("after-breakpoint 7"));
        runtime.clear_debugger().unwrap();
        let _ = std::fs::remove_file(path);
    }

    /// JSON 形态带齐宿主落盘要用的字段（字段名就是文档里的契约）。
    #[test]
    fn snapshot_json_carries_the_contract_fields() {
        let path = script(
            "function update()\n\
             \tlocal payload = { hp = 3 }\n\
             \treturn payload.missing.field\n\
             end\n",
        );
        let mut runtime = runtime();
        runtime.execute_project_script(&path).unwrap();
        let _ = runtime.call_update().expect_err("必须报错");
        let snapshot = runtime.latest_crash().expect("现场");
        let json = snapshot.to_json();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["callback"], "update()");
        assert_eq!(json["fatal"], true);
        assert!(
            json["location"]["path"]
                .as_str()
                .unwrap()
                .replace('\\', "/")
                .ends_with(path.file_name().unwrap().to_string_lossy().as_ref())
        );
        assert!(
            json["frames"]
                .as_array()
                .is_some_and(|frames| !frames.is_empty())
        );
        assert!(
            json["locals"]
                .as_array()
                .is_some_and(|locals| locals.iter().any(|local| local["name"] == "payload"))
        );
        assert!(json["entities"]["runtime"].as_u64().is_some());
        let _ = std::fs::remove_file(path);
    }
}
