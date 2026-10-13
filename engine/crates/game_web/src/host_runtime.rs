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

use crate::rust_scene::SimRoute;

/// Everything one engine host owns. Single-threaded by construction (wasm), so it lives in a
/// thread-local instead of being handed back to JS as an opaque pointer.
struct EngineHost {
    /// GPU 侧二选一：**clear-color 栈**（P0-①/接管形态，既有现场）或**共享核心**（`scene` 档，
    /// 真场景形态）。一张画布只会有一个 surface——两条分支绝不同时存在。
    gpu: HostGpu,
    clock: FrameClock,
    /// 固定步频率。**步长按 f64 现算**（`1.0 / ups`）而不是用 `FrameClock` 的 f32 步长：
    /// JS 侧喂给 Lua 的是 `1/60` 的 double，f32 取整会给出 0.016666668，两者不是同一个数。
    updates_per_second: u32,
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
    /// （`config` 不存：configure 之后没人读它，尺寸读数在 `EngineHost.width/height`）。
    Clear {
        /// 保活：surface/adapter 的生命周期挂在它上面。
        _instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
        device: wgpu::Device,
        queue: wgpu::Queue,
    },
    /// 真场景形态（P1 共享核心）：与浏览器宿主同一条渲染链
    /// （`Renderer` + `RustScene` step/mesh/贴图/取景/render/present）。
    Scene(crate::engine_core::EngineCore),
}

thread_local! {
    static HOST: RefCell<Option<EngineHost>> = const { RefCell::new(None) };
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
/// 场景相对路径则切进共享核心真场景形态）与 `sim`（路线名，缺省 `script`，只在给了 `scene` 时有意义）。
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
    let (gpu, adapter_display, backend_display, format_display, mode_display, fit_note) =
        match scene_spec {
            Some((scene_path, route)) => {
                let report_for_core = report.clone();
                let core = crate::engine_core::EngineCore::build(crate::engine_core::CoreArgs {
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
                })
                .await
                .map_err(|error| host_error("engine-core", error))?;
                // **逻辑视口 = 设计分辨率**（与浏览器宿主 `GpuHost::fit_now` 同一说法，任务 #20）：
                // 表面/后备缓冲是设备像素（接管形态下必须整屏），投影按设计盒算，渲染器再把设计盒
                // **等比居中**摆进表面（`engine::host::fit::contain_rect`）。此前这里把表面尺寸直接
                // 当逻辑视口 ⇒ 1 世界单位 = 1 设备像素，画面停在表面一角（run10 实测内容块
                // `1380×720` / 表面 `2532×1170`，非底色像素只占 31.1%）。
                let (core, fit_note) = fit_scene_viewport(core, width, height);
                let (adapter_display, backend_display, format_display) = {
                    let (adapter, backend, format) = core.stack_info();
                    (adapter.to_owned(), backend.to_owned(), format)
                };
                (
                    HostGpu::Scene(core),
                    adapter_display,
                    backend_display,
                    format_display,
                    format!("scene({scene_path})"),
                    fit_note,
                )
            }
            None => {
                let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                    // wasm 上必须显式 GL：默认在 wasm 只有 webgpu（与 `gpu_host` / 探针同一口径）。
                    backends: wgpu::Backends::GL,
                    ..Default::default()
                });
                let surface = engine::host::web_surface::canvas_surface(&instance, &gpu_canvas)
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

    let clock = FrameClock::new(updates_per_second, max_catch_up_steps, max_frame_seconds)
        .map_err(|error| host_error("frame-clock", error))?;
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
        *slot.borrow_mut() = Some(EngineHost {
            gpu,
            clock,
            updates_per_second,
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
/// 等比居中映射进附件（`engine::host::fit::contain_rect`）。浏览器外壳靠 `fit_now`
/// 把后备缓冲缩到等比适配盒（表面宽高比 ≡ 设计比 ⇒ 矩形贴满、无偏移）；微信接管形态
/// 的表面必须是整屏设备像素（缩放由平台把画布拉到屏幕，改后备尺寸就是非等比拉伸），
/// 因此留边必须发生在**表面内部**——这一步就是那条路的入口。
///
/// 读数进 `engine_host_state.fit` 与 begin summary：几何错了在 `presented=`/`frames=`
/// 里完全看不见（#19 的 31.1% 覆盖率就是这么发现的），所以矩形本身必须是可读数的。
fn fit_scene_viewport(
    mut core: crate::engine_core::EngineCore,
    surface_width: u32,
    surface_height: u32,
) -> (crate::engine_core::EngineCore, String) {
    let note = match core.sim_mut() {
        Some(sim) => {
            sim.set_viewport(crate::DESIGN_WIDTH, crate::DESIGN_HEIGHT);
            let (x, y, width, height) = engine::host::fit::contain_rect(
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
    };
    (core, note)
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
        let plan = host.clock.plan(dt_ms / 1000.0);
        // 固定步：只有 `tick: "lua"` 时由**本宿主**推进（老 JS 世界每帧也在跑，两边同时跑
        // 就是把世界推进两次——所以默认 `none`）。走的是 `lua_bridge::lua_fixed_run_steps`，
        // 与 JS 固定步阶段同一段顺序（虚拟键 → `__rustlua_dt` → `update()`）。
        if host.tick_lua && plan.steps > 0 {
            // 步长按 f64 现算：JS 喂给 Lua 的是 `1/60` 的 double，而 `FrameClock` 的步长是 f32
            // （0.016666668），两者不是同一个数。
            let delta = 1.0 / f64::from(host.updates_per_second);
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
        match &mut host.gpu {
            HostGpu::Clear {
                surface,
                device,
                queue,
                ..
            } => {
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
            }
            HostGpu::Scene(core) => {
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
                match core.frame(plan.steps, plan.delta_seconds, &input) {
                    Ok(out) => {
                        // 读数只进 `engine_host_state` 的 `scene` 块：**定长 ABI 的 16 槽不许动**
                        // （版本没变就是契约没变，这是那套槽位存在的全部理由）。
                        if let Some(reason) = core.pending_step_failure.take() {
                            host.last_scene = format!("scene_step_failed:{reason}");
                        } else if out.builtin {
                            host.last_scene = format!("builtin {}", core.sim_note);
                        } else {
                            // `out.scene_note` 是场景自报（名字/实体数/来源）——**必须进读数**：
                            // 引擎 loader 失败时会静默换内置诊断场景，只报 `rust(boot ok)`
                            // 会把"起了个假场景"说成"起了真场景"。
                            host.last_scene = format!(
                                "{} {} draws={} tris={} scene_delta={:+},{:+},{:+}",
                                core.sim_note,
                                out.scene_note,
                                out.draw_calls,
                                out.triangles,
                                out.scene_delta[0],
                                out.scene_delta[1],
                                out.scene_delta[2]
                            );
                        }
                    }
                    Err(error) => {
                        if error.starts_with("get_current_texture") {
                            host.failed_presents += 1;
                        }
                        return Err(host_error("scene-frame", error));
                    }
                }
            }
        }
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
            host.clock.total_steps() as f64,
            f64::from(plan.delta_seconds) * 1000.0,
            plan.elapsed_seconds * 1000.0,
            plan.accumulator_seconds * 1000.0,
            plan.dropped_steps as f64,
            host.clock.total_dropped_steps() as f64,
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
    HOST.with(|slot| {
        let borrow = slot.borrow();
        let host = borrow.as_ref().ok_or_else(|| {
            host_error("not-begun", "先调 engine_host_begin（画布与设备由它建立）")
        })?;
        // scene 档的读数**只进 JSON 状态**（定长 ABI 的 16 槽不动）：`last` 是每帧拼好的
        // 场景读数，`sim`/`textures` 是核心自己的账（与浏览器状态属性同一批字段名同源）。
        let (mode, scene_block) = match &host.gpu {
            HostGpu::Clear { .. } => ("clear", serde_json::Value::Null),
            HostGpu::Scene(core) => (
                "scene",
                serde_json::json!({
                    "sim": core.sim_note,
                    "textures": format!(
                        "{}/{}",
                        core.texture_stats.0,
                        core.texture_stats.0 + core.texture_stats.1
                    ),
                    "texture_note": core.texture_note,
                    "boot_ms": core.boot_ms,
                    "first_frame_ms": core.first_frame_ms.is_finite().then_some(core.first_frame_ms),
                    "last": host.last_scene,
                }),
            ),
        };
        let state = serde_json::json!({
            "abi": HOST_FRAME_ABI,
            "version": HOST_FRAME_ABI_VERSION,
            "frame": host.clock.frame_index(),
            "steps_total": host.clock.total_steps(),
            "dropped_total": host.clock.total_dropped_steps(),
            "accumulated_ms": host.clock.accumulator_seconds() * 1000.0,
            "clamped_frames": host.clock.clamped_frames(),
            "presented_total": host.presented_frames,
            "failed_presents": host.failed_presents,
            "input_frames": host.input_frames,
            "input_bytes": host.input_bytes,
            "input_applied": host.input_applied,
            "tick": if host.tick_lua { "lua" } else { "none" },
            "tick_steps_total": host.tick_steps_total,
            "tick_failures": host.tick_failures,
            "updates_per_second": host.updates_per_second,
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
        (None, Some(name)) => Err(host_error(
            "options",
            format!("给了 sim={name} 却没给 scene：clear-color 形态没有模拟可走"),
        )),
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
