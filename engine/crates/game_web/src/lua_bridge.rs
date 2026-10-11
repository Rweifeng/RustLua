//! wasm Lua 桥（WP10 迁移：Lua 执行层从 fengari 迁移到 mlua-wasm）。
//!
//! 职责：把 `web-runtime.js` 与 Lua 的交互面收窄成 5 个 wasm 导出函数，
//! JS 侧不再持有 Lua VM（fengari），全部 Lua 交互改经本桥：
//!
//! | 导出 | 语义 | 对应 web-runtime.js 旧调用 |
//! |---|---|---|
//! | `lua_boot` / `lua_boot_with_reader` | 建 LuaRuntime + vfs provider | `luaL_newstate` + `luaL_openlibs` |
//! | `lua_do_string` | 执行补丁/脚本代码串 | `doString(L, src, name)` |
//! | `lua_set_number` | 直接写 Lua 全局数值，不编译 chunk | `doString("__rustlua_dt = …")` |
//! | `lua_input_frame` | 应用一帧批量输入（`RLFI` 定长二进制，原地改常驻表） | `doString("__rustlua_input = {…}")` |
//! | `input_frame_layout_json` | 线格式布局+键表（JS 打包侧的唯一一份偏移来源） | 无（此前 JS 自己拼源码） |
//! | `lua_call_global` | 调 Lua 全局函数，返回字符串 | `callGlobal(L, name, args)` |
//! | `lua_call_global_void` | 调**无参**全局函数并丢弃返回值 | `doString("__rustlua_apply_virtual_input()")` |
//! | `lua_register_js_callback` | JS 回调注册成 Lua 全局 | `installLuaAudioBridge` 的 `registerLuaFunction` |
//! | `lua_missing_globals` | 查宿主全局缺失清单 | `missingHostGlobals(L)` |
//! | `host_api_module_names_json` | 桌面基线的模块表名字清单（内嵌常量） | 手抄的 `ENGINE_HOST_GLOBALS` |
//! | `lua_api_surface_json` | 运行期真实 `_G` 的「模块表→函数名」表面 | harness 的 API 一致性断言 |
//!
//! 值协议（与 fengari 版逐项对齐）：
//! - 参数：JSON 数组（string/number/boolean/null），Rust 侧转 Lua 值；
//! - 返回值：`callGlobal` 的语义是"只取字符串"——`Value::String/Number` → `Some`，
//!   其余（含 nil / boolean / table）→ `None`（fengari `lua_isstring` 同款判定）；
//! - JS 回调：Lua 参数逐个转 JS 基本值；JS 返回 **数组 = 多返回值**（音频桥
//!   `[ok, reason]` 的 fengari 多 push 语义），单值 = 单返回值。

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

// 浏览器（webgl）入口预加载的项目资源表（`start()` 填充，`lua_boot()` 消费）。
//
// 与微信端"打包资源表"语义一致：key 为项目相对路径（`/` 分隔，如
// `scripts/main.lua`），值为**打包原始字节**（`pack::decode` 由引擎侧
// `read_project_script` 自动解开）。
thread_local! {
    static LUA_RESOURCES: RefCell<Option<std::collections::HashMap<String, Vec<u8>>>> =
        const { RefCell::new(None) };
}

// 当前 Lua 运行时（单线程 wasm：一个槽位足够；`lua_boot` 建，Lua 桥函数用）。
//
// 借用约定：`LuaRuntime::lua()` 只需 `&self`，因此所有只读入口（do_string / execute_main /
// call_global / register_js_callback / missing_globals）一律用共享 `borrow()`；唯一的
// `borrow_mut()` 在 `boot_runtime`（一次性写入，此时尚无 Lua 在跑）。这样 Lua 执行期间经
// JS 回调重入本桥时，嵌套的共享借用可以并存，不会触发 `already mutably borrowed: BorrowError`
// （此前 do_string 持 `borrow_mut` 跨 `.exec()`，重入的 call_global 在 `borrow()` 处 panic →
// wasm 上是无法 unwind 的 `unreachable` trap）。
thread_local! {
    static LUA_RUNTIME: RefCell<Option<rustlua_scripting::script::LuaRuntime>> =
        const { RefCell::new(None) };
}

// JS 回调注册表（`lua_register_js_callback` 存，mlua 包装闭包按索引取用）。
// JS 函数与 wasm 桥同生命周期；Lua 状态销毁前不应清空。
thread_local! {
    static JS_CALLBACKS: RefCell<Vec<js_sys::Function>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// 上一帧输入（`lua_input_frame` 的差分基准）。
    ///
    /// 为什么需要它：批量 ABI 是"原地改常驻表"，而旧协议是"每帧整份替换"——替换会把**上一帧按下、
    /// 这一帧松开**的键从表里抹掉，原地改就必须自己知道该抹掉谁。没有这份基准，松开的键会永远
    /// 停留在 `keys[name] = true`，表现为"松开方向键角色还在走"。
    ///
    /// 存的是**位图**而不是 Lua 值：基准只回答"上一帧哪些位是 1"，与 Lua 表内容无关，因此它不持有
    /// Lua 引用，也就不受借用/生命周期影响。`lua_boot*` 会把它清零（重新 boot 等于换了一份常驻表，
    /// 旧基准没有意义）。文档写在宏**内部**：写在 `thread_local!` 外面会被判成 unused doc comment
    /// （宏调用不接受文档注释），本文件既有的三处是历史写法，不在此处一并改动。
    static INPUT_PREVIOUS: RefCell<rustlua_frame_abi::FrameInput> =
        const { RefCell::new(rustlua_frame_abi::FrameInput::new()) };
}

thread_local! {
    /// 宿主最近送来的一帧线格式输入（`lua_input_frame` 解码校验之后存这里）。
    ///
    /// 为什么留这一份：GPU 侧的统一模拟要走 `RuntimeScene::run_script_steps(.., &InputSnapshot)`，
    /// 而它拿不到 JS 的那份 Lua 表。**两条链必须吃同一份输入**，否则"画面里的角色会动、
    /// 模拟里的不动"，而这种分叉在读数上完全看不出来。存的是解码后的结构体（Copy 语义的位图），
    /// 不持有 Lua 引用。`lua_boot*` 不清它：输入是"这台设备现在的状态"，不是 Lua 世界的状态。
    static LATEST_FRAME: RefCell<rustlua_frame_abi::FrameInput> =
        const { RefCell::new(rustlua_frame_abi::FrameInput::new()) };
}

/// 宿主最近一帧的线格式输入（没收到过任何一帧时是全零帧）。
pub fn latest_frame_input() -> rustlua_frame_abi::FrameInput {
    LATEST_FRAME.with(|slot| *slot.borrow())
}

// ---------------------------------------------------------------- 固定步时钟（WP7-13）
//
// 这套逻辑原本整份在 `plugins/webgl/web-runtime.js` 里：`stepAccumulator` / `STEP_EPSILON` /
// `MAX_CATCH_UP_STEPS` / `droppedSteps`。而引擎的 GPU 独跑路径（`RuntimeScene::run_script_steps`）
// 用的是 `rustlua_infra::FixedTimeStep` —— 同一份产物里两套定步长实现，任何一处调整都会让两条链
// 的步数分叉，而分叉在读数上只表现为"某个平台的动画快一点/慢一点"，极难定位。
//
// 搬进来之后**权威只剩一份**（现在是 `rustlua_infra::frame_clock::FrameClock`），JS 侧不再持有
// 累积器，每帧只问一句"这一帧跑几步"。常量与 JS 旧实现逐条对齐（60 Hz / 最多 8 步 / 1e-9 落帧容差）
// ⇒ 这是**行为等价的状态搬迁**，不是顺手改玩法：两个平台的步进口径因此不可能再漂移。
//
// WP7-53 起这一层换成 `FrameClock`：它把 `FixedTimeStep` 的算术再包一层**策略**——夹住秒级间隔
// （切标签页/断点/长 GC）、计数被夹的帧、容忍"正好一帧"的子 ulp 偏差。微信侧的引擎宿主
// （`host_runtime.rs`）用的是**同一个**策略模块 ⇒ "两个宿主步进节奏一致"从此是结构保证，不是约定。
// 正常帧上两者逐位相同（间隔远小于上限、容差同值）；只有畸形间隔的记账方式变了（同样是 8 步封顶，
// 但丢弃量与累积器不再把挂起的几十秒算进去）。

/// 每秒固定步数（与 JS `FIXED_STEPS_PER_SECOND`、引擎定步长一致）。
const FIXED_STEPS_PER_SECOND: u32 = 60;
/// 单帧最多追几步（与 JS `MAX_CATCH_UP_STEPS` 一致）：超过就丢弃，避免死亡螺旋。
const MAX_CATCH_UP_STEPS: u32 = 8;
/// 单帧接受的**墙钟**上限（秒）：切标签页/断点/长 GC 会送来秒级间隔，先夹住再谈追帧。
const MAX_FRAME_SECONDS: f64 = 0.25;
/// 落帧容差：浏览器的 `elapsed` 来自两次 `performance.now()` 读数之差，"正好一帧"常落在
/// `1/60` 最后几个二进制位**之下**。严格 `>=` 会把它判成 0 步、靠下一帧补上（平均速率仍对、
/// 不会停摆），代价是步数在 0/1 之间抖动。取远小于任何真实帧间隔的值：误判上限是每帧 1e-9 秒。
/// （与 `frame_clock::FRAME_INTERVAL_TOLERANCE_SECONDS` 同值的本地副本；这里显式传一遍，
///   是为了让"这套常量与 JS 旧实现逐条对齐"这件事在本文件里仍然一眼可查。）
const STEP_EPSILON_SECONDS: f64 = 1e-9;

thread_local! {
    /// 固定步时钟（`boot_runtime` 会重置：换 Lua 世界等于换一次时间基准）。
    static FIXED_CLOCK: RefCell<Option<rustlua_infra::frame_clock::FrameClock>> =
        const { RefCell::new(None) };
}

fn with_fixed_clock<R>(f: impl FnOnce(&mut rustlua_infra::frame_clock::FrameClock) -> R) -> R {
    FIXED_CLOCK.with(|slot| {
        let mut guard = slot.borrow_mut();
        if guard.is_none() {
            // `new` 只在频率/追帧上限/上限秒数为 0 时报错，而这三个都是本文件里的非零常量。
            let clock = rustlua_infra::frame_clock::FrameClock::new(
                FIXED_STEPS_PER_SECOND,
                MAX_CATCH_UP_STEPS,
                MAX_FRAME_SECONDS,
            )
            .expect("fixed-step constants are non-zero");
            // 容差取自 `frame_clock` 的常量本身（两处不同源就会悄悄分叉，而分叉只表现为
            // "某个平台的动画快一点/慢一点"）。
            debug_assert_eq!(
                STEP_EPSILON_SECONDS,
                rustlua_infra::frame_clock::FRAME_INTERVAL_TOLERANCE_SECONDS
            );
            *guard = Some(clock.with_tolerance(STEP_EPSILON_SECONDS));
        }
        f(guard.as_mut().expect("just initialised"))
    })
}

/// 推进固定步时钟，返回**本帧要执行的 update 次数**（JS 旧 `advanceFixedSteps()` 的等价物）。
///
/// 累积器、追帧上限、丢帧计数全部留在宿主：JS 拿到的只有"跑几步"这一个数，
/// 因此当帧率抖动（高刷屏、被后台节流、弱网掉帧）时，两个平台的补偿行为由同一份代码决定。
#[wasm_bindgen]
pub fn lua_fixed_frame(elapsed_seconds: f64) -> u32 {
    with_fixed_clock(|clock| clock.plan(elapsed_seconds).steps)
}

/// 累计被丢弃的固定步数（追帧上限之外的、为避免死亡螺旋而丢弃的步数）。诊断用，只读。
#[wasm_bindgen]
pub fn lua_fixed_steps_dropped() -> u32 {
    with_fixed_clock(|clock| clock.total_dropped_steps().min(u64::from(u32::MAX)) as u32)
}

/// 当前累积器余量（秒）。诊断/自证用，只读。
#[wasm_bindgen]
pub fn lua_fixed_accumulator_seconds() -> f64 {
    with_fixed_clock(|clock| clock.accumulator_seconds())
}

/// 丢弃部分时间、不保留时间债：切后台再回前台时，那一帧按"一个固定步"起算，
/// 而不是把挂起的几十秒当成一次巨大的 elapsed 灌进累积器（JS 旧 `resetClockNextFrame` 的等价物）。
#[wasm_bindgen]
pub fn lua_fixed_reset_clock() {
    with_fixed_clock(|clock| clock.reset_accumulator());
}

/// 跑**一个**固定步。这段顺序就是 `plugins/webgl/web-runtime.js` 固定步阶段里的那三句
/// （`__rustlua_apply_virtual_input()` → `__rustlua_dt = delta` → `update()`），搬进 Rust 的
/// 理由是"引擎宿主"要自己驱动定步长：微信侧一旦由 Rust 的帧循环推进模拟，就不能再靠 JS 手写这段
/// 顺序——两份实现就是两份漂移。
///
/// 与 JS 的顺序逐条对齐，其中两条是踩过坑的：
/// * **虚拟键必须在输入批量落地之后**并回来（顺序反了等于没注入）；
/// * `__rustlua_dt` 必须写在 `update()` **之前**（脚本在 update 里读的是这一帧的步长）。
///
/// 返回 `update()` 的字符串化结果（`None` = 它没返回字符串），与 `lua_call_global` 同协议。
#[wasm_bindgen]
pub fn lua_fixed_step(delta_seconds: f64) -> Result<Option<String>, JsValue> {
    lua_call_global_void("__rustlua_apply_virtual_input")?;
    lua_set_number("__rustlua_dt", delta_seconds)?;
    lua_call_global("update", "")
}

/// 连续跑 `steps` 个固定步，返回实际跑的步数（= 入参，便于调用方直接记账）。
///
/// `step == 1` 时先吃掉一次性边沿（`__rustlua_finish_step_input`）：补步**不得重放**
/// `just_pressed`，否则一帧补两步就会让"按一下"变成"按两下"。这条与 JS 里的同一处判断严格对齐。
///
/// 连续跑 `steps` 个固定步，返回**最后一个** `update()` 的字符串化结果
/// （`None` = 没跑，或它没返回字符串）。
///
/// 返回"最后一个脚本返回值"而不是步数：调用方本来就知道自己传了几步，而**这一刻**的脚本返回值
/// 是需要被报出来的读数（`web-runtime.js` 把它写进 `data-rustlua-script-result`，诊断与门都读它）。
/// 旧 JS 循环当年也是"只保留最后一个固定步的返回值"——补步过程中的中间值没有意义。
///
/// 调用方自己已经规划好步数时用这个入口（引擎宿主就是这么用的：它用自己的帧时钟算出 `steps`，
/// 再让这里跑）；`elapsed` 的规划留在调用方，避免同一份时间被规划两次。
#[wasm_bindgen]
pub fn lua_fixed_run_steps(steps: u32, delta_seconds: f64) -> Result<Option<String>, JsValue> {
    let mut last = None;
    for step in 0..steps {
        if step == 1 {
            lua_call_global_void("__rustlua_finish_step_input")?;
        }
        last = lua_fixed_step(delta_seconds)?;
    }
    Ok(last)
}

/// JS 侧问引擎："当前世界里这个名字对应哪个**引擎句柄**？"
///
/// 返回 Lua 侧的句柄编码（`index << 32 | generation`），查不到或那个名字指向的实体已经不在世时返回 **0**
/// （0 的 `generation == 0`，在引擎里本来就不是合法句柄，拿它当"没有"没有歧义）。
///
/// 为什么需要它：webgl/微信的 JS 宿主（`plugins/webgl/web-runtime.js`）有一套自己的 Lua 世界，
/// `entity` / `sprite` / `transform` / `tilemap` / `animation` 这些全局表都被它覆盖，实体 id 用的是
/// **`.scene` 文件里的作者 id**；而它没有覆盖的 API（`quad` / `mesh` / `light` / `world_text`）
/// 会落到引擎上，于是作者 id 被当成引擎句柄解包。第 60 轮实测到的那条报错就是这么来的：
/// 场景里写着 `entity\t13\tScaleBar`，`13` 被解码成 `EntityId(0:g13)`（`index = 13 >> 32 = 0`、
/// `generation = 13`）⇒ `invalid entity id`。
///
/// 有了这个出口，JS 宿主可以在生成 Lua 绑定表时**先问引擎要真句柄**，两个 id 空间就从"各说各话"
/// 变成"以引擎为准"；引擎不认识这个名字时它给 0，调用方交 `nil`——比一个用不了的数字好得多。
#[wasm_bindgen]
pub fn lua_entity_handle(name: &str) -> i64 {
    LUA_RUNTIME.with(|slot| {
        let borrowed = slot.borrow();
        let Some(runtime) = borrowed.as_ref() else {
            return 0;
        };
        runtime.entity_handle_lua(name).unwrap_or(0)
    })
}

/// 把 JS 预加载资源表写进全局（`start()` 在 fetch 预加载后调用）。
pub fn set_resources(resources: std::collections::HashMap<String, Vec<u8>>) {
    LUA_RESOURCES.with(|slot| *slot.borrow_mut() = Some(resources));
}

/// 从预加载资源里取**第一张 PNG**（已解包），给 GPU 宿主当"真资源 → GPU 贴图"的样本。
///
/// 为什么要它：`start()` 预加载的是**打包原始字节**（压缩/加密由引擎侧读取时解开），
/// 所以这里必须走同一套 `pack::decode`，不能把裸字节直接当 PNG——那会得到一个"图集解不开"
/// 的谜题，而根因在别处。找不到任何 PNG 时返回 `None`，调用方退回内存里造的棋盘。
pub fn first_png_resource() -> Option<(String, Vec<u8>)> {
    LUA_RESOURCES.with(|slot| {
        let borrowed = slot.borrow();
        let resources = borrowed.as_ref()?;
        // 路径排序后再取第一张：HashMap 的遍历顺序不稳定，不排序会让"同一份产物每次跑
        // 选到不同贴图"，读数就不可复现了。
        let mut paths: Vec<&String> = resources
            .keys()
            .filter(|key| key.to_ascii_lowercase().ends_with(".png"))
            .collect();
        paths.sort();
        let path = paths.first()?;
        resource_png(path).map(|bytes| ((*path).clone(), bytes))
    })
}

/// 取某个资源路径的**已解包**字节（`pack::decode` 失败时返回 `None` 并告警一次）。
///
/// 与 [`first_png_resource`] 同一口径：资源表里存的是打包原始字节。
pub fn resource_png(path: &str) -> Option<Vec<u8>> {
    let raw = resource_raw(path)?;
    match rustlua_infra::pack::decode(&raw) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            web_sys::console::warn_1(&JsValue::from_str(&format!(
                "[gpu] pack::decode({path}) failed: {error}",
            )));
            None
        }
    }
}

/// 取某个资源路径在表里的**原始字节**（= `start()` 预加载时存进去的那份，未解包）。
///
/// 为什么不一律解包：解包责任在**读它的人**手上，而且两类读者要的不是同一种。
/// 贴图要已解包的 PNG 字节（[`resource_png`]）；场景则要**原始**字节，因为
/// `Scene::from_packed_bytes` / `Scene::load` 内部自己就会 `pack::decode`
/// （发布包里场景是压缩/加密的）。在这里先解一次会让场景被解两遍，得到"读到了文件但解析出来是垃圾"。
pub fn resource_raw(path: &str) -> Option<Vec<u8>> {
    LUA_RESOURCES.with(|slot| {
        let borrowed = slot.borrow();
        let resources = borrowed.as_ref()?;
        resources.get(path).cloned()
    })
}

/// 资源表里有没有这一项（**只看表，不碰文件系统**）。
///
/// 用途：平台侧那些"加载是异步的"通道需要在**同步**时机回答"这个资源存在吗"——
/// 最典型的是音频（`AudioSinkProvider` 要先判存在性，才能把"缺文件"与"平台播放失败"
/// 分开；等浏览器 `error` 事件回来时 `audio.play()` 早就返回了）。
pub fn resource_exists(path: &str) -> bool {
    LUA_RESOURCES.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|resources| resources.contains_key(path))
    })
}

/// 覆盖资源表里某一项的**原始字节**（返回是否本来就有这一项）。
///
/// 用途是**诊断**：把 `scripts/main.lua` 换成"前 N 行"，让"工程脚本卡在哪一段"变成可机械二分的问题
/// ——脚本是在引擎侧跑的，宿主看不到它的执行，但宿主**拿着它的字节**。
/// 覆盖成明文是安全的：`read_project_script` 走 `pack::decode`，而它对明文是原样透传。
pub fn override_resource(path: &str, bytes: Vec<u8>) -> bool {
    LUA_RESOURCES.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(resources) = borrowed.as_mut() else {
            return false;
        };
        let existed = resources.contains_key(path);
        resources.insert(path.to_owned(), bytes);
        existed
    })
}

/// 安装"预加载资源表"这条 vfs provider（**幂等**）。
///
/// 为什么需要幂等：provider 是进程级一次性安装的，而现在有两个合法的先到者——
/// JS 侧的 `lua_boot()`，和 Rust 侧的运行时场景（`RuntimeScene::new`，它读场景/脚本也走 vfs）。
/// 谁先到都行，两边装的是**同一份内容**（同一个 `provider_from_resources`），
/// 所以"已经装过"必须当成功，否则后到的那个会把整条启动链弄红。
pub fn install_resource_provider() -> Result<(), String> {
    match rustlua_infra::vfs::set_provider(provider_from_resources()) {
        Ok(()) => Ok(()),
        Err(error) => {
            // 只有"已安装"一种可能；内容相同，视作成功。
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[vfs] provider 已安装（{error}），沿用现有 provider"
            )));
            Ok(())
        }
    }
}

fn js_error_text(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// 平台存档槽的 wasm 实现：`localStorage` 上"一个键一个字符串"。
///
/// 为什么是 `localStorage` 而不是 IndexedDB：微信小游戏侧由 `wechat-adapter` 把
/// `localStorage` 映射到 `wx.setStorageSync`，所以**同一份代码覆盖两个平台**（浏览器 + 微信），
/// 与 web 旧运行时的做法一致。键名由引擎给出（`save_slot::storage_key`，与旧运行时逐字相同），
/// 这里只负责搬运字符串——序列化不在这层做。
struct LocalStorageSlot;

impl rustlua_infra::save_slot::SaveSlotProvider for LocalStorageSlot {
    fn read(&self, key: &str) -> Option<String> {
        let storage = web_sys::window()?.local_storage().ok()??;
        storage.get_item(key).ok()?
    }

    fn write(&self, key: &str, text: &str) -> Result<(), String> {
        // 拿不到 `localStorage` 是真失败，不能静默：Safari 隐私模式访问它直接抛 SecurityError，
        // 而"存了档其实没存上"比"存不上并报错"坏得多。
        let Some(storage) = web_sys::window()
            .and_then(|window| window.local_storage().ok())
            .flatten()
        else {
            return Err("localStorage 不可用".to_owned());
        };
        storage
            .set_item(key, text)
            .map_err(|error| format!("localStorage.setItem({key}): {}", js_error_text(&error)))
    }
}

/// 安装平台存档槽（**幂等**，与 [`install_resource_provider`] 同一套理由：可能有两个合法的先到者）。
///
/// 装了它之后，引擎自己的 `save.commit()`/`save.load()` 在 wasm 上才真的落盘——
/// 在此之前它们必然失败（`std::fs` 在 wasm 上没有写通道），也就是"玩家进度存不下来"。
pub fn install_storage_slot() -> Result<(), String> {
    match rustlua_infra::save_slot::set_provider(Box::new(LocalStorageSlot)) {
        Ok(()) => Ok(()),
        Err(error) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[save] 平台存档槽已安装（{error}），沿用现有 provider"
            )));
            Ok(())
        }
    }
}

type ResourceProvider = dyn Fn(&std::path::Path) -> Option<Vec<u8>> + 'static;

fn provider_from_resources() -> Box<ResourceProvider> {
    Box::new(|path: &std::path::Path| {
        let key = path
            .strip_prefix(super::VIRTUAL_PROJECT_ROOT)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
            .replace('\\', "/");
        LUA_RESOURCES.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|table| table.get(&key).cloned())
        })
    })
}

fn provider_from_reader(read_file: js_sys::Function) -> Box<ResourceProvider> {
    Box::new(move |path: &std::path::Path| {
        let key = path
            .strip_prefix(super::VIRTUAL_PROJECT_ROOT)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
            .replace('\\', "/");
        let value = read_file
            .call1(&JsValue::NULL, &JsValue::from_str(&key))
            .ok()?;
        value.as_string().map(|s| s.into_bytes())
    })
}

fn boot_runtime(provider: Box<ResourceProvider>) -> Result<(), JsValue> {
    // 与 Rust 侧运行时场景（`install_resource_provider`）共用同一条 setup：
    // 谁先安装都行，后到的当成功（内容相同，见该函数注释）。
    if let Err(error) = rustlua_infra::vfs::set_provider(provider) {
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[vfs] provider 已安装（{error}），沿用现有 provider"
        )));
    }
    let mut runtime = rustlua_scripting::script::LuaRuntime::new()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    runtime.set_project_root(std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT));
    LUA_RUNTIME.with(|slot| *slot.borrow_mut() = Some(runtime));
    // 新状态 = 新常驻输入表：旧差分基准会让"这一帧该松开哪些键"算错。
    INPUT_PREVIOUS.with(|slot| *slot.borrow_mut() = rustlua_frame_abi::FrameInput::new());
    // 新 Lua 世界 = 新时间基准：重新 boot 时丢掉累积器里的时间债（与 JS 侧 `resetClockNextFrame` 同义）。
    FIXED_CLOCK.with(|slot| *slot.borrow_mut() = None);
    Ok(())
}

/// 建 Lua 运行时（webgl：资源 provider 用 `start()` 预加载表）。
///
/// 注意：**不执行 `scripts/main.lua`**——web 侧流程是"先注入全部宿主补丁，
/// 再执行 main.lua"（与 fengari 版 `rustluaRunLuaGame` 同序）。`execute_project_script`
/// 内部的 `load_candidate` 会新建 Lua 状态（补丁会丢），因此 main.lua 由 JS 侧
/// 以 `lua_do_string(main, "scripts/main.lua")` 直接在既有状态上执行。
#[wasm_bindgen]
pub fn lua_boot() -> Result<(), JsValue> {
    super::install_panic_hook();
    boot_runtime(provider_from_resources())
}

/// 建 Lua 运行时（微信：资源 provider 用适配层同步读取器 `read_file`）。
#[wasm_bindgen]
pub fn lua_boot_with_reader(read_file: &js_sys::Function) -> Result<(), JsValue> {
    super::install_panic_hook();
    boot_runtime(provider_from_reader(read_file.clone()))
}

/// 在既有状态上执行 `scripts/main.lua`（web 侧在全部宿主补丁注入之后调用；
/// 读 vfs 预加载表 + `pack::decode` 自动解开压缩/加密，与桌面语义一致）。
///
/// **不能走 `execute_project_script`**：其内部 `load_candidate` 会 `Lua::new()` 新建
/// 状态并重跑 `register_api`，刚注入的 web 宿主补丁（`input`/`audio`/`__rustlua_*` 等）
/// 全部丢失。这里直接在运行时既有状态上 load + exec，保持补丁在位。
#[wasm_bindgen]
pub fn lua_execute_main() -> Result<(), JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let path = std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT).join("scripts/main.lua");
        let source = rustlua_scripting::script::read_project_script(&path)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        // 装 require 搜索器：mlua 默认搜索器走 C 文件 API，wasm 下无文件系统；
        // 与桌面 load_candidate 语义一致（vfs provider → pack::decode）。
        if let Some(script_dir) = path.parent() {
            rustlua_scripting::script::install_web_require(runtime.lua(), script_dir)
                .map_err(|e| JsValue::from_str(&format!("lua install require: {e}")))?;
        }
        runtime
            .lua()
            .load(&source)
            .set_name("scripts/main.lua")
            .exec()
            .map_err(|e| JsValue::from_str(&format!("lua scripts/main.lua: {e}")))
    })
}

/// 在既有状态上执行一段 Lua 代码（补丁/脚本；chunkname 用于报错定位）。
#[wasm_bindgen]
pub fn lua_do_string(source: &str, chunkname: &str) -> Result<(), JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        lua.load(source)
            .set_name(chunkname)
            .exec()
            .map_err(|e| JsValue::from_str(&format!("lua {chunkname}: {e}")))
    })
}

/// 把一个 Lua 全局设为数值（不经过词法/语法分析）。
///
/// 存在理由：`time.delta()` 的取值随"这一帧实际推进了多少模拟时间"变化——固定步循环里
/// 是固定步长，帧级阶段是 `steps × 固定步长`。此前只能靠 `lua_do_string("__rustlua_dt = …")`
/// 每帧（每个固定步）编译一小段源码来改这个数，那是纯浪费；这里直接写全局表。
#[wasm_bindgen]
pub fn lua_set_number(name: &str, value: f64) -> Result<(), JsValue> {
    /// 把**一批** Lua 全局设为数值（一次跨界；语义与逐个 `lua_set_number` 完全一致）。
    ///
    /// 存在理由：相机三元组每帧要写三个数，逐个写就是 3 次跨语言往返；它们同一瞬间、同一批语义，
    /// 合并成一次没有语义风险。**注意**：不同时间点的写入（例如固定步之前的 `dt`）不能并进来——
    /// 那会改变脚本读到的时机（脚本在推进前读 `dt`、在绘制时读相机值）。
    ///
    /// 入参是 `[[name, value], ...]` 的 JSON 数组；解析失败与设置失败都带具名错误，不做静默兜底。
    #[wasm_bindgen]
    pub fn lua_set_numbers(pairs_json: &str) -> Result<(), JsValue> {
        let pairs: Vec<(String, f64)> = serde_json::from_str(pairs_json)
            .map_err(|e| JsValue::from_str(&format!("lua set globals: bad json: {e}")))?;
        LUA_RUNTIME.with(|slot| {
            let guard = slot.borrow();
            let runtime = guard
                .as_ref()
                .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
            let globals = runtime.lua().globals();
            for (name, value) in pairs {
                globals
                    .set(name.as_str(), value)
                    .map_err(|e| JsValue::from_str(&format!("lua set global {name}: {e}")))?;
            }
            Ok(())
        })
    }
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        runtime
            .lua()
            .globals()
            .set(name, value)
            .map_err(|e| JsValue::from_str(&format!("lua set global {name}: {e}")))
    })
}

/// 每帧输入线格式（`RLFI` 批量 ABI）的布局与键表——**JS 打包侧唯一的偏移来源**。
///
/// 为什么由 Rust 给：偏移写错一个字在 JS 里不会报错，只会让"某个字段读到另一个字段的值"
/// （例如把 `scroll` 当 `cursor`），而这类缺陷在浏览器里的表现是"视角偶尔跳一下"，极难定位。
/// 让 JS 从这份 JSON 建 `DataView` 偏移表，两份代码就没有可漂移的余地。
/// 调用时机：boot 时一次，不参与每帧。
#[wasm_bindgen]
pub fn input_frame_layout_json() -> String {
    let offsets = serde_json::json!({
        "magic": rustlua_frame_abi::offsets::MAGIC,
        "version": rustlua_frame_abi::offsets::VERSION,
        "flags": rustlua_frame_abi::offsets::FLAGS,
        "frame": rustlua_frame_abi::offsets::FRAME,
        "axis": rustlua_frame_abi::offsets::AXIS,
        "cursor": rustlua_frame_abi::offsets::CURSOR,
        "press_cursor": rustlua_frame_abi::offsets::PRESS_CURSOR,
        "cursor_delta": rustlua_frame_abi::offsets::CURSOR_DELTA,
        "scroll": rustlua_frame_abi::offsets::SCROLL,
        "keys_held": rustlua_frame_abi::offsets::KEYS_HELD,
        "keys_pressed": rustlua_frame_abi::offsets::KEYS_PRESSED,
        "keys_released": rustlua_frame_abi::offsets::KEYS_RELEASED,
        "mouse_held": rustlua_frame_abi::offsets::MOUSE_HELD,
        "mouse_just": rustlua_frame_abi::offsets::MOUSE_JUST,
        "mouse_just_released": rustlua_frame_abi::offsets::MOUSE_JUST_RELEASED,
    });
    serde_json::json!({
        "magic": String::from_utf8_lossy(&rustlua_frame_abi::MAGIC),
        "version": rustlua_frame_abi::VERSION,
        "wire_len": rustlua_frame_abi::WIRE_LEN,
        "key_slots": rustlua_frame_abi::KEY_SLOTS,
        "mouse_slots": rustlua_frame_abi::MOUSE_SLOTS,
        "key_words": 2,
        "mouse_words": 1,
        "key_names": rustlua_frame_abi::KEY_NAMES,
        "mouse_names": rustlua_frame_abi::MOUSE_NAMES,
        "offsets": offsets,
    })
    .to_string()
}

/// 应用一帧批量输入（`RLFI` 定长二进制），**原地**改动常驻表 `__rustlua_input`。
///
/// 与旧协议（每帧 `lua_do_string("__rustlua_input = {…}")`）的语义必须逐项等价，两个易错点：
///
/// - **按住集合**：消失的位写 `false` 而不是删键——旧协议里 JS 的 `state.keys` 松开后保留
///   `false` 项，表里就是 `name = false`。改写成 `nil` 虽然对 `== true` 的读者等价，
///   但会让"键在不在表里"这件事与旧协议分叉，将来有人写 `pairs()` 遍历就会踩。
/// - **边沿集合**（`just`/`released`/鼠标边沿）：旧协议每帧新建表，上一帧的边沿**不存在**，
///   所以这里消失的位写 `nil`（真删），不写 `false`。
///
/// 返回本帧**改动的键/按钮位数量**：诊断面板用它显示"输入批次是否真的落地"
/// （恒为 0 说明宿主与 wasm 对"哪些位该变"的理解不一致），也是不同源问题的第一手线索。
#[wasm_bindgen]
pub fn lua_input_frame(bytes: &[u8]) -> Result<u32, JsValue> {
    let frame = rustlua_frame_abi::FrameInput::decode(bytes)
        .map_err(|e| JsValue::from_str(&format!("input frame: {e}")))?;
    // 位表之外的位 = 宿主与 wasm 不同源（JS 用了新键、wasm 还是旧表）。宁可响亮失败，
    // 也不要把未知位当成"没有这个键"悄悄吞掉——那会表现成"某个键按了没反应"。
    frame
        .validate_slots()
        .map_err(|e| JsValue::from_str(&format!("input frame: {e}")))?;
    // 存下这一帧：GPU 侧的统一模拟与这条 Lua 链必须吃同一份输入（见 `LATEST_FRAME`）。
    LATEST_FRAME.with(|slot| *slot.borrow_mut() = frame);
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let previous = INPUT_PREVIOUS.with(|slot| *slot.borrow());
        let touched = apply_input_frame(lua, &previous, &frame)
            .map_err(|e| JsValue::from_str(&format!("lua input frame: {e}")))?;
        INPUT_PREVIOUS.with(|slot| *slot.borrow_mut() = frame);
        Ok(touched)
    })
}

/// 把一帧输入落到 `__rustlua_input`：标量字段每帧都写（旧协议也是每帧都有值），
/// 位图只写**变化**的那几位，返回改动位数。
fn apply_input_frame(
    lua: &mlua::Lua,
    previous: &rustlua_frame_abi::FrameInput,
    frame: &rustlua_frame_abi::FrameInput,
) -> Result<u32, mlua::Error> {
    let root: mlua::Table = lua.globals().get("__rustlua_input")?;
    root.set("axis_x", frame.axis[0])?;
    root.set("axis_y", frame.axis[1])?;
    for (key, value) in [
        ("cursor", frame.cursor),
        ("press_cursor", frame.press_cursor),
        ("cursor_delta", frame.cursor_delta),
        ("scroll", frame.scroll),
    ] {
        // 嵌套的两项表是**常驻**的（旧协议每帧新建，这里改值）：表本身的身份不能变，
        // 否则 Lua 侧若有局部变量缓存了 `__rustlua_input.cursor`，会永远读到旧表。
        let pair = sub_table(&root, key)?;
        pair.set(1, value[0])?;
        pair.set(2, value[1])?;
    }
    let mut touched = 0u32;
    let keys = sub_table(&root, "keys")?;
    touched += apply_bits(
        &keys,
        rustlua_frame_abi::KEY_NAMES,
        previous.keys_held,
        frame.keys_held,
        false,
    )?;
    let just = sub_table(&root, "just")?;
    touched += apply_bits(
        &just,
        rustlua_frame_abi::KEY_NAMES,
        previous.keys_pressed,
        frame.keys_pressed,
        true,
    )?;
    let released = sub_table(&root, "released")?;
    touched += apply_bits(
        &released,
        rustlua_frame_abi::KEY_NAMES,
        previous.keys_released,
        frame.keys_released,
        true,
    )?;
    let mouse = sub_table(&root, "mouse")?;
    touched += apply_bits(
        &mouse,
        rustlua_frame_abi::MOUSE_NAMES,
        u128::from(previous.mouse_held),
        u128::from(frame.mouse_held),
        false,
    )?;
    let mouse_just = sub_table(&root, "mouse_just")?;
    touched += apply_bits(
        &mouse_just,
        rustlua_frame_abi::MOUSE_NAMES,
        u128::from(previous.mouse_just),
        u128::from(frame.mouse_just),
        true,
    )?;
    let mouse_up = sub_table(&root, "mouse_just_released")?;
    touched += apply_bits(
        &mouse_up,
        rustlua_frame_abi::MOUSE_NAMES,
        u128::from(previous.mouse_just_released),
        u128::from(frame.mouse_just_released),
        true,
    )?;
    Ok(touched)
}

/// 取 `__rustlua_input` 的某张常驻子表，取不到就**说清缺了哪一张**。
///
/// 为什么要专门报这一句：mlua 原生的报错是 `error converting Lua nil to table`，它指不出缺的是
/// `keys` 还是 `press_cursor`，而这类缺失只可能来自"预置表没声明全"——要修的地方在 Lua 预置表
/// （`web-runtime.js` 的 `__rustlua_input = {…}`），不在调用点。第一次实测就踩到了：
/// 出厂预置表漏了 `press_cursor`（旧协议每帧整份替换会把它补上，所以一直没暴露），
/// 原地改表才要求它一开始就在。
fn sub_table(root: &mlua::Table, key: &str) -> Result<mlua::Table, mlua::Error> {
    match root.get::<mlua::Value>(key)? {
        mlua::Value::Table(table) => Ok(table),
        _ => Err(mlua::Error::RuntimeError(format!(
            "__rustlua_input.{key} 不是常驻表：预置输入表必须声明 keys/just/released/mouse/\
mouse_just/mouse_just_released/cursor/press_cursor/cursor_delta/scroll 共 10 张子表"
        ))),
    }
}

/// 位图差分落地：新置的位写 `true`，消失的位按 `expire_to_nil` 写 `nil`（边沿）或 `false`（按住）。
///
/// 遍历的是**变化位**而不是整张表（`trailing_zeros` 逐个消位）：稳态帧一个位都不动，
/// 遍历 128 项的写法会把这笔开销变成每帧固定成本——那正是要消掉的东西。
fn apply_bits(
    table: &mlua::Table,
    names: &[&str],
    previous: u128,
    current: u128,
    expire_to_nil: bool,
) -> Result<u32, mlua::Error> {
    let mut touched = 0u32;
    let mut added = current & !previous;
    while added != 0 {
        let slot = added.trailing_zeros() as usize;
        added &= added - 1;
        if let Some(name) = names.get(slot) {
            table.set(*name, true)?;
            touched += 1;
        }
    }
    let mut removed = previous & !current;
    while removed != 0 {
        let slot = removed.trailing_zeros() as usize;
        removed &= removed - 1;
        if let Some(name) = names.get(slot) {
            if expire_to_nil {
                table.set(*name, mlua::Value::Nil)?;
            } else {
                table.set(*name, false)?;
            }
            touched += 1;
        }
    }
    Ok(touched)
}

/// 调用 Lua 全局函数。`args_json` 为 JSON 数组（string/number/boolean/null）。
/// 返回该全局的**字符串化结果**；全局不存在或返回非字符串（nil/boolean/table）
/// 返回 `None`（fengari `callGlobal` 同款协议）。
#[wasm_bindgen]
pub fn lua_call_global(name: &str, args_json: &str) -> Result<Option<String>, JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let value: mlua::Value = lua
            .globals()
            .get(name)
            .map_err(|e| JsValue::from_str(&format!("lua get global {name}: {e}")))?;
        let mlua::Value::Function(function) = value else {
            return Ok(None);
        };
        let args = parse_args_json(lua, args_json)?;
        // 必须是 `MultiValue`：`Vec<Value>` 会被 mlua 按 Serialize 当成**一个 table 参数**，
        // 于是 Lua 侧永远只收到 `...` = 一张表（实测 `n=1 type=table`，连 `[]` 都是）。
        // fengari 时代这里逐个 push，所以迁移后 JS→Lua 的带参调用（`__rustlua_export_state(false)`
        // 等）语义全变，但形状上不报错——只是 Lua 函数拿到的是表而不是值。
        let result: mlua::Value = function
            .call(mlua::MultiValue::from_vec(args))
            .map_err(|e| JsValue::from_str(&format!("lua call {name}: {e}")))?;
        Ok(mlua_value_to_string(result))
    })
}

/// 调用 **无参** Lua 全局函数并**丢弃返回值**。
///
/// 这是每步输入钩子（`__rustlua_apply_virtual_input` / `__rustlua_finish_step_input`）的形状：
/// 它们没有参数、也没有返回值。以前这两个钩子走 `lua_do_string("__rustlua_apply_virtual_input()")`
/// ——为了调一个无参函数**每步编译一小段源码**（一次分配 + 一次解释执行），而这段源码的内容
/// 永远不变。这里直接取全局函数调用，编译与分配一起消失，语义不变（Lua 侧那两份实现仍是唯一事实源）。
///
/// 全局**不是函数**时**报错**而不是静默返回：这两个钩子由预置层定义，缺席只可能是"预置层没跑到"
/// 或"宿主缓存了旧运行时"；静默吞掉会把这类发布故障降级成"虚拟按键偶尔不生效"这种难查的症状。
#[wasm_bindgen]
pub fn lua_call_global_void(name: &str) -> Result<(), JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let value: mlua::Value = lua
            .globals()
            .get(name)
            .map_err(|e| JsValue::from_str(&format!("lua get global {name}: {e}")))?;
        let mlua::Value::Function(function) = value else {
            return Err(JsValue::from_str(&format!(
                "lua call {name}: 全局不是函数（预置层没跑到，或宿主缓存了旧 web-runtime.js）"
            )));
        };
        let _: mlua::MultiValue = function
            .call(mlua::MultiValue::from_vec(Vec::new()))
            .map_err(|e| JsValue::from_str(&format!("lua call {name}: {e}")))?;
        Ok(())
    })
}

/// 注册 JS 回调为 Lua 全局（音频桥等 JS 宿主能力）。
///
/// JS 回调收 Lua 参数（string/number/boolean/nil → JS 基本值；其余 tostring），
/// 返回 **数组 = 多返回值**（音频桥 `[ok, reason]`），单值 = 单返回值。
#[wasm_bindgen]
pub fn lua_register_js_callback(name: &str, callback: &js_sys::Function) -> Result<(), JsValue> {
    let name_owned = name.to_owned();
    let index = JS_CALLBACKS.with(|slot| {
        let mut callbacks = slot.borrow_mut();
        callbacks.push(callback.clone());
        callbacks.len() - 1
    });
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let wrapped = lua
            .create_function(move |lua, args: mlua::MultiValue| {
                let js_fn = JS_CALLBACKS
                    .with(|slot| slot.borrow().get(index).cloned())
                    .ok_or_else(|| mlua::Error::RuntimeError("js callback slot lost".into()))?;
                let js_args = js_sys::Array::new();
                for arg in args.into_iter() {
                    js_args.push(&lua_value_to_js(lua, arg)?);
                }
                let result = js_fn.apply(&JsValue::NULL, &js_args).map_err(|e| {
                    mlua::Error::RuntimeError(format!("js callback {name_owned}: {e:?}"))
                })?;
                js_result_to_lua(lua, result)
            })
            .map_err(|e| JsValue::from_str(&format!("create_function {name}: {e}")))?;
        lua.globals()
            .set(name, wrapped)
            .map_err(|e| JsValue::from_str(&format!("set global {name}: {e}")))
    })
}

/// 返回宿主全局清单里**缺失**的名字（JSON 数组字符串）——`missingHostGlobals` 的替代。
#[wasm_bindgen]
pub fn lua_missing_globals(names_json: &str) -> Result<String, JsValue> {
    let names: Vec<String> = serde_json::from_str(names_json)
        .map_err(|e| JsValue::from_str(&format!("bad names json: {e}")))?;
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let mut missing = Vec::new();
        for name in names {
            let value: mlua::Value = lua
                .globals()
                .get(name.as_str())
                .map_err(|e| JsValue::from_str(&format!("lua get global {name}: {e}")))?;
            if matches!(value, mlua::Value::Nil) {
                missing.push(name);
            }
        }
        serde_json::to_string(&missing)
            .map_err(|e| JsValue::from_str(&format!("serialize missing globals: {e}")))
    })
}

/// 桌面侧的 Lua API 面基线，**编译期内嵌**进 wasm。
///
/// 这份文件同时被 `api_surface_snapshot` 的桌面快照门钉住（42 张模块表 / 281 个函数）。
/// 内嵌的理由是消灭手抄：`web-runtime.js` 原先自己列一份宿主全局名单，漏掉的名字既拿不到
/// 缺失代理、也不在诊断里露面，于是脚本以"取 nil"的姿态崩掉、根因看起来像脚本写错
///（`camera2d` 事故的同一类；实测这份手抄名单已经少 4 个真实模块表）。
const PC_API_BASELINE: &str =
    include_str!("../../rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json");

/// 桌面基线里的**模块表名字**清单（JSON 数组字符串）。
///
/// 静态数据（不读 Lua 状态），因此 `lua_boot` 之前就能调用；web 侧用它决定给哪些全局装
/// "自报家门"的缺失代理、在哪些真实宿主表上装缺失方法陷阱。
#[wasm_bindgen]
pub fn host_api_module_names_json() -> String {
    let parsed: serde_json::Value = serde_json::from_str(PC_API_BASELINE)
        .expect("内嵌的桌面 API 基线（lua_api_surface.json）解析失败");
    let tables = parsed["tables"]
        .as_array()
        .expect("内嵌基线的 tables 应为数组");
    let mut names: Vec<String> = tables
        .iter()
        .map(|table| {
            table["name"]
                .as_str()
                .expect("内嵌基线的模块名应为字符串")
                .to_owned()
        })
        .collect();
    names.sort();
    serde_json::to_string(&names).expect("模块名清单序列化失败")
}

/// 运行期真实 `_G` 的 API 表面（JSON 字符串），探针复用桌面快照门的 `LUA_API_PROBE`。
///
/// 用同一个常量而不是各写一份枚举逻辑，是这套一致性门的立足点：两侧"什么算一个 API"
/// 必须定义相同，否则对拍出来的差异全是枚举口径差异。
#[wasm_bindgen]
pub fn lua_api_surface_json() -> Result<String, JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let surface = runtime
            .eval_json(rustlua_scripting::script::LUA_API_PROBE)
            .map_err(|e| JsValue::from_str(&format!("lua api surface probe: {e}")))?;
        serde_json::to_string(&surface)
            .map_err(|e| JsValue::from_str(&format!("serialize api surface: {e}")))
    })
}

// ---- 值转换 ----

fn parse_args_json(lua: &mlua::Lua, json: &str) -> Result<Vec<mlua::Value>, JsValue> {
    // 空串 = **没有参数**。热路径上绝大多数全局调用是无参的（`update`、`__rustlua_export_ui`…），
    // 而空数组的 JSON 也是一个要**解析**的字符串：解析本身花时间，还在每次调用上产生一份小垃圾。
    // `[]` 依然接受（旧调用点与外部脚本可能这么写），空串只是更快的那条路。
    if json.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&format!("bad args json: {e}")))?;
    let serde_json::Value::Array(list) = parsed else {
        return Err(JsValue::from_str("lua bridge: args must be a JSON array"));
    };
    let mut values = Vec::with_capacity(list.len());
    for item in list {
        values.push(json_to_lua_value(lua, &item)?);
    }
    Ok(values)
}

fn json_to_lua_value(lua: &mlua::Lua, value: &serde_json::Value) -> Result<mlua::Value, JsValue> {
    match value {
        serde_json::Value::Null => Ok(mlua::Value::Nil),
        serde_json::Value::Bool(b) => Ok(mlua::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            // 整数走 Value::Integer：Lua 5.4 区分 integer/float，luaL_checkinteger
            // 只认 Integer，float 42.0 会报错（fengari 是 5.3 不区分，迁移后必须显式分）。
            if let Some(i) = n.as_i64() {
                Ok(mlua::Value::Integer(i))
            } else if let Some(u) = n.as_u64() {
                i64::try_from(u)
                    .map(mlua::Value::Integer)
                    .map_err(|_| JsValue::from_str("lua bridge: integer overflow in args"))
            } else {
                n.as_f64()
                    .map(mlua::Value::Number)
                    .ok_or_else(|| JsValue::from_str("lua bridge: non-finite number in args"))
            }
        }
        serde_json::Value::String(s) => lua
            .create_string(s.as_bytes())
            .map(mlua::Value::String)
            .map_err(|e| JsValue::from_str(&format!("create_string: {e}"))),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(JsValue::from_str(
            "lua bridge: nested JSON values are not supported in call args",
        )),
    }
}

/// fengari `callGlobal` 协议：只取字符串化结果（String/Number/Integer），其余 → None。
fn mlua_value_to_string(value: mlua::Value) -> Option<String> {
    match value {
        mlua::Value::String(s) => Some(s.to_string_lossy()),
        mlua::Value::Number(n) => Some(n.to_string()),
        mlua::Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Lua 值 → JS 基本值（音频桥参数是 string；其余类型 tostring 兜底）。
fn lua_value_to_js(lua: &mlua::Lua, value: mlua::Value) -> Result<JsValue, mlua::Error> {
    match value {
        mlua::Value::Nil => Ok(JsValue::NULL),
        mlua::Value::Boolean(b) => Ok(JsValue::from_bool(b)),
        // Lua 5.4 的 42 是 Integer 不是 Number；JS 侧统一按 f64 数值给。
        mlua::Value::Integer(i) => Ok(JsValue::from_f64(i as f64)),
        mlua::Value::Number(n) => Ok(JsValue::from_f64(n)),
        mlua::Value::String(s) => Ok(JsValue::from_str(&s.to_string_lossy())),
        other => Ok(JsValue::from_str(&value_tostring(lua, other)?)),
    }
}

/// JS 回调结果 → Lua 值：**数组 = 多返回值**；单值 = 单返回。
fn js_result_to_lua(lua: &mlua::Lua, result: JsValue) -> Result<mlua::MultiValue, mlua::Error> {
    if let Some(array) = result.dyn_ref::<js_sys::Array>() {
        let mut values = Vec::with_capacity(array.length() as usize);
        for item in array.iter() {
            values.push(js_value_to_lua(lua, item)?);
        }
        Ok(mlua::MultiValue::from_vec(values))
    } else {
        Ok(mlua::MultiValue::from_vec(vec![js_value_to_lua(
            lua, result,
        )?]))
    }
}

fn js_value_to_lua(lua: &mlua::Lua, value: JsValue) -> Result<mlua::Value, mlua::Error> {
    if value.is_null() || value.is_undefined() {
        return Ok(mlua::Value::Nil);
    }
    if let Some(b) = value.as_bool() {
        return Ok(mlua::Value::Boolean(b));
    }
    if let Some(n) = value.as_f64() {
        // JS 数值都是 f64：整数值转 Integer（Lua 5.4 区分，整数运算/checkinteger 需要）。
        if n.fract() == 0.0 && n >= -(2f64.powi(63)) && n < 2f64.powi(63) {
            return Ok(mlua::Value::Integer(n as i64));
        }
        return Ok(mlua::Value::Number(n));
    }
    if let Some(s) = value.as_string() {
        return lua.create_string(s.as_bytes()).map(mlua::Value::String);
    }
    // 对象/函数等：tostring 兜底（音频桥只返回 boolean/string/数组）。
    let text = js_error_text(&value);
    lua.create_string(text.as_bytes()).map(mlua::Value::String)
}

fn value_tostring(lua: &mlua::Lua, value: mlua::Value) -> Result<String, mlua::Error> {
    match value {
        mlua::Value::String(s) => Ok(s.to_string_lossy()),
        mlua::Value::Number(n) => Ok(n.to_string()),
        mlua::Value::Boolean(b) => Ok(b.to_string()),
        mlua::Value::Nil => Ok("nil".to_owned()),
        other => Ok(lua
            .coerce_string(other)?
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|| "<unprintable>".to_owned())),
    }
}
