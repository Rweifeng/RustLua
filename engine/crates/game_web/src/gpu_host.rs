//! Web 宿主的 GPU 渲染第一片：用**引擎自己的渲染器**把一帧画到画布上。
//!
//! ## 这一步在补什么
//!
//! M73-00 之前，web/微信上的画面全部由 `plugins/webgl/web-runtime.js` 的 Canvas2D 路径画出来，
//! `game_web` 里**连 `rustlua_gpu::Renderer` 都没有**——引擎那台按 WebGPU 规格写的渲染器
//! 在浏览器里从未被宿主用过。WP5 前四片把这条路的每个前提单独验通了（能建、能按期望值画对、
//! 内存贴图能交进来、能渲进画布纹理并 present），本片把它们**接起来**。
//!
//! ## P1 第一片之后：这一层只剩"外壳"
//!
//! GPU 栈搭建、渲染器、场景 boot、每帧 step/mesh/贴图/取景/render/present 全部搬进
//! [`crate::engine_core::EngineCore`]（与微信引擎宿主 `host_runtime` 共用的那份核心）。
//! 本文件保留的是**只有浏览器才有的东西**：rAF、`fit_now` 两层几何对齐、DOM 输入采集、
//! URL 开关、`data-rustlua-gpu` 状态属性。抽取是行为等价的：读数文案与帧序逐字沿用。
//!
//! ## 为什么是 Rust 侧的 URL 开关
//!
//! 默认路径（Canvas2D）一行都不动：GPU 路径只在 `?rustlua-gpu=1` 时才建，建成与否写进
//! `document.body` 的 `data-rustlua-gpu`，供门（`scripts/verify/verify-webgl-bundle.ps1 -Gpu2d`）
//! 与人工排查读。放在 Rust 侧而不是页面脚本里，是为了**不碰** `plugins/webgl/` 下的既有改动。
//!
//! ## 已实测钉住的平台事实（WP5 第四片，核心沿用同一口径）
//!
//! - WebGL2 画布提供的格式里就有 `Rgba8UnormSrgb`——**正是**渲染器构造时烘死的那个，
//!   所以不需要为 web 做格式参数化；不提供时必须**响亮失败**，不能往不匹配的 surface 上渲。
//! - 画布纹理只支持 `RENDER_ATTACHMENT`：**不要**申请 `COPY_SRC`。申请了 wgpu 会在**未捕获**的
//!   校验错误上 panic，而 wasm 是 `panic=abort`——整个模块会死，现场只剩一句
//!   `Uncaught RuntimeError: unreachable`。

use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

use crate::engine_host::EngineHost;
use crate::host_config::HostConfig;
use crate::rust_scene::SimRoute;

/// 状态属性每这么多帧改写一次：既是心跳，也让"跑到一半被抓拍"的 dump 仍有可读读数。
const STATUS_EVERY: u32 = 30;
const MAX_FRAME_WORK_SAMPLES: usize = 4096;
const SIM_UPDATES_PER_SECOND: u32 = 60;
const SIM_MAX_CATCH_UP_STEPS: u32 = 8;
const SIM_MAX_FRAME_SECONDS: f64 = 0.25;

/// 浏览器宿主的时钟配置：三个值与上面的 `SIM_*` 常量逐字同值（抽取前的取值），
/// 只是现在由 [`crate::engine_host::EngineHost`] 持有——微信侧那份由 `engine_host_begin`
/// 的 options 给。**两边的默认值必须是同一个数**，这条由 `host_config` 的单测守着。
const fn web_host_config() -> HostConfig {
    HostConfig::new(
        SIM_UPDATES_PER_SECOND,
        SIM_MAX_CATCH_UP_STEPS,
        SIM_MAX_FRAME_SECONDS,
    )
}
/// 画布尺寸取 2D 设计分辨率的一半做离屏目标不合适：这里直接按画布实际尺寸渲染。
const FALLBACK_SIZE: u32 = 256;

fn percentile_ms(samples: &[f64], percentile: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (sorted.len() as f64 * percentile).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// 帧预算仅供自动化验证；产品默认持续运行。URL `rustlua-gpu-frames=<n>` 可设定上限，`0` 表示无限。
fn frame_budget() -> Option<u32> {
    let Some(window) = web_sys::window() else {
        return None;
    };
    let Some(search) = window.location().search().ok() else {
        return None;
    };
    const PREFIX: &str = "rustlua-gpu-frames=";
    let Some(start) = search.find(PREFIX) else {
        return None;
    };
    let rest = &search[start + PREFIX.len()..];
    let end = rest.find('&').unwrap_or(rest.len());
    rest[..end]
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|frames| *frames > 0)
}

/// URL `rustlua-input-script=<键:起帧-止帧[,键:起帧-止帧]>`：确定性的输入剧本。
///
/// 为什么用 URL 而不是"探针按键"：探针按键的帧号取决于事件循环调度，同一份产物两次跑
/// 落点不同；而世界读数（主角位移）要靠帧号对齐才有可比性。URL 剧本让"第 N 帧按住什么"
/// 完全确定 ⇒ 它可以当门禁，也可以当性能基线（固定输入 = 固定负载）。
fn input_script_spec() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    url_value(&search, "rustlua-input-script=")
}

/// URL `rustlua-world-expr=<Lua>`：随状态属性报出来的**只读**工程状态查询
/// （见 [`GpuHost::world_expr`]）。表达式必须只读——它跑在工程同一个 Lua 状态里。
fn world_expr_spec() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    url_value(&search, "rustlua-world-expr=")
}

/// URL `rustlua-world-do=<Lua>`：**只跑一次**、允许有副作用的探针（见 [`GpuHost::world_do`]）。
fn world_do_spec() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    url_value(&search, "rustlua-world-do=")
}

/// 取 URL query 里 `prefix` 后面的那一段（到 `&` 为止），`decodeURIComponent` 之后返回。
///
/// 转义是必须的：剧本里有 `,`/`:`/`-`，但**逗号在某些拼 URL 的地方会被编码**，
/// 不还原就会出现"剧本看起来对、解析却报错"。空值按"没给"处理。
fn url_value(search: &str, prefix: &str) -> Option<String> {
    let start = search.find(prefix)? + prefix.len();
    let rest = &search[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    let raw = &rest[..end];
    if raw.is_empty() {
        return None;
    }
    let decoded = js_sys::decode_uri_component(raw)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| raw.to_owned());
    Some(decoded)
}

type SharedInput = std::rc::Rc<std::cell::RefCell<InputShared>>;

struct LifecycleEvents {
    document: web_sys::Document,
    visible: std::rc::Rc<std::cell::Cell<bool>>,
    resume_generation: std::rc::Rc<std::cell::Cell<u64>>,
    input_shared: std::rc::Rc<std::cell::RefCell<Option<SharedInput>>>,
    listener: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
}

impl LifecycleEvents {
    fn install(document: &web_sys::Document) -> Result<Self, String> {
        use wasm_bindgen::JsCast;

        let visible = std::rc::Rc::new(std::cell::Cell::new(!document.hidden()));
        let resume_generation = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let input_shared: std::rc::Rc<std::cell::RefCell<Option<SharedInput>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let visible_for_listener = visible.clone();
        let generation_for_listener = resume_generation.clone();
        let input_for_listener = input_shared.clone();
        let document_for_listener = document.clone();
        let listener = wasm_bindgen::closure::Closure::wrap(Box::new(move |_event| {
            let is_visible = !document_for_listener.hidden();
            if visible_for_listener.replace(is_visible) != is_visible {
                if is_visible {
                    generation_for_listener.set(generation_for_listener.get().saturating_add(1));
                    crate::audio_sink::resume_music_for_lifecycle();
                } else {
                    if let Some(shared) = input_for_listener.borrow().as_ref() {
                        shared.borrow_mut().producer.release_all();
                    }
                    crate::audio_sink::pause_for_lifecycle();
                }
            }
        })
            as Box<dyn FnMut(web_sys::Event)>);
        document
            .add_event_listener_with_callback("visibilitychange", listener.as_ref().unchecked_ref())
            .map_err(|error| format!("visibilitychange listener: {error:?}"))?;
        Ok(Self {
            document: document.clone(),
            visible,
            resume_generation,
            input_shared,
            listener,
        })
    }

    fn set_input_shared(&self, shared: SharedInput) {
        *self.input_shared.borrow_mut() = Some(shared);
    }

    fn is_visible(&self) -> bool {
        self.visible.get()
    }

    fn resume_generation(&self) -> u64 {
        self.resume_generation.get()
    }
}

impl Drop for LifecycleEvents {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;

        let _ = self.document.remove_event_listener_with_callback(
            "visibilitychange",
            self.listener.as_ref().unchecked_ref(),
        );
    }
}

/// GPU 宿主**外壳**：渲染链在 [`crate::engine_core::EngineCore`] 里，这里只剩浏览器项
/// （rAF 时钟、两层几何、DOM 输入、世界读数探针）。
struct GpuHost {
    /// 统一宿主：**时钟与共享核心都在它里面**（P1 第二片）。
    ///
    /// 抽取前这里是 `core: EngineCore` + `simulation_clock: FrameClock` 两个字段：时钟由外壳
    /// 自持、计划由外壳自算，而微信那份外壳又写了一模一样的一遍。现在"跑几步、步长多少、
    /// 这一帧画什么"只有 [`crate::engine_host::EngineHost::frame`] 一份。
    host: EngineHost,
    lifecycle: LifecycleEvents,
    /// 本层画布（GPU 那张）与**游戏画布**（Canvas2D 那张）。后者只用来读窗口尺寸：
    /// `web-runtime.js` 会把游戏画布的后备缓冲跟到窗口（含 DPR），本层必须与它同框。
    canvas: web_sys::HtmlCanvasElement,
    game_canvas: web_sys::HtmlCanvasElement,
    last_animation_frame_ms: Option<f64>,
    /// 最近一次适配读数（进 `note`，无头驱动据此断言两层同框）。
    fit_note: String,
    /// 最近一次写进画布 style 的几何（只在变化时写 DOM，避免每帧刷样式）。
    fit_css: String,
    /// 外壳拼出来的场景读数（`rust {stats} scene_delta=…`；降级时回落到核心的 `sim_note`）。
    sim_note: String,
    /// **Rust 侧自己的输入生产者**（WP6-04）。此前这一层只有消费者：JS 世界采集事件、编码成
    /// `RLFI`、经 `lua_input_frame` 写进常驻表，宿主再读出来——于是"没有 JS 世界就没有输入"。
    /// 有了它，Rust 那条路自己就能被玩家操作（退休清单第一项的落地）。
    ///
    /// 用 `Rc<RefCell<…>>` 是因为监听器闭包要活到宿主结束，而闭包不能借用宿主自己；
    /// wasm 单线程，这是这里该用的东西。
    input_shared: Option<SharedInput>,
    /// 本帧要应用给模拟的输入（Rust 采集的 + JS 世界送来的并集）。
    frame_input: rustlua_frame_abi::FrameInput,
    /// 上一帧消费到的线格式帧号：用来判"另一路本帧是否真的产过帧"（[`crate::frame_input::wire_is_fresh`]）。
    /// 没有它就只能拿"本侧没动过"当判据，而那会在没有指针事件的帧上抹掉活光标（现场=摇杆圆点闪烁）。
    last_wire_frame: u32,
    /// 收到的 DOM 事件数（键 + 指针）。**读数用它证明输入真的到了 Rust 侧**——
    /// 没有这个数，"按了没反应"只能靠猜是没收到事件、还是收到了却没落到模拟里。
    dom_events: u64,
    /// 监听器闭包要活到宿主结束，否则被 drop 后事件静默不再到达。
    input_closures: Vec<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>>,
    /// **确定性输入剧本**（WP6-05）：按帧号按住/松开某些键，用来验"输入进去之后世界真的会变"。
    ///
    /// 与 DOM 采集**不是二选一**：DOM 那条证明"事件能进来"，这条证明"进来之后世界改变了"
    /// （位移读数是判据）。按键的帧号确定 ⇒ 同一份产物跑到第 N 帧必然同一个世界状态 ⇒ 能当门禁。
    input_script: crate::frame_input::ScriptDriver,
    /// 输入剧本的解析结果说明（空 = 没给剧本）。**解析失败必须可见**——
    /// 静默丢掉一段剧本，现象就是"按了没反应"，正是本片要消灭的那类失败。
    input_script_note: String,
    /// URL `rustlua-world-expr=<Lua>`：随状态属性一起报出来的**只读**工程状态查询。
    ///
    /// 为什么需要它：`scene_delta=` 这类输出差值证明不了"是哪个逻辑动了"——待机动画也会让它动
    /// （本轮实测：不按任何有效键，`q` 对照组的顶点差就有 +60）。工程自己的状态（模式机、背包、
    /// 任务、存档）才是干净判据，而它只有工程自己知道，所以把"问什么"交给驱动。
    world_expr: Option<String>,
    /// 上面那条查询的结果（`expr=` 读数，空 = 没给查询）。
    world_expr_note: String,
    /// URL `rustlua-world-do=<Lua>`：**只跑一次**、允许有副作用的探针（`do=` 读数）。
    ///
    /// 与 `world_expr` 分开命名是刻意的：读数必须只读，动作必须显式。存档这类验证需要写能力
    /// （`save.write` → `save.commit` → `save.clear` → `save.load`），只有它做得到。
    world_do: Option<String>,
    /// 探针结果（空 = 没给探针）。
    world_do_note: String,
    /// 探针是否已跑过——**只跑一次**：反复跑会把副作用叠起来（它等价于一条控制台命令）。
    world_do_done: bool,
}

/// 输入监听器与宿主之间共享的那一份状态。
struct InputShared {
    producer: crate::frame_input::FrameInputProducer,
    /// 等比适配盒在 **CSS 像素**里的原点与尺寸，由 `fit_now` 每帧刷新（与写进画布 style 的
    /// 是同一套算式）。光标换算与绘制几何因此永远同源——各算一次是"点哪儿都不对"的经典成因。
    map: [f64; 4],
    /// 收到的 DOM 事件数（读数用）。
    events: u64,
    /// 临时遥测（排查"触摸抬手后摇杆不回弹"）：多指 down/up 各收到几次 + 非多指通路的鼠标 down/up。
    /// 真机读数就靠它回答"抬手事件到底有没有到宿主"。定位完删。
    touch_downs: u64,
    touch_ups: u64,
    mouse_downs: u64,
    mouse_ups: u64,
}

impl Default for InputShared {
    fn default() -> Self {
        let (design_w, design_h) = crate::fit::design_size();
        Self {
            producer: crate::frame_input::FrameInputProducer::new(),
            // 还没对齐几何前，退化成"CSS 像素即设计像素"（第一帧之前不会有真实操作）。
            map: [0.0, 0.0, design_w as f64, design_h as f64],
            events: 0,
            touch_downs: 0,
            touch_ups: 0,
            mouse_downs: 0,
            mouse_ups: 0,
        }
    }
}

/// 把一次指针事件的 CSS 坐标换算成设计坐标并交给生产者。
fn point_into(state: &std::rc::Rc<std::cell::RefCell<InputShared>>, css_x: f64, css_y: f64) {
    let mut shared = state.borrow_mut();
    let [left, top, width, height] = shared.map;
    let point = crate::fit::css_to_design([left, top, width, height], [css_x, css_y]);
    shared.producer.pointer_moved(point);
    shared.events = shared.events.saturating_add(1);
}

/// css → 设计坐标（与 `point_into` 同一套换算）。多指的每指通路要先拿到这个点，才能按 id 记下来。
fn design_point(
    state: &std::rc::Rc<std::cell::RefCell<InputShared>>,
    css_x: f64,
    css_y: f64,
) -> [f64; 2] {
    let [left, top, width, height] = state.borrow().map;
    crate::fit::css_to_design([left, top, width, height], [css_x, css_y])
}

/// 写 `data-rustlua-gpu`（阶段自报）＋ 一条控制台行。
///
/// **每帧那条不打控制台**（2026-10-17）：`presented frames=…` 是通过本函数每帧发一次的**直接
/// console 调用**（不走 `tracing`，所以 `frame_log_filter` 管不到它）——一次加载实测 236 条，
/// 是把控制台刷屏的最大一处。属性照写（门要读 `data-rustlua-gpu="presented…“`），
/// 控制台那行由 `?rustlua-debug=1`（= `set_frame_logs(true)`）打开。
fn set_status(state: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    if let Some(body) = document.body() {
        let _ = body.set_attribute("data-rustlua-gpu", state);
    }
    // 判据按**实况前缀**写：每帧那条从这里收到的是 `presented …`（调用点拼的），
    // 启动期那几条是 `initializing` / `sim_boot_ok` / `textures_done=…` —— 后者必须照发。
    if !crate::frame_log_filter::frame_logs_enabled() && state.starts_with("presented") {
        return;
    }
    web_sys::console::log_1(&JsValue::from_str(&format!("[gpu] {state}")));
}

/// 等待浏览器下一次绘制机会；返回 requestAnimationFrame 的单调时间戳。
async fn next_animation_frame() -> Result<f64, String> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let Some(window) = web_sys::window() else {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("window unavailable"));
            return;
        };
        if let Err(error) = window.request_animation_frame(&resolve) {
            let _ = reject.call1(&JsValue::NULL, &error);
        }
    });
    let timestamp = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|error| format!("requestAnimationFrame: {error:?}"))?;
    timestamp
        .as_f64()
        .ok_or_else(|| "requestAnimationFrame returned a non-numeric timestamp".to_owned())
}

/// URL 位 `rustlua-sim=<名字>`：选模拟来源（见 [`crate::rust_scene::SimRoute`]）。
///
/// 默认路线是 Rust 引擎加载启动场景并执行工程脚本。隔离渲染诊断仍可显式选择 `scene-only`。
///
/// 取值：`script`（引擎自己加载启动场景 + 跑脚本）、`scene-script`（宿主给场景字节 + 跑脚本）、
/// `scene-only`（宿主给场景字节、不跑脚本）；缺省等同 `script`。
fn sim_route() -> SimRoute {
    let name = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| {
            search
                .split(['?', '&'])
                .find_map(|pair| pair.strip_prefix("rustlua-sim=").map(str::to_owned))
        })
        .unwrap_or_else(|| "script".to_owned());
    SimRoute::from_name(&name)
}

/// URL 位 `rustlua-sim-lines=N`：**把工程脚本截断成前 N 行**再让引擎去跑。
///
/// 为什么需要它：工程脚本是在引擎侧同步执行的，宿主看不到它跑到哪一步，但宿主**拿着它的字节**
/// （预加载资源表就在宿主手里）。于是"脚本卡在哪一段"变成一个机械的二分问题：先只给前 1 行
/// （空块）、再给前 80 行（全部 `require`）、……直到复现。这比在引擎里逐点插桩便宜，
/// 也不需要为了诊断改引擎。
fn sim_script_lines() -> Option<usize> {
    let raw = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| {
            search
                .split(['?', '&'])
                .find_map(|pair| pair.strip_prefix("rustlua-sim-lines=").map(str::to_owned))
        })?;
    raw.parse::<usize>().ok().filter(|lines| *lines > 0)
}

/// 按 `rustlua-sim-lines` 截断工程脚本；返回一句可写进状态属性的说明。
///
/// 覆盖成**明文**是安全的：`read_project_script` 走 `pack::decode`，而它对明文原样透传。
fn apply_script_truncation() -> Option<String> {
    let lines = sim_script_lines()?;
    let decoded = crate::lua_bridge::resource_png("scripts/main.lua")?;
    let text = String::from_utf8_lossy(&decoded);
    let kept = text
        .lines()
        .take(lines)
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    let replaced = crate::lua_bridge::override_resource("scripts/main.lua", kept.clone());
    Some(format!(
        "script_lines={lines} 表里原本{}这一项 bytes={}",
        if replaced { "有" } else { "没有" },
        kept.len()
    ))
}

fn canvas_size(canvas: &web_sys::HtmlCanvasElement) -> (u32, u32) {
    let width = canvas.width();
    let height = canvas.height();
    if width == 0 || height == 0 {
        (FALLBACK_SIZE, FALLBACK_SIZE)
    } else {
        (width, height)
    }
}

/// 等比适配盒（算法在 `crate::fit`，与渲染器同一套；这里有原生单测守着）。
fn contain_box(box_w: u32, box_h: u32) -> (u32, u32) {
    let (design_w, design_h) = crate::fit::design_size();
    crate::fit::contain_box(box_w, box_h, design_w, design_h)
}

impl GpuHost {
    /// 建核心（GPU 栈 + 渲染器 + 场景，全在 [`crate::engine_core::EngineCore`]）+ 外壳自己的
    /// 时钟与读数。任何一步失败都要能说出**是哪一步**（wasm 上 panic 不给回溯）。
    async fn new(
        canvas: &web_sys::HtmlCanvasElement,
        game_canvas: &web_sys::HtmlCanvasElement,
        scene_path: &str,
        lifecycle: LifecycleEvents,
    ) -> Result<Self, String> {
        let (width, height) = canvas_size(canvas);
        // 输入剧本：解析一次就定了。**失败要可见**——静默当成"没给剧本"会让"按了没反应"
        // 变成一个查不出来的现象（本片要消灭的正是这类失败）。
        let (input_script, input_script_note) = match input_script_spec() {
            None => (crate::frame_input::InputScript::default(), String::new()),
            Some(spec) => match crate::frame_input::InputScript::parse(&spec) {
                Ok(script) => (script, format!(" input_script=\"{spec}\"")),
                Err(error) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[gpu] 输入剧本解析失败：{error}"
                    )));
                    (
                        crate::frame_input::InputScript::default(),
                        format!(" input_script_error=\"{error}\""),
                    )
                }
            },
        };
        let route = sim_route();
        let truncation = apply_script_truncation();
        // 阶段自报走 `set_status`（`data-rustlua-gpu` 属性 + 控制台行）：文案与抽取前逐字相同，
        // 无头驱动的门断言不需要改。
        let host = EngineHost::build(
            crate::engine_core::CoreArgs {
                canvas: &JsValue::from(canvas.clone()),
                width,
                height,
                scene_path,
                route,
                route_note: truncation.unwrap_or_default(),
                device_label: "rustlua-web-gpu-device",
                report: Box::new(set_status),
            },
            // 时钟参数与抽取前逐字同值（`SIM_*` 三个模块常量）；现在时钟由宿主内部持有，
            // 外壳不再自己算"这一帧该跑几步"。
            web_host_config(),
        )
        .await?;
        // 启动耗时是一条独立的控制台行：状态属性会被后续读数覆盖，事后从 dump 里读不到就没了。
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[gpu] sim_boot_ms={:.1} note={}",
            host.core.boot_ms, host.core.sim_note
        )));
        let sim_note = host.core.sim_note.clone();
        Ok(Self {
            host,
            lifecycle,
            canvas: canvas.clone(),
            game_canvas: game_canvas.clone(),
            last_animation_frame_ms: None,
            fit_note: String::new(),
            fit_css: String::new(),
            sim_note,
            input_shared: None,
            frame_input: rustlua_frame_abi::FrameInput::new(),
            last_wire_frame: 0,
            dom_events: 0,
            input_closures: Vec::new(),
            input_script: crate::frame_input::ScriptDriver::new(input_script),
            input_script_note,
            world_expr: world_expr_spec(),
            world_expr_note: String::new(),
            world_do: world_do_spec(),
            world_do_note: String::new(),
            world_do_done: false,
        })
    }

    /// 装上 Rust 侧的 DOM 输入监听器（退休清单第一项：**没有 JS 世界也要能被操作**）。
    ///
    /// 三条设计取舍：
    ///
    /// 1. **监听 window 而不是画布**。键盘只能挂 window；指针也挂 window 是为了
    ///    "在画布外按下、拖进画布"这类操作不丢事件。落点是否在内容盒内由几何换算自然处理。
    /// 2. **事件只攒进生产者，不做任何逻辑**。边沿/按住/同帧点按的语义全在
    ///    [`crate::frame_input::FrameInputProducer`] 里，与"线格式 → 快照"那一层共用同一份测试。
    ///    在监听器里判逻辑，就会出现"宿主一套、`apply_frame` 又一套"的老毛病。
    /// 3. **闭包存进 `self.input_closures`**：wasm 里闭包被 drop 后监听器静默失效，
    ///    症状是"启动时能按、过一会儿就不能按"。
    fn install_input_listeners(&mut self) {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;

        let Some(window) = web_sys::window() else {
            return;
        };
        let shared = std::rc::Rc::new(std::cell::RefCell::new(InputShared::default()));

        // 先攒成一个 `Vec`，最后一次性搬进 `self`：边装边往 `self` 里塞会让借用检查器
        // 在闭包捕获 `self` 的同一段里再要求 `&mut self`。
        let mut closures: Vec<Closure<dyn FnMut(web_sys::Event)>> = Vec::new();
        let mut listen = |event: &str, handler: Box<dyn FnMut(web_sys::Event)>| {
            let closure = Closure::wrap(handler);
            let _ =
                window.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
            closures.push(closure);
        };

        let state = shared.clone();
        listen(
            "keydown",
            Box::new(move |event: web_sys::Event| {
                if let Some(key) = event.dyn_ref::<web_sys::KeyboardEvent>() {
                    let mut shared = state.borrow_mut();
                    shared.producer.key_down(&key.key());
                    shared.events = shared.events.saturating_add(1);
                }
            }),
        );
        let state = shared.clone();
        listen(
            "keyup",
            Box::new(move |event: web_sys::Event| {
                if let Some(key) = event.dyn_ref::<web_sys::KeyboardEvent>() {
                    let mut shared = state.borrow_mut();
                    shared.producer.key_up(&key.key());
                    shared.events = shared.events.saturating_add(1);
                }
            }),
        );
        let state = shared.clone();
        listen(
            "pointermove",
            Box::new(move |event: web_sys::Event| {
                if let Some(mouse) = event.dyn_ref::<web_sys::MouseEvent>() {
                    point_into(&state, mouse.client_x() as f64, mouse.client_y() as f64);
                    // **触摸/笔走每指通路**（只更新那根手指的当前位置，不动它的按下点）。
                    if let Some(pointer) = event.dyn_ref::<web_sys::PointerEvent>()
                        && pointer.pointer_type() != "mouse"
                    {
                        let id = pointer.pointer_id().max(0) as u64;
                        let point =
                            design_point(&state, mouse.client_x() as f64, mouse.client_y() as f64);
                        state.borrow_mut().producer.pointer_move(id, point);
                    }
                }
            }),
        );
        for (event_name, is_down) in [("pointerdown", true), ("pointerup", false)] {
            let state = shared.clone();
            listen(
                event_name,
                Box::new(move |event: web_sys::Event| {
                    let Some(mouse) = event.dyn_ref::<web_sys::MouseEvent>() else {
                        return;
                    };
                    // **触摸/笔：每指一条记录，且不碰 `mouse_*` 位**（触摸不是鼠标键）。
                    //
                    // 分流依据是 `pointerType`，不是"再加一套 touch 监听"：真机上 touchstart 之后
                    // 浏览器还会合成 mouse 事件，两套监听会把同一根手指记成两根（`pointer_count`
                    // 凭空 +1，而"两指还是三指"正是这一层要报的东西）。
                    if let Some(pointer) = event.dyn_ref::<web_sys::PointerEvent>()
                        && pointer.pointer_type() != "mouse"
                    {
                        let id = pointer.pointer_id().max(0) as u64;
                        let point =
                            design_point(&state, mouse.client_x() as f64, mouse.client_y() as f64);
                        let mut shared = state.borrow_mut();
                        if is_down {
                            shared.producer.pointer_down(id, point);
                            shared.touch_downs = shared.touch_downs.saturating_add(1);
                        } else {
                            shared.producer.pointer_up(id);
                            shared.touch_ups = shared.touch_ups.saturating_add(1);
                        }
                        shared.events = shared.events.saturating_add(1);
                        return;
                    }
                    // 临时遥测：走了鼠标分支的 pointer down/up 各记一笔（读数里叫 `ptrM`）。
                    {
                        let mut shared = state.borrow_mut();
                        if is_down {
                            shared.mouse_downs = shared.mouse_downs.saturating_add(1);
                        } else {
                            shared.mouse_ups = shared.mouse_ups.saturating_add(1);
                        }
                    }
                    point_into(&state, mouse.client_x() as f64, mouse.client_y() as f64);
                    let button = match mouse.button() {
                        1 => "middle",
                        2 => "right",
                        _ => "left",
                    };
                    let mut shared = state.borrow_mut();
                    if is_down {
                        shared.producer.mouse_down(button);
                    } else {
                        shared.producer.mouse_up(button);
                    }
                }),
            );
        }
        let state = shared.clone();
        listen(
            "wheel",
            Box::new(move |event: web_sys::Event| {
                if let Some(wheel) = event.dyn_ref::<web_sys::WheelEvent>() {
                    state
                        .borrow_mut()
                        .producer
                        .scrolled([wheel.delta_x(), wheel.delta_y()]);
                }
            }),
        );

        self.lifecycle.set_input_shared(shared.clone());
        self.input_shared = Some(shared);
        self.input_closures = closures;
    }

    /// 刷新世界读数（只在宿主即将写状态属性时调用，见帧循环里的调用点）。
    ///
    /// 包一层而不是让调用方直接摸 sim：读数属于**观测**，观测不该出现在热路径上。
    fn refresh_world_readout(&mut self) {
        let Some(sim) = self.host.core.sim_mut() else {
            return;
        };
        // 顺序：先动作、再读数。这样同一帧的属性里，"做完之后世界是什么样"是自洽的
        // （动作只跑一次，读数每帧都新鲜）。
        if !self.world_do_done
            && let Some(source) = self.world_do.clone()
        {
            self.world_do_done = true;
            self.world_do_note = format!(" do={}", sim.eval_world_do(&source));
        }
        sim.refresh_world();
        // 自定义查询与内置世界读数在**同一处**刷新：两者都是"给人/给驱动看的观测"，
        // 都只能在写属性那一帧花这份钱（见 `RustScene::refresh_world` 里的实测记录）。
        if let Some(expression) = self.world_expr.clone() {
            self.world_expr_note = format!(" expr={}", sim.eval_world_expr(&expression));
        }
    }

    /// 每帧开头对齐两层几何：本层画布 = **游戏画布里的等比适配盒**。
    ///
    /// 为什么必须做：Canvas2D 层（`web-runtime.js`）每帧把 1280×720 设计空间 contain 进
    /// 游戏画布（黑边居中），而本层此前是"固定 1280×720 后备缓冲 + CSS 铺满视口"——
    /// 窗口不是 16:9 时两层就是**两个不同的盒子**（本层被浏览器非等比拉伸）。
    /// 这种错在读数里完全看不见（`frames=`/`draws=` 都正常），只有把两层几何都写进读数才抓得到。
    ///
    /// 尺寸/位置都从**游戏画布的真实几何**推：后备缓冲尺寸给像素，`getBoundingClientRect`
    /// 给我们 CSS 像素与原点——这样 DPR、页面缩放、画布被 CSS 挪过位置全都自动跟着走，
    /// 不需要自己去读 `devicePixelRatio` 再假设它等于后备/布局比。
    fn fit_now(&mut self) {
        let (box_w, box_h) = canvas_size(&self.game_canvas);
        let (fit_w, fit_h) = contain_box(box_w, box_h);
        if (fit_w, fit_h) != (self.host.core.width(), self.host.core.height()) {
            // 后备缓冲尺寸变了：画布属性 → surface 重新配置 → 渲染视口。三者必须同一帧改完，
            // 否则会出现"按旧尺寸渲、往新尺寸显"的一帧（Windows 上是闪烁，wasm 上是拉伸）。
            self.canvas.set_width(fit_w);
            self.canvas.set_height(fit_h);
            self.host.core.resize(fit_w, fit_h);
            if let Some(sim) = self.host.core.sim_mut() {
                // Keep scene extraction in design coordinates; the surface/backbuffer size is
                // only the raster target. Passing the fitted pixel size here scales Canvas UI twice.
                let (design_w, design_h) = crate::fit::design_size();
                sim.set_viewport(design_w, design_h);
            }
        }
        // CSS：把画布摆到游戏画布内容盒的正中，尺寸按同一个比例换回 CSS 像素。
        let rect = self.game_canvas.get_bounding_client_rect();
        let (css_left, css_top, css_w, css_h) = if rect.width() > 0.0 && rect.height() > 0.0 {
            let per_pixel = rect.width() / box_w as f64;
            (
                rect.left() + (rect.width() - fit_w as f64 * per_pixel) / 2.0,
                rect.top() + (rect.height() - fit_h as f64 * per_pixel) / 2.0,
                fit_w as f64 * per_pixel,
                fit_h as f64 * per_pixel,
            )
        } else {
            (0.0, 0.0, fit_w as f64, fit_h as f64)
        };
        let css = format!(
            "left:{css_left:.2}px;top:{css_top:.2}px;width:{css_w:.2}px;height:{css_h:.2}px"
        );
        if css != self.fit_css {
            let style = self.canvas.style();
            let _ = style.set_property("position", "fixed");
            let _ = style.set_property("pointer-events", "none");
            let _ = style.set_property("z-index", "1");
            let _ = style.set_property("left", &format!("{css_left:.2}px"));
            let _ = style.set_property("top", &format!("{css_top:.2}px"));
            let _ = style.set_property("width", &format!("{css_w:.2}px"));
            let _ = style.set_property("height", &format!("{css_h:.2}px"));
            self.fit_css = css;
        }
        self.fit_note = format!(
            "viewport={box_w}x{box_h} fit={fit_w}x{fit_h} css={css_w:.0}x{css_h:.0}+{css_left:.0}+{css_top:.0}"
        );
        // 同一组数喂给输入映射：**"画在哪"与"点在哪"必须同源**。分别各算一次是
        // "虚拟摇杆/点哪块地整条命中链错位"的经典成因，而这种错在读数里看不出来。
        if let Some(shared) = self.input_shared.as_ref() {
            shared.borrow_mut().map = [css_left, css_top, css_w, css_h];
        }
    }

    /// 渲一帧到画布纹理并 present：本帧输入在这里采好（DOM 采集 ∪ JS 世界 `RLFI`），
    /// 渲染链本身在 [`crate::engine_core::EngineCore::frame`]；返回渲染器自报的 draw 计数
    /// 与拼好的状态属性文案（抽取前后逐字相同）。
    fn frame(&mut self, timestamp_ms: f64) -> Result<(u64, u64, String), String> {
        let elapsed_seconds = self
            .last_animation_frame_ms
            .replace(timestamp_ms)
            .map(|previous| (timestamp_ms - previous).max(0.0) / 1000.0)
            .unwrap_or(1.0 / f64::from(SIM_UPDATES_PER_SECOND));
        // 先对齐几何（窗口/DPR 变了就在这里改画布 + surface + 视口），再取画布纹理：
        // 反过来就会用旧尺寸的纹理去渲新尺寸的内容。
        self.fit_now();
        // 输入：本帧 = **Rust 侧 DOM 采集**（WP6-04，没有 JS 世界也能操作）**并上** JS 世界送来的
        // `RLFI`（它那里有屏幕虚拟摇杆的轴与补丁层的虚拟键）。两路都是合法来源，丢掉任何一边
        // 都会让某一种操作方式失灵，且症状（"摇杆没反应"）离原因很远。
        // 顺序是**行为**的一部分（本片实测踩到）：取值必须在**本帧剧本跑完、`take_frame`
        // 之前**读 —— `take_frame` 会清掉 `pointer_fresh`，读晚了就永远得到"本侧本帧没动过"，
        // 于是另一侧赢走光标**与按下点**，本侧的指针输入被静默抹掉（症状：`cursor` 恒为 0,0）。
        // **判据是"线格式那一侧本帧产过帧"（`wire_is_fresh`），不是"本侧本帧没动过"**（2026-10-17
        // 修正）：B8 删掉旧 JS 世界后浏览器里 `LATEST_FRAME` 恒为默认值（光标/按下点都是 0,0），
        // 用"本侧没动过"当判据会让**两次指针事件之间的每一帧**都抹掉活光标——现场症状正是
        // "拖虚拟摇杆时中心圆点闪来闪去"与"点按钮没反应"（按下点被抹成 (0,0) ⇒ 命中测试脱靶）。
        let mut dom_pointer_moved = false;
        if let Some(shared) = self.input_shared.as_ref() {
            let mut shared = shared.borrow_mut();
            // 确定性输入剧本先走：它按**帧号**发边沿，与 DOM 那条路并进同一份快照。
            // 必须在 `take_frame` 之前——本帧发出的边沿要落在本帧的线格式里。
            // 没给剧本时整段跳过：正常路径一行不改，也不多一次集合比较。
            if !self.input_script.is_empty() {
                self.input_script.tick(&mut shared.producer);
            }
            // 见上面那段说明：**此刻**才是"本侧本帧动过没有"的正确答案。
            dom_pointer_moved = shared.producer.pointer_fresh();
            shared.producer.take_frame(&mut self.frame_input);
            self.dom_events = shared.events;
        }
        let wire = crate::lua_bridge::latest_frame_input();
        let wire_fresh = crate::frame_input::wire_is_fresh(self.last_wire_frame, &wire);
        self.last_wire_frame = wire.frame;
        crate::frame_input::merge_into(
            &mut self.frame_input,
            &wire,
            wire_fresh && !dom_pointer_moved,
        );
        let input = self.frame_input;
        // 计划与渲染都交给统一宿主（`EngineHost::frame`）：外壳不再自己持钟、不再自己算步数。
        // 失败原因原样带出（浏览器这条路的读数与抽取前逐字相同，不加分段前缀）。
        let frame = self
            .host
            .frame(elapsed_seconds, &input)
            .map_err(|error| error.detail)?;
        // 浏览器外壳只读**渲染产出**：这一帧的计划（步数/累加器/夹取）由宿主内部消化——
        // 读数拼装那一段直接用 `clock()` 取累计值，不需要经过 `plan`。
        let out = frame.out;
        // 推进失败**不拆模拟**（核心里只记原因、下次还试）：外壳把"第一次失败"发一次事件，
        // 方便无头驱动直接看到首因，不必等 30 帧后的心跳。
        if let Some(reason) = self.host.core.pending_step_failure.take() {
            set_status(&format!("sim_step_failed:{reason}"));
        }
        if out.builtin {
            // 降级/起不来：原因就在核心的 `sim_note`（`rust-failed:{原因}`），照原样带出去。
            self.sim_note = self.host.core.sim_note.clone();
        } else {
            self.sim_note = format!(
                "rust {} scene_delta={:+},{:+},{:+}{}{} {}",
                out.scene_note,
                out.scene_delta[0],
                out.scene_delta[1],
                out.scene_delta[2],
                self.world_do_note,
                self.world_expr_note,
                crate::audio_sink::note()
            );
        }
        // 稳态三分（**首帧不计入**：那一帧含一次性建图集/建管线）。三者之和与 `avg_frame_ms`
        // 的差就是宿主自身开销（取画布纹理、每帧让出宏任务等）。
        let per = |sum: f64| {
            if self.host.core.timed_frames == 0 {
                0.0
            } else {
                sum / self.host.core.timed_frames as f64
            }
        };
        // 输入读数与几何读数同级：`dom_events` 证明事件真的到了 Rust 侧，
        // `input_dropped` 证明"落不进键表的键"是被**计数**丢弃而不是静默失效。
        // 没有这两个数，"按了没反应"只能靠猜是没收到事件、还是收到了却没落到模拟里。
        let input_dropped = self
            .input_shared
            .as_ref()
            .map(|shared| shared.borrow().producer.dropped_keys())
            .unwrap_or(0);
        // 临时遥测（排查"触摸抬手后摇杆不回弹"）：`ptrD/ptrU` = 多指通路收到几次，
        // `ptrM` = 走鼠标分支几次（读数是 `down/up`）。
        let (touch_downs, touch_ups, mouse_downs, mouse_ups) = self
            .input_shared
            .as_ref()
            .map(|shared| {
                let shared = shared.borrow();
                (
                    shared.touch_downs,
                    shared.touch_ups,
                    shared.mouse_downs,
                    shared.mouse_ups,
                )
            })
            .unwrap_or((0, 0, 0, 0));
        let note = format!(
            "scene={} textures={}/{} {} boot_ms={:.1} first_frame_ms={:.1} startup_ms={:.1} \
             avg_step_ms={:.2} avg_mesh_ms={:.2} avg_render_ms={:.2} timed_frames={} \
             dom_events={} ptrD={}/{} ptrM={}/{} input_dropped={}{} {}",
            self.sim_note,
            self.host.core.texture_stats.0,
            self.host.core.texture_stats.0 + self.host.core.texture_stats.1,
            self.host.core.texture_note,
            self.host.core.boot_ms,
            self.host.core.first_frame_ms,
            self.host.core.boot_ms + self.host.core.first_frame_ms,
            per(self.host.core.sum_step_ms),
            per(self.host.core.sum_mesh_ms),
            per(self.host.core.sum_render_ms),
            self.host.core.timed_frames,
            self.dom_events,
            touch_downs,
            touch_ups,
            mouse_downs,
            mouse_ups,
            input_dropped,
            self.input_script_note,
            self.fit_note,
        );
        // P1 第三片（§9-A 第一刀）：把**宿主本帧真正消费到的那份输入**与场景读数写成
        // **结构化**属性 `data-rustlua-engine-host`，供门断言。
        //
        // 为什么必须结构化：删掉旧 JS 世界之后"输入还有没有进到宿主"是这条路上风险最高的一条
        // （旧运行时不跑 ⇒ 不崩溃、不报错、`presented` 照涨，只是人钉死），而门要断言的是
        // "某个数随事件变化"——从日志串里正则抠数字是最脆的做法。
        //
        // 为什么这件事在**浏览器**上先做：这条路**不依赖旧世界**——Rust 侧 DOM 输入生产者
        // （`install_input_listeners`）与确定性剧本（`frame_input::ScriptDriver`）都在宿主自己
        // 手里，JS 世界送来的 `RLFI` 只是一条可选旁路。于是"输入进了宿主"在这里有真现场可断言，
        // 而在微信那条路上它被"离线假宿主不给 GL ⇒ 宿主到不了 running"压住（见 M73-01 §9）。
        //
        // 每帧写一次而不是每 30 帧：它是**本帧**的读数，慢一拍就会把"这一帧没输入"读成"有"。
        // 累计计数与"本帧消费到的是什么"都由宿主自己落账（`EngineHost::frame`），这里只把那份
        // 读数**发布**出去——两个外壳发布的是同一个实现（见 `EngineHost::scene_state_json`）。
        self.publish_engine_state();
        Ok((out.draw_calls, out.triangles, note))
    }

    /// 写 `data-rustlua-engine-host`（见 [`Self::frame`] 里调用点的说明）。
    ///
    /// 内容来自 [`crate::engine_host::EngineHost::scene_state_json`]——与微信那条路的
    /// `engine_host_state().scene` **同一个实现**（本片之前是两份手拼的 JSON，且同一个 `textures`
    /// 已经漂成"数组 vs 字符串"两种形状）。
    fn publish_engine_state(&self) {
        // `world` 走浏览器自己的只读观测通道（`?rustlua-world-expr=`）；这条外壳没有
        // "最近一帧场景自报"的概念 ⇒ 传空串（留空而不是省字段，见 `scene_state_json` 的说明）。
        let state = serde_json::json!({
            // 谁答的：`engine_host_state()` 是**同一个导出**，两个外壳都可能答它。
            "shell": "browser-gpu-host",
            "mode": "scene",
            "size": format!("{}x{}", self.host.core.width(), self.host.core.height()),
            "fit": self.fit_note,
            "scene": self.host.scene_state_json(&self.world_expr_note, ""),
        })
        .to_string();
        // **同一份字符串喂两个读者**：`engine_host_state()`（导出，给 JS/真机查）与 DOM 属性
        // （给无头驱动读）。必须先发布再取 body——没有 DOM 的形态下导出也该答得出来。
        // 共享同一份是刻意的：各拼一次就是"门断言的"与"导出答的"再次分叉。
        crate::host_runtime::publish_adopted_state(state.clone());
        let Some(body) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.body())
        else {
            // 没有 document/body ⇒ 属性这条路不存在，但导出那份已经发布过了。
            return;
        };
        let _ = body.set_attribute("data-rustlua-engine-host", &state);
        // **启动状态的归属交接**（B5）：`ok` 以前只有 JS 世界写（`web-runtime.js` 的 `markBoot`），
        // JS 世界删掉之后这个属性就没有写者了 —— 而诊断与回归门读的正是它。宿主在这里补上，
        // 但**只补不盖**（判据见 `boot_state_to_publish`）：`failed` 绝不能被"我这边一切正常"盖掉。
        if let Some(boot) = crate::boot_state::boot_state_to_publish(
            body.get_attribute("data-rustlua-boot").as_deref(),
        ) {
            let _ = body.set_attribute("data-rustlua-boot", boot);
        }
    }
}

/// GPU 路径**自己**的画布 id。画面自检与人工排查都认这个名字。
const GPU_CANVAS_ID: &str = "rustlua-gpu-canvas";

/// 给 GPU 路径造一张**独立**画布。
///
/// ## 为什么非独立不可（本轮实测，不是设计偏好）
///
/// 一张 canvas 只能有一个上下文。第一版把 GPU 渲染器建在游戏画布上，`?rustlua-gpu=1` 时
/// 页面立刻以 `boot=failed … detail=2D canvas context is not available` 收场：WebGL2 先被建起，
/// 随后 `web-runtime.js` 的 `getContext('2d')` 拿到 `null`（浏览器对这种请求只回 null、不抛错），
/// 于是**整条既有画面链路**被自己的新通道打死——比"GPU 没画面"严重得多。
///
/// 所以：游戏画布留给 Canvas2D 运行时，GPU 画布单独一张、盖在上面。样式上
/// `pointer-events:none` 是必需的——输入事件必须继续由游戏画布与 Lua 侧收（见
/// `input_frame_layout_json` 那条链），GPU 画布只负责显示。
fn create_gpu_canvas(document: &web_sys::Document) -> Result<web_sys::HtmlCanvasElement, String> {
    if let Some(existing) = document.get_element_by_id(GPU_CANVAS_ID) {
        // 重入（同一次页面里再启一次）时复用，避免叠出好几张画布。
        return existing
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| format!("#{GPU_CANVAS_ID} 不是 canvas"));
    }
    let canvas = document
        .create_element("canvas")
        .map_err(|error| format!("create_element(canvas): {error:?}"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "create_element 返回的不是 canvas".to_owned())?;
    canvas.set_id(GPU_CANVAS_ID);
    // 尺寸**不由这里定**：本层画布每帧对齐到"游戏画布里的等比适配盒"（见 `fit_now`），
    // 这里只给一个与设计分辨率同比例的初值，让第一帧之前也不至于是一张 0×0 的画布。
    // 曾用"固定设计分辨率 + CSS 铺满视口"：窗口不是 16:9 时本层被**非等比拉伸**，
    // 与 Canvas2D 层的 contain 盒子对不上，而这种错在属性读数里完全看不出来。
    let (design_w, design_h) = crate::fit::design_size();
    canvas.set_width(design_w);
    canvas.set_height(design_h);
    let style = canvas.style();
    let _ = style.set_property("position", "fixed");
    // 首帧之前（还没有适配读数）不铺满：显式给 0，避免用"铺满"当成默认行为。
    let _ = style.set_property("left", "0");
    let _ = style.set_property("top", "0");
    let _ = style.set_property("width", "0px");
    let _ = style.set_property("height", "0px");
    // 输入必须继续落到游戏画布上：这张画布只显示。
    let _ = style.set_property("pointer-events", "none");
    let _ = style.set_property("z-index", "1");
    let body = document
        .body()
        .ok_or_else(|| "document.body 不存在".to_owned())?;
    body.append_child(&canvas)
        .map_err(|error| format!("append_child(gpu canvas): {error:?}"))?;
    Ok(canvas)
}

/// 宿主入口：`?rustlua-gpu=1` 时调用。**任何失败都写进 `data-rustlua-gpu`**，
/// 不让它变成页面上一句无声的白屏。
#[wasm_bindgen]
pub async fn gpu_host_run(canvas_id: &str, scene_path: &str) -> Result<String, JsValue> {
    set_status("initializing");
    let result = run_inner(canvas_id, scene_path).await;
    match &result {
        Ok(note) => set_status(&format!("presented {note}")),
        Err(error) => set_status(&format!("failed: {error}")),
    }
    result.map_err(|error| JsValue::from_str(&error))
}

async fn run_inner(canvas_id: &str, scene_path: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_owned())?;
    let document = window.document().ok_or_else(|| "no document".to_owned())?;
    // 只做存在性校验之外的用途：**读它的几何**——Canvas2D 层把设计空间 contain 进这张画布，
    // 本层要复刻同一个内容盒（见 `GpuHost::fit_now`）。GPU 有**自己的**画布（一张画布只能有
    // 一个渲染上下文，见本文件顶部），所以尺寸不再取设计分辨率，而是取这张画布里的适配盒。
    let host_canvas = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| format!("missing canvas #{canvas_id}"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| format!("#{canvas_id} is not a canvas"))?;
    let canvas = create_gpu_canvas(&document)?;
    let lifecycle = LifecycleEvents::install(&document)?;
    let mut host = GpuHost::new(&canvas, &host_canvas, scene_path, lifecycle).await?;
    // 起完栈先对齐一次几何：这样"frames=0"那条开屏读数与第一帧的取景就已经是同一个盒子，
    // 不必靠"第一帧跑完再看"（否则无头驱动在 frames=0 上读到的是设计分辨率的旧数）。
    host.fit_now();
    // 几何对齐之后再装输入监听器：监听器里的光标换算用的是 `fit_now` 写的映射，
    // 顺序反过来会有一小段"按初始盒换算"的窗口。
    host.install_input_listeners();
    // 起设备是**必须等**的（它决定这条通道能不能用，读数要当场拿到）；帧循环则不该等：
    // 它跑几百帧、每帧让出一个宏任务，同步跑完会把 JS 那条启动链一起摁住几百毫秒。
    // 所以这里把它派到事件循环上，`start()` 立刻继续跑游戏自己的启动。
    let width = host.host.core.width();
    let height = host.host.core.height();
    let opening = format!(
        "frames=0 draw_calls=0 triangles=0 {} canvas=#{GPU_CANVAS_ID} {}x{}",
        host.sim_note, width, height
    );
    wasm_bindgen_futures::spawn_local(async move {
        // 每帧均时：这条通道**唯一**能回答"现在跑多快"的数。两条读法上的注意：
        // ① present 是 `Fifo`（跟随 rAF/vsync），所以下限就是刷新周期，不是 GPU 上限；
        // ② 本机 GPU 是 SwiftShader（`device_type=Cpu`），绝对帧时比真机差一个量级。
        // 因此它只用于**同机对比**（这一轮 vs 上一轮），不当真机性能。
        let loop_started = web_time::Instant::now();
        let budget = frame_budget();
        let mut index = 0u32;
        let mut resume_generation = host.lifecycle.resume_generation();
        let mut frame_work_samples_ms = Vec::with_capacity(
            (budget.unwrap_or(STATUS_EVERY * 4).saturating_sub(1) as usize)
                .min(MAX_FRAME_WORK_SAMPLES),
        );
        let wasm_memory_start_bytes = crate::wasm_memory_bytes();
        let mut wasm_memory_peak_bytes = wasm_memory_start_bytes;
        loop {
            let timestamp_ms = match next_animation_frame().await {
                Ok(timestamp) => timestamp,
                Err(error) => {
                    set_status(&format!("failed frame-clock:{error}"));
                    return;
                }
            };
            if !host.lifecycle.is_visible() {
                if let Some(shared) = host.input_shared.as_ref() {
                    shared.borrow_mut().producer.release_all();
                }
                host.last_animation_frame_ms = None;
                host.host.reset_accumulator();
                continue;
            }
            let current_resume_generation = host.lifecycle.resume_generation();
            if current_resume_generation != resume_generation {
                host.last_animation_frame_ms = None;
                host.host.reset_accumulator();
                resume_generation = current_resume_generation;
            }
            index = index.saturating_add(1);
            // 写状态属性之前**按需**刷一次世界读数：读数不进热路径（见 RustScene::refresh_world
            // 里的实测记录——自动刷新会把并行跑的 JS 剧本挤出帧预算）。放在 rame() 之前，
            // 这样本帧组出来的读数与本帧的世界一致。
            if index % STATUS_EVERY == 0 || budget == Some(index) {
                host.refresh_world_readout();
            }
            let work_started = web_time::Instant::now();
            let frame = match host.frame(timestamp_ms) {
                Ok(frame) => frame,
                Err(error) => {
                    // 失败要**当场**写进属性并收工：沉默地停掉会让"通道坏了"看起来像"跑完了"。
                    set_status(&format!("failed frames={index} detail={error}"));
                    return;
                }
            };
            let frame_work_ms = work_started.elapsed().as_secs_f64() * 1000.0;
            if index > 1 && frame_work_samples_ms.len() < MAX_FRAME_WORK_SAMPLES {
                frame_work_samples_ms.push(frame_work_ms);
            }
            wasm_memory_peak_bytes = wasm_memory_peak_bytes.max(crate::wasm_memory_bytes());
            if index % STATUS_EVERY == 0 || budget == Some(index) {
                let elapsed_ms = loop_started.elapsed().as_secs_f64() * 1000.0;
                let avg_frame_ms = elapsed_ms / index as f64;
                let work_p95_ms = percentile_ms(&frame_work_samples_ms, 0.95);
                let work_p99_ms = percentile_ms(&frame_work_samples_ms, 0.99);
                // 读数取**本帧**的：最后一步写下的就是最后一帧的数字，因此不需要额外留一份"上一帧"。
                set_status(&format!(
                    "presented frames={index} sim_steps={} sim_dropped={} sim_clamped={} sim_acc_ms={:.3} draw_calls={} triangles={} avg_frame_ms={avg_frame_ms:.2} work_p95_ms={work_p95_ms:.2} work_p99_ms={work_p99_ms:.2} work_samples={} wasm_mem_start_bytes={wasm_memory_start_bytes} wasm_mem_peak_bytes={wasm_memory_peak_bytes} fps={:.1} {} canvas=#{GPU_CANVAS_ID} {width}x{height}",
                    host.host.clock().total_steps(),
                    host.host.clock().total_dropped_steps(),
                    host.host.clock().clamped_frames(),
                    host.host.clock().accumulator_seconds() * 1000.0,
                    frame.0,
                    frame.1,
                    frame_work_samples_ms.len(),
                    1000.0 / avg_frame_ms,
                    frame.2,
                ));
            }
            if budget == Some(index) {
                break;
            }
        }
    });
    Ok(opening)
}

/// 宿主画布能力探针（WP8 微信半边第一片）：**不碰 `window`/`document`/`location`**，
/// 直接拿调用方给的那张画布问引擎"能不能在这上面起渲染栈"。
///
/// ## 它为什么存在
///
/// 微信小游戏里没有浏览器那套宿主：`HTMLCanvasElement` 不是那个构造器，而
/// [`gpu_host_run`] 是先 `document.get_element_by_id(canvas_id)` 再
/// `dyn_into::<HtmlCanvasElement>()`——`dyn_into` **会**做构造器判定，小游戏里必然失败
/// （见 `lib.rs` 里 [`crate::start_wechat`] 的说明）。而引擎真正需要的那一步
/// （`rustlua_gpu::web_surface::canvas_surface(&instance, &JsValue)`）本来就只收**原始对象**。
/// 所以"能不能在小游戏上沿用同一条渲染通道"这个问题，可以在**不引入任何浏览器构造器依赖**的
/// 前提下，由引擎自己回答——这也正是唯一能在真机上被回答的问题。
///
/// ## 契约（与 `plugins/wechat-minigame/wechat-adapter.js` 的 `glCapability` 同一族）
///
/// * **输入**：一个画布对象（`JsValue`，用 `unchecked_into` 收下 ⇒ 只做属性/方法访问，
///   不做构造器判定；普通对象也能进），外加 `report(message)` 回报。
/// * **绝不静默**：成功返回摘要（适配器名/后端/设备类型/画布尺寸/格式是否匹配），
///   失败返回 `probe: …` 开头的**具名**原因（不返回 `undefined`、不挂起）。
/// * **硬约束（调用方负责）**：只允许传**专用**画布。一张画布只能有一个上下文，把主画布
///   传进来会毁掉它已有的 2D 上下文（`wechat-adapter.js` 1457-1462 行记过这条实测）。
/// * 本函数**只建实例/表面/适配器**并读能力，不建设备、不建渲染器、不画帧——
///   因此它可以在小游戏宿主里安全地先跑一次，作为"要不要往这条路上投资"的判据。
#[wasm_bindgen]
pub async fn probe_canvas_host(
    gpu_canvas: JsValue,
    report: js_sys::Function,
) -> Result<String, JsValue> {
    crate::install_panic_hook();
    match probe_canvas_host_inner(gpu_canvas, &report).await {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let text = format!("probe: {error}");
            // 失败既要抛给调用方，也要走同一条回报通道：真机上控制台不可靠，
            // 适配层收下的这行才是可读证据。
            let _ = report.call1(&JsValue::NULL, &JsValue::from_str(&text));
            Err(JsValue::from_str(&text))
        }
    }
}

async fn probe_canvas_host_inner(
    gpu_canvas: JsValue,
    report: &js_sys::Function,
) -> Result<String, String> {
    if gpu_canvas.is_null() || gpu_canvas.is_undefined() {
        return Err("no canvas（调用方没有给出画布对象）".to_owned());
    }
    // `unchecked_into` 是这一片的全部要点：不触发 wasm-bindgen 的构造器判定。
    let canvas: web_sys::HtmlCanvasElement = gpu_canvas.clone().unchecked_into();
    // **直接读**宽高，不能走 `canvas_size`：那个函数对 0 尺寸有回退（`FALLBACK_SIZE`），
    // 于是"画布没建好"会被静默换成设计分辨率，这里就永远查不出来（我自己第一版就写成
    // 用 canvas_size 判定，结果那个分支是死代码——本用例把它抓了出来）。
    let (width, height) = (canvas.width(), canvas.height());
    if width == 0 || height == 0 {
        // 0 尺寸的 surface 配置在 wgpu 里无效：与其让它到下面报一句难懂的校验错，
        // 不如在这里具名拒绝（读数是"画布没建好"，不是"GPU 不可用"）。
        return Err(format!("canvas {width}x{height}（尺寸为 0）"));
    }
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    let surface = rustlua_gpu::web_surface::canvas_surface(&instance, &gpu_canvas)
        .map_err(|error| format!("canvas_surface: {error}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|error| format!("request_adapter: {error}"))?;
    let info = adapter.get_info();
    let capabilities = surface.get_capabilities(&adapter);
    // 渲染器的颜色附件格式是构造时烘死的（`GpuHost::new` 同一口径），所以这里也要回答
    // "这个宿主给不给它"——否则"能建表面"会被误读成"能画"。
    let wanted = wgpu::TextureFormat::Rgba8UnormSrgb;
    let format_ok = capabilities.formats.contains(&wanted);
    let summary = format!(
        "canvas {width}x{height}; adapter={} backend={:?} device={:?}; formats=[{}]; srgb={}; present_modes={}",
        info.name,
        info.backend,
        info.device_type,
        capabilities
            .formats
            .iter()
            .map(|format| format!("{format:?}"))
            .collect::<Vec<_>>()
            .join(","),
        if format_ok { "yes" } else { "no" },
        capabilities.present_modes.len()
    );
    report
        .call1(&JsValue::NULL, &JsValue::from_str(&summary))
        // 回报通道自己抛错时也要说清是哪一步。这里**不**引 `js_error_text`：它是内层模块的
        // 私有函数，跨层引用会把"这条路径只依赖调用方给的画布与回调"这个边界搞乱。
        .map_err(|error| {
            format!(
                "report: {}",
                error.as_string().unwrap_or_else(|| format!("{error:?}"))
            )
        })?;
    if !format_ok {
        return Err(format!(
            "画布不提供 {wanted:?}（给的是 {:?}）；渲染器的目标格式尚未参数化",
            capabilities.formats
        ));
    }
    Ok(summary)
}
