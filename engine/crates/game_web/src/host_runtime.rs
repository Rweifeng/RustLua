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
//! Begin/end, the fixed-step clock, per-frame clear + present, and a JSON state readout. The
//! simulation tick itself is still driven by the existing Lua bridge (`lua_fixed_frame`); wiring
//! the engine's own scene host into this loop is the next slice, and the input batch (`RLFI`) is
//! carried through untouched but not yet decoded here (counted, so the pipe is visible).
//!
//! The GPU half only exists in the `gpu2d` build (`package.ps1 -Gpu2d`): the default kernel has no
//! `wgpu` at all, and therefore no `engine_host_*` exports.

use std::cell::RefCell;

use rustlua_infra::frame_clock::{FRAME_INTERVAL_TOLERANCE_SECONDS, FrameClock};
use wasm_bindgen::prelude::*;

/// Everything one engine host owns. Single-threaded by construction (wasm), so it lives in a
/// thread-local instead of being handed back to JS as an opaque pointer.
struct EngineHost {
    /// Kept alive so the surface/adapter remain valid for the whole host lifetime.
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
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
    clear: wgpu::Color,
    canvas_note: String,
    presented_frames: u64,
    failed_presents: u64,
    input_frames: u64,
    input_bytes: u64,
    /// 真正交给 `lua_input_frame` 解码成功的批次数（与 `input_frames` 分开：后者含空帧）。
    input_applied: u64,
    last_input_note: String,
    last_note: String,
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
/// `clear` (`[0.06, 0.07, 0.09, 1]`), `canvas_width` / `canvas_height` (read from the canvas).
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
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
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

    let clock = FrameClock::new(updates_per_second, max_catch_up_steps, max_frame_seconds)
        .map_err(|error| host_error("frame-clock", error))?;
    let canvas_note = format!("{width}x{height}");
    let summary = serde_json::json!({
        "ok": true,
        "canvas": canvas_note,
        "format": format!("{format:?}"),
        "adapter": info.name,
        "backend": format!("{:?}", info.backend),
        "updates_per_second": updates_per_second,
        "max_catch_up_steps": max_catch_up_steps,
        "max_frame_seconds": max_frame_seconds,
        "tolerance_seconds": FRAME_INTERVAL_TOLERANCE_SECONDS,
        "tick": if tick_lua { "lua" } else { "none" },
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
            "host: 已就绪 canvas={canvas_note} fmt={format:?} adapter={} backend={:?} tick={}",
            info.name,
            info.backend,
            if tick_lua { "lua" } else { "none" }
        ),
    );
    HOST.with(|slot| {
        *slot.borrow_mut() = Some(EngineHost {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            clock,
            updates_per_second,
            tick_lua,
            tick_steps_total: 0,
            tick_failures: 0,
            clear,
            canvas_note,
            presented_frames: 0,
            failed_presents: 0,
            input_frames: 0,
            input_bytes: 0,
            input_applied: 0,
            last_input_note: String::new(),
            last_note: String::new(),
        });
    });
    Ok(summary.to_string())
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
        let frame = match host.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(error) => {
                host.failed_presents += 1;
                return Err(host_error("present", error));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = host
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
        host.queue.submit(Some(encoder.finish()));
        frame.present();
        // WebGL2 没有阻塞能力：显式 poll 一次，让这一帧的校验错误在这里暴露，而不是变成下一帧的谜题。
        let _ = host.device.poll(wgpu::PollType::Poll);
        host.presented_frames += 1;
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
            "input_note": host.last_input_note,
            "note": host.last_note,
            "size": format!("{}x{}", host.config.width, host.config.height),
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
