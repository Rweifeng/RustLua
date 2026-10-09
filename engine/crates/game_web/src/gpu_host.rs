//! Web 宿主的 GPU 渲染第一片：用**引擎自己的渲染器**把一帧画到画布上。
//!
//! ## 这一步在补什么
//!
//! M73-00 之前，web/微信上的画面全部由 `plugins/webgl/web-runtime.js` 的 Canvas2D 路径画出来，
//! `game_web` 里**连 `engine::host::Renderer` 都没有**——引擎那台按 WebGPU 规格写的渲染器
//! 在浏览器里从未被宿主用过。WP5 前四片把这条路的每个前提单独验通了（能建、能按期望值画对、
//! 内存贴图能交进来、能渲进画布纹理并 present），本片把它们**接起来**。
//!
//! ## 为什么是 Rust 侧的 URL 开关
//!
//! 默认路径（Canvas2D）一行都不动：GPU 路径只在 `?rustlua-gpu=1` 时才建，建成与否写进
//! `document.body` 的 `data-rustlua-gpu`，供门（`scripts/verify-webgl-bundle.ps1 -Gpu2d`）
//! 与人工排查读。放在 Rust 侧而不是页面脚本里，是为了**不碰** `plugins/webgl/` 下的既有改动。
//!
//! ## 已实测钉住的平台事实（WP5 第四片，本片直接沿用）
//!
//! - WebGL2 画布提供的格式里就有 `Rgba8UnormSrgb`——**正是**渲染器构造时烘死的那个，
//!   所以不需要为 web 做格式参数化；不提供时必须**响亮失败**，不能往不匹配的 surface 上渲。
//! - 画布纹理只支持 `RENDER_ATTACHMENT`：**不要**申请 `COPY_SRC`。申请了 wgpu 会在**未捕获**的
//!   校验错误上 panic，而 wasm 是 `panic=abort`——整个模块会死，现场只剩一句
//!   `Uncaught RuntimeError: unreachable`。

use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

use engine::host::{OverlayVertex, RenderOutcome, Renderer, SceneDraw, SceneMesh};

use crate::rust_scene::SimRoute;

/// 这条通道跑多少帧后收工。它**不再阻塞启动链**（见 `run_inner`）：帧循环被派到事件循环上，
/// 所以"跑多少帧"只决定这条通道活多久，不决定页面多久能起来。
///
/// 为什么从"固定 3 帧"变成"跑一长串"：3 帧能证明 `Fifo` 下画布纹理能反复取用（第二帧才会暴露
/// "上一帧没还回去"），但证明不了"它在**持续**渲染"——而持续渲染才是宿主该有的形状，
/// 也才谈得上后面接输入与场景切换。
const FRAME_BUDGET: u32 = 240;
/// 状态属性每这么多帧改写一次：既是心跳，也让"跑到一半被抓拍"的 dump 仍有可读读数。
const STATUS_EVERY: u32 = 30;
/// 画布尺寸取 2D 设计分辨率的一半做离屏目标不合适：这里直接按画布实际尺寸渲染。
const FALLBACK_SIZE: u32 = 256;

/// 本帧循环跑多少帧收工——默认 [`FRAME_BUDGET`]，可用 URL `rustlua-gpu-frames=<n>` 覆盖（`0` = 不设上限）。
///
/// 为什么需要这个开关：状态属性每 [`STATUS_EVERY`] 帧才刷一次，而 240 帧跑完**循环就停了**，
/// 属性随之冻结。于是"按一下键，看读数有没有动"这种验法在 240 帧之后永远看不到变化——
/// 本轮实测就卡在这里：探针读到的 `frames 240 -> 240 -> 240`、`dom_events` 恒为 0，
/// 看起来像"事件没到"，实际是**宿主已经收工了**。交互式验证必须让循环活着。
fn frame_budget() -> u32 {
    let Some(window) = web_sys::window() else {
        return FRAME_BUDGET;
    };
    let Some(search) = window.location().search().ok() else {
        return FRAME_BUDGET;
    };
    const PREFIX: &str = "rustlua-gpu-frames=";
    let Some(start) = search.find(PREFIX) else {
        return FRAME_BUDGET;
    };
    let rest = &search[start + PREFIX.len()..];
    let end = rest.find('&').unwrap_or(rest.len());
    match rest[..end].trim().parse::<u32>() {
        // `0` 读作"不设上限"：把它当成 `u32::MAX` 会让 `index == budget` 那条永远不会触发，
        // 但循环本身照跑——语义上就是"一直跑到页面关闭"。
        Ok(0) => u32::MAX,
        Ok(value) => value,
        Err(_) => FRAME_BUDGET,
    }
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

/// 每条宿主帧推进几个**定长步**，以及一步多长。
///
/// 为什么按帧固定步、而不是按真实经过时间：wasm 上 `std::time::Instant::now()` 是会 panic 的
/// 不支持路径（第 13 轮那类事故的同族），而 `performance.now()` 会让读数随无头环境的调度抖动。
/// 固定步换来的是**可复现**：读第 N 帧就能反推模拟到哪了。代价写在这里，别当成"对时间没意见"：
/// 宿主帧率与逻辑步因此**一一对应**，二者不同源——真实时间对齐要等输入 ABI 把
/// 宿主的帧序号/时间一并送进来（`FrameInput.frame` 已经在线上，接它即可）。
const SIM_STEPS_PER_FRAME: u32 = 1;
/// 一步的时间长度（秒）。与编辑器预览、headless 运行器用的同一个值。
const SIM_FIXED_DELTA: f32 = 1.0 / 60.0;

/// 内存里造一张 2×2 四色棋盘。资源里找不到可用贴图时的兜底，也是"贴图路径真的被采样到了"的判据：
/// 四色里**没有白**，所以最后那格若不是对应颜色，只可能是白兜底。
const CHECKER: [[u8; 4]; 4] = [
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
    [255, 255, 0, 255],
];
const CHECKER_KEY: &str = "web://checker2x2";

/// GPU 宿主：surface + device/queue + 渲染器。
struct GpuHost {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    renderer: Renderer,
    /// surface 配置（改尺寸时只改宽高再 `configure`，其余字段原样保留——重建配置最容易
    /// 悄悄换掉 `alpha_mode`/`present_mode`）。
    surface_config: wgpu::SurfaceConfiguration,
    /// 本层画布（GPU 那张）与**游戏画布**（Canvas2D 那张）。后者只用来读窗口尺寸：
    /// `web-runtime.js` 会把游戏画布的后备缓冲跟到窗口（含 DPR），本层必须与它同框。
    canvas: web_sys::HtmlCanvasElement,
    game_canvas: web_sys::HtmlCanvasElement,
    width: u32,
    height: u32,
    /// 最近一次适配读数（进 `note`，无头驱动据此断言两层同框）。
    fit_note: String,
    /// 最近一次写进画布 style 的几何（只在变化时写 DOM，避免每帧刷样式）。
    fit_css: String,
    /// 贴图注册用的键。**必须与内建场景写进 `paths[0]` 的是同一个字符串**，
    /// 否则渲染器查不到这张贴图、静默退回白兜底——画面"看着正常"，只是那块贴图永远不对。
    texture_key: String,
    /// 贴图来源（写进状态属性，便于一眼分辨"用的是真资源还是兜底棋盘"）。
    texture_note: String,
    /// Rust 侧运行时场景。`None` = 起不来（`sim_note` 里就是原因），画面退回内建场景。
    sim: Option<crate::rust_scene::RustScene>,
    sim_note: String,
    /// 已经交给渲染器的贴图路径（避免每帧重复解码/上传）。
    uploaded: std::collections::HashSet<String>,
    /// `(已上传, 失败)` 计数，写进状态属性。
    texture_stats: (usize, usize),
    /// 贴图只在**第一帧**对齐一次：本片不处理运行期换图（流式重传是后续内容）。
    textures_ensured: bool,
    /// 第一帧专用：阶段自报只在第一帧写（诊断卡死用，不污染每帧路径）。
    first_frame_done: bool,
    /// 推进失败只报一次事件（读数里的 `step_errors` 每帧都在，事件是给无头驱动看的）。
    step_failure_reported: bool,
    /// 启动（场景 + 工程脚本装载）耗时，毫秒。**必须活到最后一帧**：阶段属性会被后续
    /// `set_status` 覆盖，只有进 `note` 才留得下来。
    boot_ms: f64,
    /// **首帧**耗时，毫秒：首步（工程脚本 + `ui.measure_text` 首次读/解析字体）**加上**首次取网格
    /// （首次文本图集 = 解码 + 建图集 + 预热，以及首次绘制管线/贴图上传）。
    /// 两段合在一起报，是因为它们发生在同一次 `next_mesh()` 里；拆开反而会让人把图集成本
    /// 算到逻辑步头上。
    first_frame_ms: f64,
    /// 稳态每帧三分：逻辑（`step`）、取网格（`mesh`）、渲染+呈现（`render_into_texture`+`present`）。
    /// **首帧不计入**：那一帧含一次性建图集/建管线，混进来会把三个均值一起抬高，
    /// 而"稳态跑多快"问的正是后面的帧。
    sum_step_ms: f64,
    sum_mesh_ms: f64,
    sum_render_ms: f64,
    timed_frames: u64,
    /// **Rust 侧自己的输入生产者**（WP6-04）。此前这一层只有消费者：JS 世界采集事件、编码成
    /// `RLFI`、经 `lua_input_frame` 写进常驻表，宿主再读出来——于是"没有 JS 世界就没有输入"。
    /// 有了它，Rust 那条路自己就能被玩家操作（退休清单第一项的落地）。
    ///
    /// 用 `Rc<RefCell<…>>` 是因为监听器闭包要活到宿主结束，而闭包不能借用宿主自己；
    /// wasm 单线程，这是这里该用的东西。
    input_shared: Option<std::rc::Rc<std::cell::RefCell<InputShared>>>,
    /// 本帧要应用给模拟的输入（Rust 采集的 + JS 世界送来的并集）。
    frame_input: rustlua_frame_abi::FrameInput,
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
    /// 稳态第一帧的场景输出计数（draw/顶点/路径），用于 `scene_delta=` 读数。
    mesh_baseline: Option<[usize; 3]>,
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
}

impl Default for InputShared {
    fn default() -> Self {
        Self {
            producer: crate::frame_input::FrameInputProducer::new(),
            // 还没对齐几何前，退化成"CSS 像素即设计像素"（第一帧之前不会有真实操作）。
            map: [
                0.0,
                0.0,
                crate::DESIGN_WIDTH as f64,
                crate::DESIGN_HEIGHT as f64,
            ],
            events: 0,
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
    web_sys::console::log_1(&JsValue::from_str(&format!("[gpu] {state}")));
}

/// 让出一个**宏任务**：WebGL2 的 present 与浏览器合成都靠主循环推进，promise microtask 不够
/// （与探针 `yield_to_event_loop` 同一条实测结论）。
async fn yield_to_event_loop() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            // `setTimeout(..."0")` 的返回值是 timeout id，这里不需要它（失败也无所谓：
            // 拿不到定时器时下面这次 await 会由 microtask 兜住，不会卡死流程）。
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// URL 位 `rustlua-sim=<名字>`：选模拟来源（见 [`crate::rust_scene::SimRoute`]）。
///
/// 默认**不跑脚本**，这是一次刻意的取舍：不跑脚本才能把"引擎能画出工程的**真场景**"
/// （组件 → 网格 → GPU）单独证出来；而"工程的 `main.lua` 能在没有 JS 补丁层的 VM 里跑"
/// 是**另一件事**，已知会卡住（读数见
/// `artifacts/logs/gpu-web-probe/web-host-rust-sim-first-slice-20261008.md`）。
/// 两者合成一条时，脚本的卡住会让"引擎到底画不画得出"这个问题一起失效。
///
/// 取值：`script`（引擎自己加载启动场景 + 跑脚本）、`scene-script`（宿主给场景字节 + 跑脚本）、
/// 其他/缺省（宿主给场景字节、不跑脚本）。
fn sim_route() -> SimRoute {
    let name = web_sys::window()
        .and_then(|window| window.location().search().ok())
        .and_then(|search| {
            search
                .split(['?', '&'])
                .find_map(|pair| pair.strip_prefix("rustlua-sim=").map(str::to_owned))
        })
        .unwrap_or_default();
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

/// 状态属性里的短名：事后从 dump 读到时，不需要回头查映射表。
fn route_label(route: SimRoute) -> &'static str {
    match route {
        SimRoute::SceneOnly => "scene-only",
        SimRoute::EngineStartup => "engine-startup",
        SimRoute::SceneWithScript => "scene-script",
        SimRoute::SceneScriptNoRoot => "scene-script-noroot",
    }
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

/// 等比适配盒（算法在 `crate::fit`，与 Canvas2D 层同一套；这里有原生单测守着）。
fn contain_box(box_w: u32, box_h: u32) -> (u32, u32) {
    crate::fit::contain_box(box_w, box_h, crate::DESIGN_WIDTH, crate::DESIGN_HEIGHT)
}

impl GpuHost {
    /// 建整套 GPU 栈。任何一步失败都要能说出**是哪一步**（wasm 上 panic 不给回溯）。
    async fn new(
        canvas: &web_sys::HtmlCanvasElement,
        game_canvas: &web_sys::HtmlCanvasElement,
        scene_path: &str,
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
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            // WP2 硬约束 1：wasm 上必须显式 `Backends::GL`（默认在 wasm 上只有 webgpu）。
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        // 引擎里那**唯一**一份"从画布建 surface"的实现（web_surface 模块注释解释了这个
        // 不得已而为之的 unsafe 通路：wgpu 27 的 `create_surface(canvas)` 在本版本用不了）。
        let surface =
            engine::host::web_surface::canvas_surface(&instance, &JsValue::from(canvas.clone()))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("request_adapter: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        if !capabilities.formats.contains(&format) {
            // 渲染器的颜色附件格式是构造时烘死的；不匹配就**不要**渲，把话写清楚。
            return Err(format!(
                "画布不提供 {format:?}（给的是 {:?}）；渲染器的目标格式尚未参数化",
                capabilities.formats
            ));
        }
        // WP2 硬约束 2：起点用 WebGL2 一定接受的地板，再把分辨率上限抬到适配器实际值。
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rustlua-web-gpu-device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("request_device: {error}"))?;
        let surface_config = wgpu::SurfaceConfiguration {
            // 只要 RENDER_ATTACHMENT：画布纹理当不了 copy 源，申请 COPY_SRC 会让 wgpu
            // 在未捕获的校验错误上 panic（wasm 直接 abort，见模块注释）。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);
        let mut renderer = Renderer::new_offscreen_async(
            device.clone(),
            queue.clone(),
            // 不挂项目根：本片只画手工场景，不解析任何文件路径。
            None,
            false,
            false,
        )
        .await
        .map_err(|error| format!("Renderer::new_offscreen_async: {error:#}"))?;
        let (texture_key, texture_note) = prepare_texture(&mut renderer)?;
        // Rust 侧运行时场景：**失败不致命**，原因原样带进状态属性，画面退回内建场景。
        // 本片要问的正是"它在 wasm 上到底跑不跑得起来"，所以这里只记录、不重试、不吞掉原因。
        //
        // 阶段自报（`sim_boot_begin` → …）：这一步要跑整个工程的 Lua 脚本，在 wasm 上是
        // 整页唯一的同步长任务——**万一它卡住，页面就卡在 `initializing`，什么线索都没有**。
        // 所以每过一个阶段就把属性改写一次：事后从 dump 里读到最后落定的那个阶段，
        // 就知道该去看哪一段（与 `gpu-web-probe` 的 `set_stage` 同一套做法）。
        set_status("sim_boot_begin");
        let route = sim_route();
        let truncation = apply_script_truncation();
        set_status(&format!(
            "sim_boot_route={} {}",
            route_label(route),
            truncation.clone().unwrap_or_default()
        ));
        // 启动耗时（wasm 上唯一的长同步任务）：`Instant::now()` 在这里是 panic 路径，
        // 所以用与引擎同一颗 `web_time`。读数进状态属性，事后从 dump 里就能读。
        let boot_started = web_time::Instant::now();
        let (sim, sim_note) =
            match crate::rust_scene::RustScene::boot(width, height, route, scene_path) {
                Ok(scene) => {
                    set_status("sim_boot_ok");
                    (Some(scene), "rust(boot ok)".to_owned())
                }
                Err(error) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[gpu] RustScene::boot failed: {error}; builtin scene will be used"
                    )));
                    set_status(&format!("sim_boot_failed:{error}"));
                    (None, format!("rust-failed:{error}"))
                }
            };
        let boot_ms = boot_started.elapsed().as_secs_f64() * 1000.0;
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[gpu] sim_boot_ms={boot_ms:.1} note={sim_note}"
        )));
        Ok(Self {
            surface,
            device,
            renderer,
            surface_config,
            canvas: canvas.clone(),
            game_canvas: game_canvas.clone(),
            width,
            height,
            fit_note: String::new(),
            fit_css: String::new(),
            texture_key,
            texture_note,
            sim,
            sim_note,
            uploaded: std::collections::HashSet::new(),
            texture_stats: (0, 0),
            textures_ensured: false,
            first_frame_done: false,
            step_failure_reported: false,
            boot_ms,
            first_frame_ms: f64::NAN,
            sum_step_ms: 0.0,
            sum_mesh_ms: 0.0,
            sum_render_ms: 0.0,
            timed_frames: 0,
            input_shared: None,
            frame_input: rustlua_frame_abi::FrameInput::new(),
            dom_events: 0,
            input_closures: Vec::new(),
            input_script: crate::frame_input::ScriptDriver::new(input_script),
            input_script_note,
            mesh_baseline: None,
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

        self.input_shared = Some(shared);
        self.input_closures = closures;
    }

    /// 刷新世界读数（只在宿主即将写状态属性时调用，见帧循环里的调用点）。
    ///
    /// 包一层而不是让调用方直接摸 sim：读数属于**观测**，观测不该出现在热路径上。
    fn refresh_world_readout(&mut self) {
        let Some(sim) = self.sim.as_mut() else {
            return;
        };
        // 顺序：先动作、再读数。这样同一帧的属性里，"做完之后世界是什么样"是自洽的
        // （动作只跑一次，读数每帧都新鲜）。
        if !self.world_do_done {
            if let Some(source) = self.world_do.clone() {
                self.world_do_done = true;
                self.world_do_note = format!(" do={}", sim.eval_world_do(&source));
            }
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
        if (fit_w, fit_h) != (self.width, self.height) {
            // 后备缓冲尺寸变了：画布属性 → surface 重新配置 → 渲染视口。三者必须同一帧改完，
            // 否则会出现"按旧尺寸渲、往新尺寸显"的一帧（Windows 上是闪烁，wasm 上是拉伸）。
            self.canvas.set_width(fit_w);
            self.canvas.set_height(fit_h);
            self.surface_config.width = fit_w;
            self.surface_config.height = fit_h;
            self.surface.configure(&self.device, &self.surface_config);
            self.width = fit_w;
            self.height = fit_h;
            if let Some(sim) = self.sim.as_mut() {
                sim.set_viewport(fit_w, fit_h);
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

    /// 选出本帧要渲的网格：优先 Rust 侧运行时场景，失败就退回内建场景（原因留在 `sim_note`）。
    fn next_mesh(&mut self) -> SceneMesh {
        if !self.first_frame_done {
            set_status("mesh_begin");
        }
        // 借出再调：模拟要独占借用，先取出结果、再决定用哪条分支。
        // **先推进模拟（含工程脚本），再取网格**——与桌面运行器同序。反过来会永远画上一帧的世界，
        // 而现在模拟已经吃真实输入了，差一帧就是"操作慢半拍"。
        //
        // 推进失败**不拆模拟**：原因由 `RustScene` 记成读数（`step_errors` / `last_step_error`），
        // 这里只把"第一次失败"发一次事件，方便无头驱动直接看到首因，不必等 30 帧后的心跳。
        let mut step_failed = false;
        // 首帧单独计时：它是"工程脚本第一次真正跑起来"的那一帧，里面含 `ui.measure_text`
        // 的**首次**字体读取与首次文本图集，是启动耗时里最不可预测的一段。
        let first_frame = !self.first_frame_done;
        // 输入：本帧 = **Rust 侧 DOM 采集**（WP6-04，没有 JS 世界也能操作）**并上** JS 世界送来的
        // `RLFI`（它那里有屏幕虚拟摇杆的轴与补丁层的虚拟键）。两路都是合法来源，丢掉任何一边
        // 都会让某一种操作方式失灵，且症状（"摇杆没反应"）离原因很远。
        let js_cursor_wins = self
            .input_shared
            .as_ref()
            .map(|shared| !shared.borrow().producer.pointer_fresh())
            .unwrap_or(true);
        if let Some(shared) = self.input_shared.as_ref() {
            let mut shared = shared.borrow_mut();
            // 确定性输入剧本先走：它按**帧号**发边沿，与 DOM 那条路并进同一份快照。
            // 必须在 `take_frame` 之前——本帧发出的边沿要落在本帧的线格式里。
            // 没给剧本时整段跳过：正常路径一行不改，也不多一次集合比较。
            if !self.input_script.is_empty() {
                self.input_script.tick(&mut shared.producer);
            }
            shared.producer.take_frame(&mut self.frame_input);
            self.dom_events = shared.events;
        }
        let wire = crate::lua_bridge::latest_frame_input();
        crate::frame_input::merge_into(&mut self.frame_input, &wire, js_cursor_wins);
        // 稳态三分：逻辑与取网格分开计时。两者在同一个闭包里发生，所以用两个局部变量带出来。
        let mut step_ms = 0.0f64;
        let mut mesh_ms = 0.0f64;
        let step_started = web_time::Instant::now();
        let frame = self.frame_input;
        let attempt = self.sim.as_mut().map(|sim| -> Result<SceneMesh, String> {
            step_failed = sim
                .step(SIM_STEPS_PER_FRAME, SIM_FIXED_DELTA, &frame)
                .is_err();
            step_ms = step_started.elapsed().as_secs_f64() * 1000.0;
            let mesh_started = web_time::Instant::now();
            let mesh = sim.mesh();
            mesh_ms = mesh_started.elapsed().as_secs_f64() * 1000.0;
            mesh
        });
        if first_frame {
            self.first_frame_ms = step_started.elapsed().as_secs_f64() * 1000.0;
            // 首帧耗时与启动耗时是**两个不同的瓶颈**（前者=工程脚本 + 首次字体读取 + 首次网格/图集，
            // 后者=场景/脚本装载），所以分开报，并且各留一条控制台行（属性会被后面覆盖）。
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[gpu] sim_boot_ms={:.1} first_frame_ms={:.1}",
                self.boot_ms, self.first_frame_ms
            )));
        }
        if step_failed && !self.step_failure_reported {
            self.step_failure_reported = true;
            let reason = self
                .sim
                .as_ref()
                .and_then(|sim| sim.last_step_error())
                .unwrap_or("unknown")
                .to_string();
            set_status(&format!("sim_step_failed:{reason}"));
        }
        let mesh = match attempt {
            Some(Ok(mesh)) => {
                if !self.textures_ensured {
                    self.textures_ensured = true;
                    set_status("textures_begin");
                    self.ensure_textures(&mesh.paths);
                    set_status(&format!(
                        "textures_done={}/{}",
                        self.texture_stats.0,
                        self.texture_stats.0 + self.texture_stats.1
                    ));
                }
                let views = self.sim.as_ref().map(|sim| sim.frame_views());
                if let Some(views) = views {
                    // 2D 取景由**场景自己的相机**给出（与桌面运行器同源）：不清屏/视口两套说法。
                    self.renderer.set_frame_views(views);
                }
                // 世界 Sprite 的深度通道：与桌面运行器同一处调用（场景说没有就保持渲染器默认）。
                let camera = self.sim.as_ref().and_then(|sim| sim.world_sprite_camera());
                if let Some(camera) = camera {
                    self.renderer.set_world_sprite_camera(camera);
                }
                let stats = self.sim.as_ref().map(|sim| sim.note(&mesh));
                if let Some(stats) = stats {
                    // 场景输出基线：稳态第一帧记下 draw/顶点/路径三项，之后每帧报**差值**。
                    //
                    // 为什么是差值：绝对值说不出"这一帧和开局一样不一样"。UI 面板打开、交互结果、
                    // 地图切换都会让这三项跳，而差值能让驱动用一个正则就断言"输入确实改变了场景输出"
                    // （跨进程比对用不了：两次运行的绝对计数会被字体图集预热顺序影响）。
                    // 基线取稳态第一帧（`first_frame` 那一帧含一次性建图集/建管线，它的网格不是稳态）。
                    let counts = [mesh.draws.len(), mesh.vertices.len(), mesh.paths.len()];
                    let delta = match self.mesh_baseline {
                        Some(base) => [
                            counts[0] as i64 - base[0] as i64,
                            counts[1] as i64 - base[1] as i64,
                            counts[2] as i64 - base[2] as i64,
                        ],
                        None => {
                            if !first_frame {
                                self.mesh_baseline = Some(counts);
                            }
                            [0, 0, 0]
                        }
                    };
                    self.sim_note = format!(
                        "rust {stats} scene_delta={:+},{:+},{:+}{}{} {}",
                        delta[0],
                        delta[1],
                        delta[2],
                        self.world_do_note,
                        self.world_expr_note,
                        crate::audio_sink::note()
                    );
                }
                mesh
            }
            Some(Err(error)) => {
                // 起得来、取不出：把它降级成"内建场景"，但**保留原因**（这一片就是来收集原因的）。
                self.sim = None;
                self.sim_note = format!("rust-failed:{error}");
                scene_mesh(&self.texture_key)
            }
            None => scene_mesh(&self.texture_key),
        };
        if !first_frame {
            self.sum_step_ms += step_ms;
            self.sum_mesh_ms += mesh_ms;
        }
        mesh
    }

    /// 把本帧网格引用的贴图从预加载资源表交进渲染器。
    ///
    /// 跳过两个**不是路径**的保留项：白纹理（渲染器就地造）与内建字模图集（保留纹理）。
    /// 这两条最早写在 JS 的 `web-runtime.js` 里（`WHITE_TEXTURE_PATH` 那段注释），
    /// 现在 Rust 侧也要认同一套语义，否则纯色块实体每帧都会去读一个不存在的文件。
    fn ensure_textures(&mut self, paths: &[String]) {
        let mut loaded = 0usize;
        let mut failed = 0usize;
        for path in paths {
            if path == engine::host::frame::WHITE_TEXTURE_PATH
                || path == engine::host::frame::ATLAS_TEXTURE_PATH
            {
                continue;
            }
            if !self.uploaded.insert(path.clone()) {
                loaded += 1;
                continue;
            }
            match crate::lua_bridge::resource_png(path) {
                Some(bytes) => match self.renderer.upload_texture_png(path, &bytes) {
                    Ok(_) => loaded += 1,
                    Err(error) => {
                        failed += 1;
                        web_sys::console::warn_1(&JsValue::from_str(&format!(
                            "[gpu] upload_texture_png({path}) failed: {error}"
                        )));
                    }
                },
                None => {
                    failed += 1;
                    // 资源表里没有这张图：**不静默**——它意味着这一帧的贴图会退回白兜底。
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[gpu] 资源表没有贴图 {path}（该四边形会退回白兜底）"
                    )));
                }
            }
        }
        self.texture_stats = (loaded, failed);
    }

    /// 渲一帧到画布纹理并 present。返回渲染器自报的 draw 计数，供状态属性带出去。
    fn frame(&mut self) -> Result<(u64, u64, String), String> {
        let first_frame = !self.first_frame_done;
        // 先对齐几何（窗口/DPR 变了就在这里改画布 + surface + 视口），再取画布纹理：
        // 反过来就会用旧尺寸的纹理去渲新尺寸的内容。
        self.fit_now();
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|error| format!("get_current_texture: {error}"))?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mesh = self.next_mesh();
        // 渲染+呈现：这一段是 GPU 提交侧的成本（含 present 与一次显式 poll）。
        let render_started = web_time::Instant::now();
        let outcome =
            self.renderer
                .render_into_texture(&mesh, None, &view, self.width, self.height);
        let metrics = self.renderer.last_frame_metrics();
        if !matches!(outcome, RenderOutcome::Presented) {
            // `RenderOutcome` 没有 `Debug`（它的语义靠 `matches!` 读），所以这里给一句人话。
            return Err("render_into_texture 未返回 Presented".to_owned());
        }
        frame.present();
        // WebGL2 没有阻塞能力，错误域与 fence 都靠 poll 推进；这里显式轮询一次，
        // 让"这一帧的校验错误"在本函数返回前暴露，而不是变成下一帧的谜题。
        let _ = self.device.poll(wgpu::PollType::Poll);
        if !first_frame {
            self.sum_render_ms += render_started.elapsed().as_secs_f64() * 1000.0;
            self.timed_frames += 1;
        }
        self.first_frame_done = true;
        // 稳态三分（**首帧不计入**：那一帧含一次性建图集/建管线）。三者之和与 `avg_frame_ms`
        // 的差就是宿主自身开销（取画布纹理、每帧让出宏任务等）。
        let per = |sum: f64| {
            if self.timed_frames == 0 {
                0.0
            } else {
                sum / self.timed_frames as f64
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
        let note = format!(
            "scene={} textures={}/{} {} boot_ms={:.1} first_frame_ms={:.1} startup_ms={:.1} \
             avg_step_ms={:.2} avg_mesh_ms={:.2} avg_render_ms={:.2} timed_frames={} \
             dom_events={} input_dropped={}{} {}",
            self.sim_note,
            self.texture_stats.0,
            self.texture_stats.0 + self.texture_stats.1,
            self.texture_note,
            self.boot_ms,
            self.first_frame_ms,
            self.boot_ms + self.first_frame_ms,
            per(self.sum_step_ms),
            per(self.sum_mesh_ms),
            per(self.sum_render_ms),
            self.timed_frames,
            self.dom_events,
            input_dropped,
            self.input_script_note,
            self.fit_note,
        );
        Ok((metrics.draw_calls, metrics.triangles, note))
    }
}

/// 给场景准备一张贴图，返回 **(注册用的键, 人读的来源说明)**：优先用工程资源里第一张 PNG
/// （证明"打包资源 → GPU 贴图"这条路在真宿主里也是通的），拿不到就退回内存棋盘
/// （保证本片自己可验，不依赖某个游戏的内容）。
///
/// 键必须**只在这一处决定**：调用方（`scene_mesh`）拿到的是同一个字符串，
/// 不允许两处各自去挑一次——那样"上传失败退回棋盘"时，场景仍按资源键去查，
/// 于是静默变成白兜底，而白兜底与"贴图被采样到了"在画面上无法区分。
fn prepare_texture(renderer: &mut Renderer) -> Result<(String, String), String> {
    if let Some((path, bytes)) = crate::lua_bridge::first_png_resource() {
        match renderer.upload_texture_png(&path, &bytes) {
            Ok(_) => return Ok((path.clone(), format!("resource:{path} ({} B)", bytes.len()))),
            Err(error) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "[gpu] upload_texture_png({path}) failed: {error}; falling back to builtin checker"
                )));
            }
        }
    }
    renderer
        .upload_texture_rgba8(CHECKER_KEY, 2, 2, &CHECKER.concat())
        .map_err(|error| format!("upload_texture_rgba8: {error}"))?;
    Ok((CHECKER_KEY.to_owned(), format!("builtin:{CHECKER_KEY}")))
}

/// 手工场景：两块内建白四边形 + 一块**贴图**四边形（用 `texture = 1` 指向 `paths[0]`）。
///
/// 贴图那一格四色之一必须出现：白兜底与本片要证的"贴图被采样到了"是两回事，
/// 所以这一格的颜色是判据，不是装饰。
fn scene_mesh(texture_key: &str) -> SceneMesh {
    let rects = [
        [-0.9f32, -0.3, 0.1, 0.9],
        [-0.1, 0.7, 0.1, 0.9],
        [-0.9, -0.1, -0.9, -0.1],
    ];
    let mut vertices = Vec::with_capacity(18);
    let mut draws = Vec::with_capacity(3);
    for (index, quad) in rects.iter().enumerate() {
        let [x0, x1, y0, y1] = *quad;
        let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
            position: [x, y],
            uv: [u, v],
            color: [1.0, 1.0, 1.0, 1.0],
            sdf: 0,
        };
        let start = vertices.len() as u32;
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y0, 1.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y1, 0.0, 1.0));
        let texture = if index == rects.len() - 1 { 1 } else { 0 };
        draws.push(SceneDraw::screen_ui(texture, start, 6));
    }
    let paths = vec![texture_key.to_owned()];
    SceneMesh {
        vertices,
        draws,
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths,
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
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
    canvas.set_width(crate::DESIGN_WIDTH);
    canvas.set_height(crate::DESIGN_HEIGHT);
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
    let mut host = GpuHost::new(&canvas, &host_canvas, scene_path).await?;
    // 起完栈先对齐一次几何：这样"frames=0"那条开屏读数与第一帧的取景就已经是同一个盒子，
    // 不必靠"第一帧跑完再看"（否则无头驱动在 frames=0 上读到的是设计分辨率的旧数）。
    host.fit_now();
    // 几何对齐之后再装输入监听器：监听器里的光标换算用的是 `fit_now` 写的映射，
    // 顺序反过来会有一小段"按初始盒换算"的窗口。
    host.install_input_listeners();
    // 起设备是**必须等**的（它决定这条通道能不能用，读数要当场拿到）；帧循环则不该等：
    // 它跑几百帧、每帧让出一个宏任务，同步跑完会把 JS 那条启动链一起摁住几百毫秒。
    // 所以这里把它派到事件循环上，`start()` 立刻继续跑游戏自己的启动。
    let width = host.width;
    let height = host.height;
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
        for index in 1..=budget {
            // 写状态属性之前**按需**刷一次世界读数：读数不进热路径（见 RustScene::refresh_world
            // 里的实测记录——自动刷新会把并行跑的 JS 剧本挤出帧预算）。放在 rame() 之前，
            // 这样本帧组出来的读数与本帧的世界一致。
            if index % STATUS_EVERY == 0 || index == budget {
                host.refresh_world_readout();
            }
            let frame = match host.frame() {
                Ok(frame) => frame,
                Err(error) => {
                    // 失败要**当场**写进属性并收工：沉默地停掉会让"通道坏了"看起来像"跑完了"。
                    set_status(&format!("failed frames={index} detail={error}"));
                    return;
                }
            };
            // 收工那一帧也要写一次：`budget` 可能来自 URL，与常量 `FRAME_BUDGET` 不是一回事。
            if index % STATUS_EVERY == 0 || index == budget {
                let elapsed_ms = loop_started.elapsed().as_secs_f64() * 1000.0;
                let avg_frame_ms = elapsed_ms / index as f64;
                // 读数取**本帧**的：最后一步写下的就是最后一帧的数字，因此不需要额外留一份"上一帧"。
                set_status(&format!(
                    "presented frames={index} draw_calls={} triangles={} avg_frame_ms={avg_frame_ms:.2} fps={:.1} {} canvas=#{GPU_CANVAS_ID} {width}x{height}",
                    frame.0,
                    frame.1,
                    1000.0 / avg_frame_ms,
                    frame.2
                ));
            }
            // 每帧之间让出宏任务：present 与合成由浏览器主循环推进（见 `yield_to_event_loop`）。
            yield_to_event_loop().await;
        }
    });
    Ok(opening)
}
