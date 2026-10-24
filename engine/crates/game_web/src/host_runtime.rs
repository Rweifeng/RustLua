//! WP8 engine host runtime: the wasm side of a host that owns its own canvas and GL context.
//!
//! ## Why this module exists
//!
//! The browser host (`gpu_host`) is driven by a page: it looks up `document`, a canvas element by
//! id, `window`, `location`. **WeChat has none of those** (`plugins/wechat-minigame/wechat-adapter.js`
//! shims a partial DOM, and the platform owns the canvas), so the WeChat path needs an entry that
//! takes the canvas as an argument and never touches a browser constructor. `probe_canvas_host`
//! proved that much is possible (a plain JS object can reach wgpu's WebGL backend); this module
//! turns that proof into a *runtime*: begin → frame(dt) → end.
//!
//! ## Contract (the two sides must not fight over the clock)
//!
//! * **JS owns the frame source.** `requestAnimationFrame` / `wx.requestAnimationFrame` decides
//!   when a frame happens and measures its interval; that is the only reliable timer on both
//!   platforms.
//! * **wasm owns the fixed-step policy.** `engine_host_frame(dt_ms, …)` feeds the interval into
//!   `rustlua_infra::frame_clock::FrameClock` — the *same* policy the browser path uses, including
//!   the interval clamp and the sub-ulp tolerance. Two hosts that step the simulation differently
//!   is exactly the kind of drift this refactor exists to remove.
//! * **Failures are named.** Every error leaving this module starts with `host: ` and says which
//!   stage rejected the canvas, so a device report can distinguish "no WebGL2" from "our path is
//!   wrong" without a debugger.
//!
//! ## Scope of this slice
//!
//! Begin/end, the fixed-step clock, per-frame clear + present, and a JSON state readout.
//! With `options.scene` set the frame goes through the **shared engine core**
//! (`crate::engine_core::EngineCore` — the same renderer + `RustScene` chain the browser host
//! uses), so the WeChat host can draw the real project scene instead of a clear color. Without
//! it the host stays the verified clear-color/pixel-sample shape (P0-① / takeover batches).
//! The input batch (`RLFI`) is carried through untouched but not decoded here against the
//! scene's Lua world — it still lands in `LATEST_FRAME`, which is what the core reads.
//!
//! The GPU half only exists in the `gpu2d` build (`package.ps1 -Gpu2d`): the default kernel has no
//! `wgpu` at all, and therefore no `engine_host_*` exports.

use std::cell::RefCell;

use rustlua_infra::frame_clock::{FRAME_INTERVAL_TOLERANCE_SECONDS, FrameClock};
use wasm_bindgen::prelude::*;

use crate::engine_host::{EngineHost, HostFrameStage};
use crate::host_config::HostConfig;
use crate::rust_scene::SimRoute;

/// Everything one engine host owns. Single-threaded by construction (wasm), so it lives in a
/// thread-local instead of being handed back to JS as an opaque pointer.
struct WechatHost {
    /// GPU 侧二选一：**clear-color 栈**（P0-①/接管形态，既有现场）或**共享核心**（`scene` 档，
    /// 真场景形态）。一张画布只会有一个 surface——两条分支绝不同时存在。
    gpu: HostGpu,
    /// 时钟配置（P1 第二片）：两档共用同一个 `HostConfig`，因此"默认跑几步、步长多少"
    /// 只有一处来源。`FrameClock` 本身在需要它的那一档里——clear 档由本外壳持有
    /// （P0-①/接管既有形态，逐字不动），scene 档在统一宿主 `EngineHost` 里。
    ///
    /// 步长必须按 f64 现算（`HostConfig::fixed_delta_seconds_f64`），**不要**用
    /// `FramePlan::delta_seconds`：后者是 f32（0.016666668），而 JS 侧喂给 Lua 的是 `1/60`
    /// 的 double，两者不是同一个数。
    config: HostConfig,
    /// `true` = 这个宿主负责跑 Lua 的固定步（`lua_fixed_run_steps`）。
    /// 默认 **false**：老 JS 世界每帧也在自己跑固定步，两边同时跑等于把世界推进两次。
    tick_lua: bool,
    /// 宿主已经推进的模拟步总数（真接模拟后这是最有用的一个读数）。
    tick_steps_total: u64,
    /// 固定步执行失败次数（失败原因沿 `host: tick: …` 抛出，同时也计数）。
    tick_failures: u64,
    /// clear 档的清屏色；scene 档由渲染器自己管清屏，这里不生效。
    clear: wgpu::Color,
    canvas_note: String,
    /// 后备缓冲尺寸（begin 时定；`engine_host_state` 与像素回读都读这里，不再摸 config）。
    width: u32,
    height: u32,
    /// scene 档的**几何读数**：`declared=设计盒 rect=宽x高+左+上`（渲染器这一帧把声明盒
    /// 等比居中摆进表面的实际矩形）。clear 档恒为空串——那条路只清屏，没有取景可言。
    fit_note: String,
    /// `pixel_sample` 开关（任务 #13 方案 A）：present 后同任务回读几个像素。
    /// 为什么必须是**引擎侧 readPixels**而不是 JS 的 `toDataURL`：wgpu-hal 27 建上下文只设
    /// `antialias:false`（`gles/web.rs:103`），`preserveDrawingBuffer` 保持默认 false ⇒
    /// 浏览器在合成之后就把绘制缓冲清空，事后 `toDataURL` 只会截到空白。present 之后、
    /// 本任务结束之前读像素是唯一可信窗口。
    pixel_sample: bool,
    /// 同一个 WebGL2 上下文（`getContext('webgl2')` 二次调用返回已存在的那个），只为回读。
    gl: Option<web_sys::WebGl2RenderingContext>,
    /// 最近一次回读的读数（`中心=… 左上=… …`）；未开启/未采过为空串。
    last_pixel_sample: String,
    presented_frames: u64,
    failed_presents: u64,
    input_frames: u64,
    input_bytes: u64,
    /// 真正交给 `lua_input_frame` 解码成功的批次数（与 `input_frames` 分开：后者含空帧）。
    input_applied: u64,
    last_input_note: String,
    last_note: String,
    /// scene 档最近一帧的场景读数（进 `engine_host_state` 的 `scene` 块；clear 档恒为空串）。
    last_scene: String,
    /// scene 档**本帧真正消费到的**那份输入的指纹（`latest_frame_input()` 的读数）。
    ///
    /// 注入者不是宿主：`web-runtime.js` 是输入注入的唯一所有者（它每帧采样、打包成 `RLFI`、
    /// 经 `lua_input_frame` 落进 Lua 常驻表并把解码结果缓存进 `LATEST_FRAME`），宿主只是从这里取。
    /// 为什么这条必须有读数：`engine_host_frame_into` 那两个计数器说的是**另一条通道**——
    /// `input_frames` 记"这个入口被调了几次"（接管形态 ≈ 帧数），`input_applied` 只记"随调用附带的
    /// 批量解码成功了几次"，而适配层在接管形态恒传 `undefined`（批量已经由 `lua_input_frame` 走掉）
    /// ⇒ `input_applied` 恒为 0。拿它判"接管形态有没有输入"会把"这条旁路没走"读成"没有输入"。
    scene_input_note: String,
}

/// GPU 侧的两条互斥形态。
enum HostGpu {
    /// clear-color 栈（P0-①/接管验收的既有形态）：每帧只清屏 + present。字段与抽取前同源
    /// （`config` 不存：configure 之后没人读它，尺寸读数在 `WechatHost.width/height`）。
    Clear {
        /// clear 档的时钟由**本外壳**持有：这条路是 P0-①/接管两批的既有现场，
        /// 行为逐字不动。scene 档的时钟在统一宿主里（见下面的 `Scene`）。
        clock: FrameClock,
        /// surface 配置。抽取前这里不存（configure 之后没人读）；有了 `engine_host_resize`
        /// 就必须留着——重配 surface 的三件事（尺寸、config、渲染视口）不能由调用方各改一半。
        config: wgpu::SurfaceConfiguration,
        /// 保活：surface/adapter 的生命周期挂在它上面。
        _instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
    },
    /// 真场景形态（P1 共享核心 + P1 第二片统一宿主）：与浏览器宿主**同一台宿主**——
    /// `EngineHost` = 帧时钟 + `EngineCore`（`Renderer` + `RustScene` step/mesh/贴图/取景/render/present）。
    Scene(EngineHost),
}

/// 宿主自产输入的那一份状态（见 [`HOST_INPUT`]）。
struct HostInputState {
    producer: crate::frame_input::FrameInputProducer,
    /// 上一帧的线格式：`take_frame` 用它算光标位移与"一直按住"的边沿。
    frame: rustlua_frame_abi::FrameInput,
}

thread_local! {
    static HOST: RefCell<Option<WechatHost>> = const { RefCell::new(None) };
    /// 浏览器外壳**接进来**的那份状态读数（`gpu_host::publish_engine_state` 每帧发布）。
    ///
    /// 为什么要有它：`engine_host_state()` 此前只在 `engine_host_begin` 建过宿主时才答得出来，
    /// 而离线假宿主给不出 WebGL2 ⇒ 宿主永远到不了 `running` ⇒ 微信那条路的读数**一条门都没有**
    /// （见 `tasks/plan/M73-01` §9）。浏览器**有** WebGL2，所以把它的读数接进同一个导出，等于给
    /// 这台宿主一个"有 GL 的可断言现场"——两边读到的形状由同一个
    /// [`crate::engine_host::EngineHost::scene_state_json`] 决定，不会再漂。
    static ADOPTED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// 宿主自己的输入生产者（B4）。
    ///
    /// 为什么必须有它：此前 `LATEST_FRAME` 的唯一写者是 `lua_bridge::lua_input_frame`，而它唯一
    /// 的调用者是旧 JS 运行时（`web-runtime.js` 的 `createWebInput` → `applyInputFrame`）。也就是
    /// **旧运行时不跑 ⇒ 宿主每帧拿 `FrameInput::default()`**：不崩溃、不报错、`presented` 照涨，
    /// 只是人钉死。删掉旧世界之前必须先把这条链搬进宿主。
    ///
    /// 复用 [`crate::frame_input::FrameInputProducer`]——与浏览器 DOM 那条路**同一个实现**，
    /// 边沿语义、落不进键表的键计数、`release_all` 都在它里面，并且有单测守着。
    /// `frame` 必须跨帧留着：`take_frame` 拿它当"上一帧"，整份重建会把"一直按住"变成每帧刚按下。
    static HOST_INPUT: RefCell<HostInputState> = RefCell::new(HostInputState {
        producer: crate::frame_input::FrameInputProducer::new(),
        frame: rustlua_frame_abi::FrameInput::new(),
    });
}

/// Defaults for `engine_host_begin`'s options JSON; also the documented contract.
const DEFAULT_UPDATES_PER_SECOND: u32 = 60;
const DEFAULT_MAX_CATCH_UP_STEPS: u32 = 8;
const DEFAULT_MAX_FRAME_SECONDS: f64 = 0.25;

fn host_error(stage: &str, detail: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&format!("host: {stage}: {detail}"))
}

fn report_line(report: &js_sys::Function, line: &str) {
    let _ = report.call1(&JsValue::NULL, &JsValue::from_str(line));
}

/// Starts an engine host on `gpu_canvas` (a **dedicated** canvas: a canvas has exactly one
/// context, so passing the 2D game canvas would destroy it).
///
/// `options_json` may be empty or `{}`; recognised keys and defaults:
/// `updates_per_second` (60), `max_catch_up_steps` (8), `max_frame_seconds` (0.25),
/// `clear` (`[0.06, 0.07, 0.09, 1]`), `canvas_width` / `canvas_height` (read from the canvas),
/// `pixel_sample` (false), `tick` (`"none"`|`"lua"`), `scene`（缺省 = clear-color 形态；给出
/// 场景相对路径则切进共享核心真场景形态）与 `sim`（路线名，缺省 `script`）。
///
/// **`sim` 可以单独给**：`sim=script`（`SimRoute::EngineStartup`）让**引擎自己**解析启动场景，
/// 它的 `scene_path` 只进读数，所以不必给 `scene` 也能进 scene 档——微信把默认翻到 Rust 宿主时
/// 走的就是这一档（发布形态不需要在适配层再写一份"启动场景是哪个"）。另外三条路线由宿主提供
/// 场景字节，缺 `scene` 仍然具名拒绝。
///
/// Returns a JSON summary. Failures reject with a `host: <stage>: <detail>` string and are also
/// pushed through `report`.
#[wasm_bindgen]
pub async fn engine_host_begin(
    gpu_canvas: JsValue,
    options_json: String,
    report: js_sys::Function,
) -> Result<String, JsValue> {
    // 诊断唯一安装点（见 `lib.rs::install_panic_hook` 的注释）：async 导出的 panic 落在
    // microtask pump 里，没有 hook 时现场只剩一句 `unreachable`——2026-10-12 微信模拟器
    // 六问探针就是靠补这一行才把真凶（panic 位置）捞出来的。
    crate::install_panic_hook();
    match begin_inner(gpu_canvas, &options_json, &report).await {
        Ok(summary) => Ok(summary),
        Err(error) => {
            let text = error.as_string().unwrap_or_else(|| format!("{error:?}"));
            report_line(&report, &text);
            Err(error)
        }
    }
}

async fn begin_inner(
    gpu_canvas: JsValue,
    options_json: &str,
    report: &js_sys::Function,
) -> Result<String, JsValue> {
    if gpu_canvas.is_null() || gpu_canvas.is_undefined() {
        return Err(host_error("canvas", "调用方没有给出画布对象"));
    }
    let options: serde_json::Value = if options_json.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(options_json).map_err(|error| host_error("options-json", error))?
    };
    let updates_per_second =
        option_u32(&options, "updates_per_second", DEFAULT_UPDATES_PER_SECOND)?;
    let max_catch_up_steps =
        option_u32(&options, "max_catch_up_steps", DEFAULT_MAX_CATCH_UP_STEPS)?;
    let max_frame_seconds = option_f64(&options, "max_frame_seconds", DEFAULT_MAX_FRAME_SECONDS)?;
    let clear = option_color(&options)?;
    // `pixel_sample`（任务 #13 方案 A）：present 后同任务回读像素。默认关——readPixels 是
    // 同步阻塞调用，不该白加在生产帧循环上；只有可见画面接管的验收场合显式打开。
    let pixel_sample = match options.get("pixel_sample") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(value)) => *value,
        Some(other) => {
            return Err(host_error(
                "options",
                format!("pixel_sample 只接受 true/false，给的是 {other}"),
            ));
        }
    };
    // `tick`：谁跑 Lua 的固定步。默认 `none` —— 老 JS 世界每帧也在跑，两边同时跑就是把世界推进两次。
    // `lua` = 由本宿主按自己的帧时钟推进（`lua_fixed_run_steps`），给"引擎宿主拥有整个世界"那条路用。
    let tick_lua = match options.get("tick") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(text)) if text == "none" => false,
        Some(serde_json::Value::String(text)) if text == "lua" => true,
        Some(other) => {
            return Err(host_error(
                "options",
                format!("tick 只接受 \"none\" 或 \"lua\"，给的是 {other}"),
            ));
        }
    };

    // 读画布尺寸**直接读属性**（与探针同款理由：`canvas_size()` 在 0 时会退回内建兜底尺寸，
    // 那会把"调用方给了一张没尺寸的画布"变成"我们悄悄用了别的尺寸"）。
    let canvas: web_sys::HtmlCanvasElement = gpu_canvas.clone().unchecked_into();
    let (mut width, mut height) = (canvas.width(), canvas.height());
    if let Some(value) = options
        .get("canvas_width")
        .and_then(serde_json::Value::as_u64)
    {
        width = value.min(u64::from(u32::MAX)) as u32;
    }
    if let Some(value) = options
        .get("canvas_height")
        .and_then(serde_json::Value::as_u64)
    {
        height = value.min(u64::from(u32::MAX)) as u32;
    }
    if width == 0 || height == 0 {
        return Err(host_error(
            "canvas",
            format!("{width}x{height}（尺寸为 0）"),
        ));
    }

    // `scene` 档（P1 共享核心）：给了就走与浏览器宿主**同一条**渲染链（Renderer + RustScene，
    // 阶段自报走 `report` 同一条通道）；没给就保持 clear-color 形态（P0-①/接管两批的既有现场，
    // 那条路径的建栈代码逐字不动，验收读数才有可比性）。
    let scene_spec = option_scene(&options)?;
    // scene 档与 `tick:"lua"` **互斥**：scene 模式下 `EngineCore::frame` 自己就按 plan.steps
    // 推进模拟（与浏览器宿主同序），宿主再跑一遍 `lua_fixed_run_steps` 就是把世界推进两次。
    if scene_spec.is_some() && tick_lua {
        return Err(host_error(
            "options",
            "tick=\"lua\" 不能与 scene 同时给：scene 档的固定步由共享核心的帧链推进",
        ));
    }
    // 时钟配置（P1 第二片）：两档共用同一个 `HostConfig`，`FrameClock` 由需要它的那一档持有——
    // clear 档由本外壳持有（P0-①/接管既有形态，逐字不动），scene 档由统一宿主
    // [`crate::engine_host::EngineHost`] 持有（与浏览器同一台宿主）。
    //
    // 校验放在**建栈之前**：写错的 `updates_per_second` 不该先建出一个 device/surface 再报错
    // （现场看起来像"GPU 起不来"）。这条具名失败与抽取前逐字相同。
    let config = HostConfig::new(updates_per_second, max_catch_up_steps, max_frame_seconds);
    let clear_clock = config
        .clock()
        .map_err(|error| host_error("frame-clock", error))?;
    let (gpu, adapter_display, backend_display, format_display, mode_display, fit_note) =
        match scene_spec {
            Some((scene_path, route)) => {
                let report_for_core = report.clone();
                let mut engine_host = EngineHost::build(
                    crate::engine_core::CoreArgs {
                        canvas: &gpu_canvas,
                        width,
                        height,
                        scene_path: &scene_path,
                        route,
                        route_note: String::new(),
                        device_label: "rustlua-engine-host-scene-device",
                        report: Box::new(move |line: &str| {
                            report_line(&report_for_core, line);
                        }),
                    },
                    config,
                )
                .await
                .map_err(|error| host_error("engine-core", error))?;
                // **逻辑视口 = 设计分辨率**（与浏览器宿主 `GpuHost::fit_now` 同一说法，任务 #20）：
                // 表面/后备缓冲是设备像素（接管形态下必须整屏），投影按设计盒算，渲染器再把设计盒
                // **等比居中**摆进表面（`rustlua_render::fit::contain_rect`）。此前这里把表面尺寸直接
                // 当逻辑视口 ⇒ 1 世界单位 = 1 设备像素，画面停在表面一角（run10 实测内容块
                // `1380×720` / 表面 `2532×1170`，非底色像素只占 31.1%）。
                let fit_note = fit_scene_viewport(&mut engine_host, width, height);
                let (adapter_display, backend_display, format_display) = {
                    let (adapter, backend, format) = engine_host.core.stack_info();
                    (adapter.to_owned(), backend.to_owned(), format)
                };
                (
                    HostGpu::Scene(engine_host),
                    adapter_display,
                    backend_display,
                    format_display,
                    // `scene` 可以缺省（`sim=script` 那条路由引擎自己解析启动场景）：这时读数
                    // 报 `scene(engine-startup)`，不要留一个空的 `scene()`。
                    if scene_path.is_empty() {
                        "scene(engine-startup)".to_owned()
                    } else {
                        format!("scene({scene_path})")
                    },
                    fit_note,
                )
            }
            None => {
                let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                    // wasm 上必须显式 GL：默认在 wasm 只有 webgpu（与 `gpu_host` / 探针同一口径）。
                    backends: wgpu::Backends::GL,
                    ..Default::default()
                });
                let surface = rustlua_gpu::web_surface::canvas_surface(&instance, &gpu_canvas)
                    .map_err(|error| host_error("canvas_surface", error))?;
                let adapter = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        compatible_surface: Some(&surface),
                        force_fallback_adapter: false,
                    })
                    .await
                    .map_err(|error| host_error("request_adapter", error))?;
                let info = adapter.get_info();
                let capabilities = surface.get_capabilities(&adapter);
                let format = wgpu::TextureFormat::Rgba8UnormSrgb;
                if !capabilities.formats.contains(&format) {
                    return Err(host_error(
                        "surface-format",
                        format!("不提供 {format:?}（给的是 {:?}）", capabilities.formats),
                    ));
                }
                let limits =
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
                let (device, queue) = adapter
                    .request_device(&wgpu::DeviceDescriptor {
                        label: Some("rustlua-engine-host-device"),
                        required_features: wgpu::Features::empty(),
                        experimental_features: wgpu::ExperimentalFeatures::disabled(),
                        required_limits: limits,
                        memory_hints: wgpu::MemoryHints::Performance,
                        trace: wgpu::Trace::Off,
                    })
                    .await
                    .map_err(|error| host_error("request_device", error))?;
                let config = wgpu::SurfaceConfiguration {
                    // 只要 RENDER_ATTACHMENT（理由同 `gpu_host`：画布纹理当不了 copy 源，申请 COPY_SRC
                    // 会让 wgpu 在未捕获的校验错误上 panic，wasm 直接 abort）。
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width,
                    height,
                    present_mode: wgpu::PresentMode::Fifo,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: capabilities.alpha_modes[0],
                    view_formats: vec![],
                };
                surface.configure(&device, &config);
                (
                    HostGpu::Clear {
                        clock: clear_clock,
                        config,
                        _instance: instance,
                        surface,
                        device,
                        queue,
                    },
                    info.name,
                    format!("{:?}", info.backend),
                    format!("{format:?}"),
                    "clear".to_owned(),
                    // clear 档没有逻辑视口可言（整张表面就是清屏色），读数留空，验收形态照旧。
                    String::new(),
                )
            }
        };

    // 回读开启时拿 wgpu-hal 在 create_surface 时就建好的**同一个** WebGL2 上下文：
    // WebGL 规范保证同一 canvas 上二次 `getContext('webgl2')` 返回已存在的那个，
    // 所以这里不会抢在 wgpu 之前建第二个上下文（那才是真正会毁掉 surface 的写法）。
    let gl = if pixel_sample {
        let context = canvas
            .get_context("webgl2")
            .map_err(|error| host_error("gl-context", format!("{error:?}")))?
            .ok_or_else(|| host_error("gl-context", "画布没有给出 WebGL2 上下文"))?;
        Some(
            context
                .dyn_into::<web_sys::WebGl2RenderingContext>()
                .map_err(|_| host_error("gl-context", "画布上已存在的上下文不是 WebGL2"))?,
        )
    } else {
        None
    };

    let canvas_note = format!("{width}x{height}");
    let summary = serde_json::json!({
        "ok": true,
        "canvas": canvas_note,
        "format": format_display,
        "adapter": adapter_display,
        "backend": backend_display,
        "mode": mode_display,
        "updates_per_second": updates_per_second,
        "max_catch_up_steps": max_catch_up_steps,
        "max_frame_seconds": max_frame_seconds,
        "tolerance_seconds": FRAME_INTERVAL_TOLERANCE_SECONDS,
        "tick": if tick_lua { "lua" } else { "none" },
        "pixel_sample": pixel_sample,
        "fit": fit_note,
        "frame_abi": HOST_FRAME_ABI,
        "frame_abi_version": HOST_FRAME_ABI_VERSION,
        "frame_abi_len": HOST_FRAME_SLOTS.len(),
        // docs/65 P0-⑥：begin 时刻的 wasm 线性内存基线（Rust 侧口径；JS 侧另有
        // `__RUSTLUA_WASM_MEMORY_BYTES`，两侧同问对拍用）。
        "wasm_mem_bytes": crate::wasm_memory_bytes(),
    });
    report_line(
        report,
        &format!(
            "host: 已就绪 canvas={canvas_note} fmt={format_display} adapter={adapter_display} backend={backend_display} tick={} fit={fit_note}",
            if tick_lua { "lua" } else { "none" }
        ),
    );
    HOST.with(|slot| {
        *slot.borrow_mut() = Some(WechatHost {
            gpu,
            config,
            tick_lua,
            tick_steps_total: 0,
            tick_failures: 0,
            clear,
            canvas_note,
            width,
            height,
            fit_note,
            pixel_sample,
            gl,
            last_pixel_sample: String::new(),
            presented_frames: 0,
            failed_presents: 0,
            input_frames: 0,
            input_bytes: 0,
            input_applied: 0,
            last_input_note: String::new(),
            last_note: String::new(),
            last_scene: String::new(),
            scene_input_note: String::new(),
        });
    });
    Ok(summary.to_string())
}

/// scene 档的几何收敛：把核心的**逻辑视口**收到设计分辨率，并返回这一帧的几何读数。
///
/// 为什么两侧都要"逻辑视口 ≠ 表面尺寸"：投影矩阵按声明盒取景，pass 视口再把声明盒
/// 等比居中映射进附件（`rustlua_render::fit::contain_rect`）。浏览器外壳靠 `fit_now`
/// 把后备缓冲缩到等比适配盒（表面宽高比 ≡ 设计比 ⇒ 矩形贴满、无偏移）；微信接管形态
/// 的表面必须是整屏设备像素（缩放由平台把画布拉到屏幕，改后备尺寸就是非等比拉伸），
/// 因此留边必须发生在**表面内部**——这一步就是那条路的入口。
///
/// 读数进 `engine_host_state.fit` 与 begin summary：几何错了在 `presented=`/`frames=`
/// 里完全看不见（#19 的 31.1% 覆盖率就是这么发现的），所以矩形本身必须是可读数的。
/// 收 `&mut` 而不是按值收发：`engine_host_resize` 要复用**同一处**实现（begin 时与 resize 时
/// 各写一套，就是"改尺寸之后取景悄悄不对"的经典成因）。
fn fit_scene_viewport(
    engine_host: &mut EngineHost,
    surface_width: u32,
    surface_height: u32,
) -> String {
    match engine_host.core.sim_mut() {
        Some(sim) => {
            sim.set_viewport(crate::DESIGN_WIDTH, crate::DESIGN_HEIGHT);
            let (x, y, width, height) = rustlua_render::fit::contain_rect(
                (crate::DESIGN_WIDTH as f32, crate::DESIGN_HEIGHT as f32),
                (surface_width, surface_height),
            );
            format!(
                "declared={}x{} rect={width}x{height}+{x}+{y}",
                crate::DESIGN_WIDTH,
                crate::DESIGN_HEIGHT
            )
        }
        // 核心自己退化成了内建档（场景没起来）：没有逻辑视口可收，读数留空。
        None => String::new(),
    }
}

/// 每帧状态的**定长 ABI** 槽位数。布局只有这一份来源（`engine_host_frame_layout_json` 把它交给 JS，
/// 与 `input_frame_layout_json` 同一套做法）。
const HOST_FRAME_LEN: usize = 16;

/// 定长 ABI 的标识与版本（布局变了就改版本，别让 JS 猜）。
const HOST_FRAME_ABI: &str = "rustlua-engine-host-frame";
const HOST_FRAME_ABI_VERSION: u32 = 1;

/// 槽位名（顺序 = 下标）。热路径每帧只搬一个 `Float64Array`：诊断通道不该在每帧付
/// 序列化 + `JSON.parse` 的钱，定长槽位还让"哪个字段在第几格"变成可断言的契约。
const HOST_FRAME_SLOTS: [&str; HOST_FRAME_LEN] = [
    "frame_index",
    "steps",
    "steps_total",
    "delta_ms",
    "elapsed_ms",
    "accumulated_ms",
    "dropped_steps",
    "dropped_total",
    "clamped",
    "presented_total",
    "failed_presents",
    "input_frames",
    "input_bytes",
    "input_applied",
    "tick_steps_total",
    "tick_failures",
];

/// 一帧的全部逻辑（**唯一实现**）：输入 → 固定步 → 清屏呈现 → 打包成定长数组。
///
/// JSON 入口与定长入口都从这一份结果出发 ⇒ 两个门面不可能报出不一样的读数。
///
/// `input` 是浏览器路径交给 `lua_input_frame` 的**同一份** `RLFI` 定长批量（`Uint8Array`）；
/// `undefined`/`null` = 这一帧没有输入；其它类型具名拒绝。
///
/// 失败一律具名：`host: dt-ms` / `host: input` / `host: not-begun` / `host: input-frame` /
/// `host: tick` / `host: present`。固定步失败**不呈现**这一帧（计入 `tick_failures`）；
/// 呈现失败保留宿主并计入 `failed_presents`，让调用方自己决定要不要重建。
fn frame_core(dt_ms: f64, input: JsValue) -> Result<[f64; HOST_FRAME_LEN], JsValue> {
    if !dt_ms.is_finite() {
        return Err(host_error("dt-ms", format!("不是有限数：{dt_ms}")));
    }
    // 输入批量：只认 `Uint8Array`（RLFI）或"这一帧没有输入"。
    // 这里对 `Uint8Array` 做 instanceof 判定是**可以**的：它是每个 JS 引擎都有的语言内建，
    // 与 `HTMLCanvasElement` 那种"宿主才有的构造器"完全不是一回事（后者是小游戏上必须消灭的依赖）。
    let batch: Option<Vec<u8>> = if input.is_undefined() || input.is_null() {
        None
    } else if input.is_instance_of::<js_sys::Uint8Array>() {
        // `unchecked_into` 是 `JsCast` 的方法（接收者在**前**）：第一版写成
        // `js_sys::Uint8Array::unchecked_into(input)` 编不过（E0308，接收者被当成 Uint8Array）。
        // instanceof 已经问过一遍，这里只做类型搬运、不再做构造器判定。
        Some(input.unchecked_into::<js_sys::Uint8Array>().to_vec())
    } else {
        return Err(host_error(
            "input",
            "只接受 Uint8Array（RLFI 批量）或 undefined/null（这一帧没有输入）",
        ));
    };
    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let host = borrow.as_mut().ok_or_else(|| {
            host_error("not-begun", "先调 engine_host_begin（画布与设备由它建立）")
        })?;
        host.input_frames += 1;
        if let Some(bytes) = batch {
            host.input_bytes += bytes.len() as u64;
            // 与浏览器宿主**同一个**解码入口（`lua_bridge::lua_input_frame`）：原地改常驻表
            // `__rustlua_input`。它要求 Lua 已 boot（没 boot 就是具名的 `host: input-frame: …`）。
            let touched = crate::lua_bridge::lua_input_frame(&bytes).map_err(|error| {
                host_error(
                    "input-frame",
                    error.as_string().unwrap_or_else(|| format!("{error:?}")),
                )
            })?;
            host.input_applied += 1;
            host.last_input_note = format!("{touched} 项 / {} 字节", bytes.len());
        }
        // 一帧的**计划**与**呈现**在两档各有一处来源（这是本文件唯一还按档分叉的地方）：
        //   clear 档 = 本外壳自持时钟（P0-①/接管两批的既有现场，逐字不动）；
        //   scene 档 = 统一宿主 `EngineHost`（与浏览器**同一台宿主**，P1 第二片）。
        // 抽取这一层之前两档共用同一个外壳时钟；scene 档改用统一宿主之后，时钟必须跟着搬进
        // **它真正被规划的那一档**——否则状态读数会去读一个从不推进的钟（恒 0 的"仪器说谎"）。
        let plan = match &mut host.gpu {
            HostGpu::Clear {
                clock,
                surface,
                device,
                queue,
                // `_instance` 只保活（surface/adapter 的生命周期挂在它上面），这一档不读它。
                ..
            } => {
                let plan = clock.plan(dt_ms / 1000.0);
                // 固定步：只有 `tick: "lua"` 时由**本宿主**推进（老 JS 世界每帧也在跑，
                // 两边同时跑就是把世界推进两次——所以默认 `none`）。`tick: "lua"` 与 scene 档
                // 互斥（begin 里已具名拒绝），所以这一段只可能落在 clear 档。
                if host.tick_lua && plan.steps > 0 {
                    // 步长按 f64 现算，且**只有一处来源**（`HostConfig`）：JS 喂给 Lua 的是
                    // `1/60` 的 double，而 `FramePlan::delta_seconds` 是 f32（0.016666668），
                    // 两者不是同一个数。
                    let delta = host.config.fixed_delta_seconds_f64();
                    match crate::lua_bridge::lua_fixed_run_steps(plan.steps, delta) {
                        // 返回值是**最后一个 update 的脚本结果**（不是步数）：宿主这里只要记账，
                        // 步数就是刚规划出来的 `plan.steps`。
                        Ok(_) => host.tick_steps_total += u64::from(plan.steps),
                        Err(error) => {
                            host.tick_failures += 1;
                            return Err(host_error(
                                "tick",
                                error.as_string().unwrap_or_else(|| format!("{error:?}")),
                            ));
                        }
                    }
                }
                let frame = match surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(error) => {
                        host.failed_presents += 1;
                        return Err(host_error("present", error));
                    }
                };
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rustlua-engine-host-frame"),
                });
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("rustlua-engine-host-clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(host.clear),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                queue.submit(Some(encoder.finish()));
                frame.present();
                // WebGL2 没有阻塞能力：显式 poll 一次，让这一帧的校验错误在这里暴露，而不是变成下一帧的谜题。
                let _ = device.poll(wgpu::PollType::Poll);
                plan
            }
            HostGpu::Scene(engine_host) => {
                // 场景输入与浏览器宿主**同一个回读口**：`lua_input_frame` 把批量落进
                // `LATEST_FRAME`（按住集合是权威，本帧没批量 = 状态不变）。核心内部
                // 已含 present 与显式 poll（与 clear 档同一口径）。
                let input = crate::lua_bridge::latest_frame_input();
                // 本帧消费的指纹（判"输入到底进没进宿主"的唯一现场证据，见字段注释）。
                host.scene_input_note = format!(
                    "frame={} axis={:.2},{:.2} cursor={:.0},{:.0} held={} press={} mouse={}",
                    input.frame,
                    input.axis[0],
                    input.axis[1],
                    input.cursor[0],
                    input.cursor[1],
                    input.keys_held.count_ones(),
                    input.keys_pressed.count_ones(),
                    input.mouse_held.count_ones(),
                );
                // 计划与渲染都交给统一宿主：外壳不再自己算"这一帧跑几步"。
                let outcome = engine_host.frame(dt_ms / 1000.0, &input);
                // 呈现失败的账**只有一个来源**（统一宿主自己的计数），外壳照抄不重算。
                host.failed_presents = engine_host.failed_presents();
                match outcome {
                    Ok(host_frame) => {
                        let out = host_frame.out;
                        // 读数只进 `engine_host_state` 的 `scene` 块：**定长 ABI 的 16 槽不许动**
                        // （版本没变就是契约没变，这是那套槽位存在的全部理由）。
                        if let Some(reason) = engine_host.core.pending_step_failure.take() {
                            host.last_scene = format!("scene_step_failed:{reason}");
                        } else if out.builtin {
                            host.last_scene = format!("builtin {}", engine_host.core.sim_note);
                        } else {
                            // `out.scene_note` 是场景自报（名字/实体数/来源）——**必须进读数**：
                            // 引擎 loader 失败时会静默换内置诊断场景，只报 `rust(boot ok)`
                            // 会把"起了个假场景"说成"起了真场景"。
                            host.last_scene = format!(
                                "{} {} draws={} tris={} scene_delta={:+},{:+},{:+}",
                                engine_host.core.sim_note,
                                out.scene_note,
                                out.draw_calls,
                                out.triangles,
                                out.scene_delta[0],
                                out.scene_delta[1],
                                out.scene_delta[2]
                            );
                        }
                        host_frame.plan
                    }
                    Err(error) => {
                        // 分段命名与抽取前逐字相同：取不到表面纹理那族是 `present`，
                        // 其余是 `scene-frame`。
                        let stage = match error.stage {
                            HostFrameStage::Present => "present",
                            HostFrameStage::SceneFrame => "scene-frame",
                        };
                        return Err(host_error(stage, error.detail));
                    }
                }
            }
        };
        // 时钟累计读数也按档取：scene 档的钟在统一宿主里，读外壳那个从不推进的钟会恒为 0。
        let (clock_steps_total, clock_dropped_total) = match &host.gpu {
            HostGpu::Clear { clock, .. } => (clock.total_steps(), clock.total_dropped_steps()),
            HostGpu::Scene(engine_host) => {
                let clock = engine_host.clock();
                (clock.total_steps(), clock.total_dropped_steps())
            }
        };
        host.presented_frames += 1;
        if host.pixel_sample {
            host.last_pixel_sample = read_pixel_sample(host.gl.as_ref(), host.width, host.height);
        }
        host.last_note = if plan.clamped {
            format!("clamped({:.3}s)", plan.elapsed_seconds)
        } else {
            String::new()
        };
        // 定长结果：**顺序必须与 `HOST_FRAME_SLOTS` 一致**（布局 JSON 就是这份顺序的唯一来源）。
        Ok([
            plan.frame_index as f64,
            f64::from(plan.steps),
            clock_steps_total as f64,
            f64::from(plan.delta_seconds) * 1000.0,
            plan.elapsed_seconds * 1000.0,
            plan.accumulator_seconds * 1000.0,
            plan.dropped_steps as f64,
            clock_dropped_total as f64,
            if plan.clamped { 1.0 } else { 0.0 },
            host.presented_frames as f64,
            host.failed_presents as f64,
            host.input_frames as f64,
            host.input_bytes as f64,
            host.input_applied as f64,
            host.tick_steps_total as f64,
            host.tick_failures as f64,
        ])
    })
}

/// 每帧状态的定长 ABI 布局（`abi` / `version` / `len` / `slots`）——JS 侧偏移的**唯一来源**。
#[wasm_bindgen]
pub fn engine_host_frame_layout_json() -> String {
    serde_json::json!({
        "abi": HOST_FRAME_ABI,
        "version": HOST_FRAME_ABI_VERSION,
        "len": HOST_FRAME_LEN,
        "slots": HOST_FRAME_SLOTS,
    })
    .to_string()
}

/// 跑一帧并把定长状态写进 `out`（至少 [`HOST_FRAME_LEN`] 个 `f64`），返回写入的槽位数。
///
/// 这是**每帧**该走的路：没有字符串、没有 JSON、没有 `JSON.parse`。
/// `out` 太短 ⇒ `host: frame-out: …`（**先于**一切状态检查：入参不合法就该在碰设备之前说清楚）。
#[wasm_bindgen]
pub fn engine_host_frame_into(dt_ms: f64, input: JsValue, out: &mut [f64]) -> Result<u32, JsValue> {
    if out.len() < HOST_FRAME_LEN {
        return Err(host_error(
            "frame-out",
            format!(
                "至少要 {HOST_FRAME_LEN} 个 f64 槽位（{} 的布局），给的是 {}",
                HOST_FRAME_ABI,
                out.len()
            ),
        ));
    }
    let values = frame_core(dt_ms, input)?;
    out[..HOST_FRAME_LEN].copy_from_slice(&values);
    Ok(HOST_FRAME_LEN as u32)
}

/// 跑一帧并返回 JSON（与 [`engine_host_frame_into`] **同一份**结果，仅供诊断/测试）。
///
/// 键名来自 `HOST_FRAME_SLOTS`，另加 `abi`/`version`；不额外塞任何别处来的字段——
/// 两个门面报不同的读数是最难查的一类"仪器说谎"。
#[wasm_bindgen]
pub fn engine_host_frame(dt_ms: f64, input: JsValue) -> Result<String, JsValue> {
    let values = frame_core(dt_ms, input)?;
    let mut map = serde_json::Map::with_capacity(HOST_FRAME_LEN + 3);
    map.insert(
        "abi".to_string(),
        serde_json::Value::String(HOST_FRAME_ABI.to_string()),
    );
    map.insert(
        "version".to_string(),
        serde_json::Value::from(HOST_FRAME_ABI_VERSION),
    );
    for (index, name) in HOST_FRAME_SLOTS.iter().enumerate() {
        map.insert((*name).to_string(), serde_json::Value::from(values[index]));
    }
    Ok(serde_json::Value::Object(map).to_string())
}

/// 不渲染，只读当前状态（JSON；`frame` 那个入口是每帧读数，这个是"现在在哪儿"）。
#[wasm_bindgen]
pub fn engine_host_state() -> Result<String, JsValue> {
    // ① 浏览器外壳接进来了 ⇒ 答它那份（整份原样，见 `publish_adopted_state`）。
    if let Some(adopted) = ADOPTED.with(|slot| slot.borrow().clone()) {
        return Ok(adopted);
    }
    // ② 否则答 `engine_host_begin` 建的那台（微信/引擎宿主外壳）。
    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let host = borrow.as_mut().ok_or_else(|| {
            host_error("not-begun", "先调 engine_host_begin（画布与设备由它建立）")
        })?;
        // scene 档的读数**只进 JSON 状态**（定长 ABI 的 16 槽不动）：`last` 是每帧拼好的
        // 场景读数，`sim`/`textures` 是核心自己的账（与浏览器状态属性同一批字段名同源）。
        let (mode, scene_block) = match &mut host.gpu {
            HostGpu::Clear { .. } => ("clear", serde_json::Value::Null),
            HostGpu::Scene(engine_host) => {
                // `world`（主角坐标 + 相对原点的位移）在**读态这一刻**现刷，不挂在每帧链上。
                // 两条理由：① `RustScene::refresh_world` 的实测教训写在它自己的注释里——热路径上
                // 多一次 Lua 求值会挤掉同机并行剧本的帧预算（浏览器宿主因此只在写状态属性那一帧刷）；
                // ② 接管形态没有状态属性可写，而 `engine_host_state` 是诊断口（适配层只把它挂在
                // `wx.__rustluaEngineHostState` 上，产品帧循环不调）⇒ "有人问才刷"比"每 30 帧刷一次"
                // 更省，也保证读到的就是读的那一帧。为什么必须有这根线：`scene_input` 只证到"输入进到
                // 了宿主消费的那一帧"，而"世界因此动了没有"要的是**位移**读数（`moved=dx,dy`，
                // 绝对坐标证明不了这一帧有没有动过——见 `RustScene::world_origin` 的注释）。
                // 刻意不开浏览器那条 `rustlua-world-expr=<任意 Lua>`：那条通道的"只读"靠调用方自觉、
                // 无护栏（plan §WP6-06 未完成① 就记着"微信侧若复用需补"），而这里的读数用的是引擎
                // 自己的 `entity.find`/`transform.get`，构造上不写世界。
                let world_note = match engine_host.core.sim_mut() {
                    Some(sim) => {
                        sim.refresh_world();
                        sim.world_note().to_owned()
                    }
                    // 核心起不来时 `sim` 是 None（画面退回内建场景），读数要说"没场景"而不是空串。
                    None => "world=no-scene".to_owned(),
                };
                (
                    "scene",
                    // 与浏览器外壳（`data-rustlua-engine-host`）**同一个实现**。本片之前两边各拼
                    // 一份，而同一个 `textures` 已经漂成"数组 vs 字符串"两种形状——同一台宿主的
                    // 同一件事有两套形状，正是"两侧都看着正常、直到有人拿一边的字段读另一边"。
                    engine_host.scene_state_json(&world_note, &host.last_scene),
                )
            }
        };
        // 时钟读数按档取（**不是同一个来源**）：clear 档读外壳自己的钟，scene 档读统一宿主的钟。
        // 抽取前这里只有一处 `host.clock`——scene 档改用统一宿主之后，再读那个从不推进的钟
        // 会让 `frame` / `steps_total` 恒为 0，而那是最难查的一类"仪器说谎"。
        let (clock_frame, clock_steps, clock_dropped, clock_accumulated_ms, clock_clamped) =
            match &host.gpu {
                HostGpu::Clear { clock, .. } => (
                    clock.frame_index(),
                    clock.total_steps(),
                    clock.total_dropped_steps(),
                    clock.accumulator_seconds() * 1000.0,
                    clock.clamped_frames(),
                ),
                HostGpu::Scene(engine_host) => {
                    let clock = engine_host.clock();
                    (
                        clock.frame_index(),
                        clock.total_steps(),
                        clock.total_dropped_steps(),
                        clock.accumulator_seconds() * 1000.0,
                        clock.clamped_frames(),
                    )
                }
            };
        let state = serde_json::json!({
            // 谁答的：两个外壳都可能答这个导出（浏览器经 `ADOPTED`、引擎宿主答自己那台）。
            // 读的人据此判断后面那些字段属于哪一套——不写这一项，两套会看起来一模一样。
            "shell": "engine-host",
            "abi": HOST_FRAME_ABI,
            "version": HOST_FRAME_ABI_VERSION,
            "frame": clock_frame,
            "steps_total": clock_steps,
            "dropped_total": clock_dropped,
            "accumulated_ms": clock_accumulated_ms,
            "clamped_frames": clock_clamped,
            "presented_total": host.presented_frames,
            "failed_presents": host.failed_presents,
            "input_frames": host.input_frames,
            "input_bytes": host.input_bytes,
            "input_applied": host.input_applied,
            "tick": if host.tick_lua { "lua" } else { "none" },
            "tick_steps_total": host.tick_steps_total,
            "tick_failures": host.tick_failures,
            "updates_per_second": host.config.updates_per_second,
            "canvas": host.canvas_note,
            "fit": host.fit_note,
            "input_note": host.last_input_note,
            "scene_input": host.scene_input_note,
            "note": host.last_note,
            "pixel_sample": host.last_pixel_sample,
            "mode": mode,
            "scene": scene_block,
            "size": format!("{}x{}", host.width, host.height),
            // docs/65 P0-⑥：随时可采的线性内存读数（与 begin summary 同一口径）。
            "wasm_mem_bytes": crate::wasm_memory_bytes(),
        });
        Ok(state.to_string())
    })
}

// ── 宿主自己的输入生产者（B4）：事件进、每帧一次 flush 出 ──────────────────────────
//
// 形状是"事件级导出 + 每帧 flush"，不是每个事件直接写 `LATEST_FRAME`：边沿是**每帧量**
// （`take_frame` 取走即清空），按事件写会把同一次点按在之后每一帧重复发出去——
// 症状是"点一下角色一直走"，而读数上完全看不出来。

/// 键按下/抬起。键名用线格式名（`frame_input::wire_key_name` 那张表，与旧运行时同一张）。
///
/// 返回 `false` = 这个名字落不进线格式（已计数丢弃，读数见 `engine_host_input_flush` 的
/// `dropped_keys`）。**刻意不静默**：那正是"某个键按了没反应"的经典成因。
#[wasm_bindgen]
pub fn engine_host_input_key(key: &str, down: bool) -> bool {
    HOST_INPUT.with(|slot| {
        let mut state = slot.borrow_mut();
        if down {
            state.producer.key_down(key)
        } else {
            state.producer.key_up(key)
        }
    })
}

/// 把 JS 给的指针标识收敛到合法槽位。
///
/// JS 侧给的是 `touch.identifier` / `pointerId`（number，可能是 0/负数/NaN）。这里**钳到
/// 1..=MAX_POINTERS**：宁可把异常值都归到第一根手指，也不要造出一个 `u64::MAX`——那是鼠标的保留槽，
/// 撞上会让"鼠标 + 手指"变成同一根。
fn pointer_slot(raw: f64) -> u64 {
    if !raw.is_finite() || raw < 1.0 {
        return 1;
    }
    (raw as u64).min(rustlua_frame_abi::MAX_POINTERS as u64)
}

/// 一根手指按下（多指）。与 [`engine_host_input_pointer`] 的区别是**带 id**：那条是"光标动了一下"
/// （单指语义），这条是"某根手指按下"——线格式的按下点数组靠它才装得下两根手指同时按。
#[wasm_bindgen]
pub fn engine_host_input_pointer_down(id: f64, x: f64, y: f64) {
    HOST_INPUT.with(|slot| {
        slot.borrow_mut()
            .producer
            .pointer_down(pointer_slot(id), [x, y]);
    });
}

/// 某根手指移动（按下点不变，只更新它的当前位置）。
#[wasm_bindgen]
pub fn engine_host_input_pointer_move(id: f64, x: f64, y: f64) {
    HOST_INPUT.with(|slot| {
        slot.borrow_mut()
            .producer
            .pointer_move(pointer_slot(id), [x, y]);
    });
}

/// 某根手指抬起。
#[wasm_bindgen]
pub fn engine_host_input_pointer_up(id: f64) {
    HOST_INPUT.with(|slot| {
        slot.borrow_mut().producer.pointer_up(pointer_slot(id));
    });
}

/// 指针移动到**设计坐标**（换算成设计坐标是几何的事，由调用方做——与浏览器 DOM 那条路同规矩）。
#[wasm_bindgen]
pub fn engine_host_input_pointer(x: f64, y: f64) {
    HOST_INPUT.with(|slot| slot.borrow_mut().producer.pointer_moved([x, y]));
}

/// 指针按钮按下/抬起（`"left"`/`"right"`/`"middle"`）。
#[wasm_bindgen]
pub fn engine_host_input_button(button: &str, down: bool) {
    HOST_INPUT.with(|slot| {
        let mut state = slot.borrow_mut();
        if down {
            state.producer.mouse_down(button);
        } else {
            state.producer.mouse_up(button);
        }
    });
}

/// 虚拟摇杆轴（0/1 = 左摇杆 X/Y），限幅到设备轴的有效范围。
#[wasm_bindgen]
pub fn engine_host_input_axis(axis: u32, value: f64) {
    HOST_INPUT.with(|slot| {
        slot.borrow_mut()
            .producer
            .gamepad_axis(axis as usize, value)
    });
}

/// 丢焦点/切后台时释放所有按住项。**必须有**：漏掉一次抬起会让角色一直走（或一直攻击），
/// 而这种故障在读数上只表现为"世界还在动"。
#[wasm_bindgen]
pub fn engine_host_input_release_all() {
    HOST_INPUT.with(|slot| slot.borrow_mut().producer.release_all());
}

/// 把本帧攒下的事件取成一份线格式并落到 `LATEST_FRAME`（**每帧调用一次**，见上面那段说明）。
///
/// 返回本帧读数 JSON（帧号 / 按住与按下位数 / 落不进键表的键数），供适配层自报做证据——
/// "输入进了宿主"从此是一个可断言的数，而不是靠手感。
#[wasm_bindgen]
pub fn engine_host_input_flush() -> String {
    HOST_INPUT.with(|slot| {
        let mut state = slot.borrow_mut();
        let mut frame = state.frame;
        state.producer.take_frame(&mut frame);
        state.frame = frame;
        // 落点与旧运行时那条路**同一个**（`LATEST_FRAME`）：世界只有一个输入所有者。
        crate::lua_bridge::set_latest_frame(frame);
        serde_json::json!({
            "frame": frame.frame,
            "keys_held": frame.keys_held.count_ones(),
            "keys_pressed": frame.keys_pressed.count_ones(),
            "mouse_held": frame.mouse_held.count_ones(),
            "dropped_keys": state.producer.dropped_keys(),
        })
        .to_string()
    })
}

/// 画布尺寸变化后把宿主切到新尺寸（P1 第二片 · 生产接线）。
///
/// 为什么必须由宿主自己做：surface 的 config、渲染目标尺寸、取景矩形与像素回读的采样尺寸是
/// **四件必须同一帧改完**的事——只改一半就会出现"按旧尺寸渲、往新尺寸显"，或者回读读到旧坐标。
/// 抽取前这几件事由旧 JS 世界分散处理；默认翻到 Rust 宿主之后这里是唯一入口，否则窗口一变化
/// surface 与画面比例就永久失配（而 `presented=`/`frames=` 全都正常，属于读数看不出来的错）。
///
/// `width`/`height` 是**画布后备缓冲的设备像素尺寸**（调用方先写 `canvas.width/height` 再调它）。
/// 返回读数 JSON（`size` + `fit`）；失败具名 `host: resize: …` / `host: not-begun: …`。
#[wasm_bindgen]
pub fn engine_host_resize(width: u32, height: u32) -> Result<String, JsValue> {
    if width == 0 || height == 0 {
        return Err(host_error(
            "resize",
            format!("{width}x{height}（尺寸为 0）"),
        ));
    }
    HOST.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let host = borrow.as_mut().ok_or_else(|| {
            host_error("not-begun", "先调 engine_host_begin（画布与设备由它建立）")
        })?;
        let fit_note = match &mut host.gpu {
            HostGpu::Clear {
                surface,
                device,
                config,
                ..
            } => {
                config.width = width;
                config.height = height;
                surface.configure(device, config);
                // clear 档没有逻辑视口可言（整张表面就是清屏色），与 begin 一致地留空。
                String::new()
            }
            HostGpu::Scene(engine_host) => {
                engine_host.core.resize(width, height);
                // 取景矩形按**新**表面尺寸重算；这一步同时把设计分辨率重新设回逻辑视口，
                // 与 begin 走的是同一处实现。
                fit_scene_viewport(engine_host, width, height)
            }
        };
        host.width = width;
        host.height = height;
        host.fit_note = fit_note.clone();
        Ok(serde_json::json!({
            "ok": true,
            "size": format!("{width}x{height}"),
            "fit": fit_note,
        })
        .to_string())
    })
}

/// 浏览器外壳把它的状态读数发布进来（见 [`ADOPTED`] 的说明）。
///
/// **语义是"整份读数原样替换"**：调用方给什么，`engine_host_state()` 就答什么——所以浏览器
/// 写进 DOM 属性的那个字符串与这个导出的返回值是**同一个**（同一处拼、同一处发布），
/// 门断言属性就等于断言导出。
pub(crate) fn publish_adopted_state(state: String) {
    ADOPTED.with(|slot| *slot.borrow_mut() = Some(state));
}

/// Drops the host (surface, device and queue). Safe to call when nothing is running.
#[wasm_bindgen]
pub fn engine_host_end() {
    HOST.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// 用 `readPixels` 采 5 个点：中心 + 四角（各内缩 1px）。坐标按 GL 口径（原点在**左下**），
/// 命名按屏幕口径（"左上"= 屏幕上看到的左上）。
///
/// 每点一次 1x1 回读，依赖的唯一前提是"present 之后、同一 JS 任务之内，默认帧缓冲里就是
/// 刚呈现的内容"（`preserveDrawingBuffer:false` 清空发生在本任务结束后的合成时点）。
/// 这条口径在 #19 用字节对拍钉过：同一形态下 JS **事后** `toDataURL` 多数给整幅
/// `RGBA(0,0,0,0)`（清空后的缓冲），偶尔给到活画面（落在清空之前）⇒ 事后回读是竞态，不能当验收通道。
/// 失败**写进读数而不是 panic**：这是验收读数通道，不该把帧循环带崩。
fn read_pixel_sample(
    gl: Option<&web_sys::WebGl2RenderingContext>,
    width: u32,
    height: u32,
) -> String {
    let Some(gl) = gl else {
        return "no-gl-context".to_string();
    };
    let inset_x = width.saturating_sub(2).max(1);
    let inset_y = height.saturating_sub(2).max(1);
    let points: [(&str, u32, u32); 5] = [
        ("中心", width / 2, height / 2),
        ("左上", 1, inset_y),
        ("右上", inset_x, inset_y),
        ("左下", 1, 1),
        ("右下", inset_x, 1),
    ];
    let mut parts = Vec::with_capacity(points.len());
    for (name, x, y) in points {
        let mut pixel = [0u8; 4];
        let read = gl.read_pixels_with_opt_u8_array(
            x as i32,
            y as i32,
            1,
            1,
            web_sys::WebGl2RenderingContext::RGBA,
            web_sys::WebGl2RenderingContext::UNSIGNED_BYTE,
            Some(&mut pixel),
        );
        if read.is_ok() {
            parts.push(format!(
                "{name}=[{},{},{},{}]",
                pixel[0], pixel[1], pixel[2], pixel[3]
            ));
        } else {
            parts.push(format!("{name}=read-failed"));
        }
    }
    parts.join(" ")
}

/// `scene` / `sim` 选项（P1 共享核心切片）：给了 `scene`（非空字符串 = 启动场景的工程相对路径）
/// 就把本宿主切进真场景形态；`sim` 是模拟路线名（与浏览器 `rustlua-sim` 同源，缺省 `script`）。
///
/// 只给 `sim` 不给 `scene` ⇒ **具名拒绝**而不是静默忽略："说了走哪条路线却没走成"是意图落空，
/// 与"某个键按了没反应"同族，宁可在这里响亮失败。
fn option_scene(options: &serde_json::Value) -> Result<Option<(String, SimRoute)>, JsValue> {
    let route_name = match options.get("sim") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(other) => {
            return Err(host_error(
                "options",
                format!("sim 只接受字符串（路线名），给的是 {other}"),
            ));
        }
    };
    let scene_path = match options.get("scene") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(serde_json::Value::String(_)) => {
            return Err(host_error("options", "scene 不能是空字符串"));
        }
        Some(other) => {
            return Err(host_error(
                "options",
                format!("scene 只接受非空字符串（场景资源路径），给的是 {other}"),
            ));
        }
    };
    match (scene_path, route_name) {
        (Some(path), name) => Ok(Some((
            path,
            SimRoute::from_name(&name.unwrap_or_else(|| "script".to_owned())),
        ))),
        // 只给 `sim` 不给 `scene`：只有"引擎自己解析启动场景"那一条路合法——它的 `scene_path`
        // 只进读数（见 `RustScene::boot` 的 `EngineStartup` 分支），所以宿主可以不带场景路径
        // 直接进 scene 档。另外三条路由**宿主提供场景字节**，缺 `scene` 是真的无从谈起 ⇒
        // 仍然具名拒绝，而不是静默退成 clear 档。
        (None, Some(name)) => {
            let route = SimRoute::from_name(&name);
            if route.loads_engine_startup() {
                Ok(Some((String::new(), route)))
            } else {
                Err(host_error(
                    "options",
                    format!("sim={name} 需要同时给 scene：这条路线由宿主提供场景字节"),
                ))
            }
        }
        (None, None) => Ok(None),
    }
}

fn option_u32(options: &serde_json::Value, key: &str, fallback: u32) -> Result<u32, JsValue> {
    match options.get(key) {
        None | Some(serde_json::Value::Null) => Ok(fallback),
        Some(value) => value
            .as_u64()
            .filter(|raw| *raw > 0 && *raw <= u64::from(u32::MAX))
            .map(|raw| raw as u32)
            .ok_or_else(|| host_error("options", format!("{key} 必须是正整数，给的是 {value}"))),
    }
}

fn option_f64(options: &serde_json::Value, key: &str, fallback: f64) -> Result<f64, JsValue> {
    match options.get(key) {
        None | Some(serde_json::Value::Null) => Ok(fallback),
        Some(value) => value
            .as_f64()
            .filter(|raw| raw.is_finite() && *raw > 0.0)
            .ok_or_else(|| host_error("options", format!("{key} 必须是正有限数，给的是 {value}"))),
    }
}

fn option_color(options: &serde_json::Value) -> Result<wgpu::Color, JsValue> {
    let Some(value) = options.get("clear") else {
        return Ok(wgpu::Color {
            r: 0.06,
            g: 0.07,
            b: 0.09,
            a: 1.0,
        });
    };
    let list = value
        .as_array()
        .filter(|list| list.len() == 4)
        .ok_or_else(|| {
            host_error(
                "options",
                format!("clear 必须是 4 个数的数组，给的是 {value}"),
            )
        })?;
    let mut channels = [0.0_f64; 4];
    for (index, item) in list.iter().enumerate() {
        channels[index] = item
            .as_f64()
            .ok_or_else(|| host_error("options", format!("clear[{index}] 不是数：{item}")))?;
    }
    Ok(wgpu::Color {
        r: channels[0],
        g: channels[1],
        b: channels[2],
        a: channels[3],
    })
}
