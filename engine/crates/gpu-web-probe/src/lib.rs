//! **技术验证 spike，不是产品代码**：在真实浏览器里问 wgpu 27 的 WebGL2 后端几个问题。
//!
//! 为什么需要它：本仓的 `engine` 渲染器在原生目标上按 WebGPU 的规格写（`Limits::default()`、
//! 只读存储缓冲、`InstanceDescriptor::default()`），而 wasm 目标要靠 `webgl` 特性落到 WebGL2
//! 后端（微信小游戏只有 WebGL2，没有 WebGPU）。WebGL2 的规格比 WebGPU 弱一大截，哪些调用会
//! 直接失败、哪些只是数值变小，只有在真浏览器里跑一遍才算数——这些结论会决定 `engine` 的
//! wasm 分支要不要降级限制、要不要把存储缓冲路径改掉，所以先花一次 spike 把事实钉住。
//!
//! 为什么要两种 `backends` 配置：`Backends::GL` 是"只给 WebGL2"的最小假设，`Backends::all()`
//! 是"浏览器今天真正会给什么"。两者结果不同就说明**宿主环境的差异会改变引擎行为**（例如有
//! WebGPU 的浏览器会绕开 WebGL2 的限制），这决定了引擎能不能写死一套限制值。
//!
//! 三条设计约束（都是被本仓的 wasm 环境逼出来的）：
//! 1. **任何内部错误都必须变成 JSON 里的 `error` 字段**，绝不能让 Promise reject：
//!    wasm 目标是 `panic = "abort"`，一旦 reject/panic 到 JS 层，调用侧拿不到任何上下文，
//!    spike 就白跑了。所以每一层都显式收集错误文本。
//! 2. **校验错误用 error scope 捕获**：wgpu 默认把未被 error scope 捕获的错误升级成 panic，
//!    而 wasm 上 panic 不可捕获（abort）。探测"存储缓冲会不会报错"这件事本身就是在制造
//!    校验错误，所以必须先 push_error_scope 再动手。
//! 3. **不挂进 DOM**：`document.create_element("canvas")` 造出的游离 canvas 足够让 wgpu 建
//!    surface（WebGL2 上下文不需要 canvas 在文档里），这样探针可以随时调用，不污染页面。

// 目标位闸门：本 crate 通篇是"浏览器里的一段异步问答"，宿主目标上三个面**同时**不存在
// （实测 `cargo clippy -p gpu-web-probe --all-targets`，宿主 Windows）：
//   E0433 `engine::host::web_surface` —— 该模块在 engine 侧就是 `#[cfg(target_arch = "wasm32")]`；
//   E0277 ×2 `map_async` 回调要 `WasmNotSend`，非 wasm 目标上退化成 `Send`，而回调必须捕获 `Rc<RefCell<…>>`。
// 所以这不是"少写一个 feature"能补的，是设计上的 wasm-only。宿主侧继续编它只会把 CI 的
// Clippy 步（`cargo clippy --workspace --all-targets --all-features`）永久钉红，
// 而红门没人当真 = 门禁名存实亡。
// 代价说清楚：**宿主目标上本 crate 不再被检查**，它的编译门只发生在
// `cargo build -p gpu-web-probe --target wasm32-unknown-unknown`（`scripts/gpu-web-probe.ps1`
// 的构建路径就是它）。闸门不是把探针变空：同一条 wasm 构建里产物仍是 324,920,099 字节的
// debug wasm，`gpu_probe` / `gpu_render_probe` / `storage_buffer` 这些符号都还在导出段里。
// 仍留在 workspace members 里是有意的：`cargo tree -p game_web` 那条"发布包不含本探针"的门
// 要靠 members 才查得到。
#![cfg(target_arch = "wasm32")]

use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

/// 探针用 canvas / 离屏贴图的边长：64×64 是 WebGL2 上最小且能满足 copy 行对齐（256 字节）的方便尺寸。
const CANVAS_SIZE: u32 = 64;

/// `copy_texture_to_buffer` 的行距：`bytes_per_row` 必须是 256 的整数倍。
/// 64 像素 × 4 字节 = 256，正好等于该对齐值，所以本探针不需要额外补行。
const ROW_STRIDE: u32 = CANVAS_SIZE * 4;

/// 探针入口。resolve 出**一个 JSON 字符串**（字段清单见仓库报告），任何失败都在 JSON 里，不会 reject。
#[wasm_bindgen]
pub fn gpu_probe() -> js_sys::Promise {
    wasm_bindgen_futures::future_to_promise(async move {
        // 整个探针的兜底：连 window/document 都拿不到时也要给出 JSON，而不是 throw。
        let value = match run_probe().await {
            Ok(value) => value,
            Err(message) => json!({ "fatal": message }),
        };
        Ok(JsValue::from_str(&value.to_string()))
    })
}

/// 把环境事实和两种后端配置的结果拼成一个对象。
async fn run_probe() -> Result<Value, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let navigator = window.navigator();
    // `navigator.gpu` 存在只说明"浏览器认识 WebGPU"，不代表一定能拿到适配器；
    // 这里只作为环境标注，真正的证据是下面各配置里请求到的 backend。
    let webgpu_available =
        js_sys::Reflect::has(&navigator, &JsValue::from_str("gpu")).unwrap_or(false);

    let mut results = serde_json::Map::new();
    // gl_only 放前面：它最可能失败（浏览器根本没有 WebGL2 时），先失败也先留下证据。
    for config in ["gl_only", "auto"] {
        let backends = if config == "gl_only" {
            wgpu::Backends::GL
        } else {
            wgpu::Backends::all()
        };
        let mut slot = Value::Null;
        // 单种配置内部出错不拖垮整份报告：另一种配置的结论仍然要交回去。
        if let Err(error) = probe_config(backends, &mut slot).await {
            slot = json!({ "error": error });
        }
        results.insert(config.to_owned(), slot);
    }

    Ok(json!({
        // `user_agent()` 在 web-sys 里返回 `Result<String, JsValue>`；取不到时给空串，
        // 而不是让整份探针结果因为一个纯标注字段失败。
        "user_agent": navigator.user_agent().unwrap_or_default(),
        "webgpu_available": webgpu_available,
        // 请求侧的两个 limits 集合不需要设备就能取到，先原样报出来，好和适配器实际支持值逐项对比。
        "requested_limits": {
            "limits_default": describe_limits(&wgpu::Limits::default()),
            "downlevel_webgl2_defaults": describe_limits(&wgpu::Limits::downlevel_webgl2_defaults()),
        },
        "configs": Value::Object(results),
    }))
}

/// 跑一种 `backends` 配置的全流程。任何一步失败都**不提前返回**，而是把 `null` 留给后续步骤，
/// 因为"第一个失败点"和"后面还会不会失败"是两条不同的情报。
async fn probe_config(backends: wgpu::Backends, out: &mut Value) -> Result<(), String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let document = window.document().ok_or_else(|| "no document".to_string())?;

    // 游离 canvas：只为拿一个 WebGL2 上下文，不进 DOM。
    let canvas = document
        .create_element("canvas")
        .map_err(|error| format!("create_element(canvas) failed: {error:?}"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "create_element(canvas) did not produce HtmlCanvasElement".to_owned())?;
    canvas.set_width(CANVAS_SIZE);
    canvas.set_height(CANVAS_SIZE);

    // 只覆盖 `backends`：`InstanceDescriptor::default()` 在 wasm 上会把后端集合设成
    // "编译进来的全部"，这里换成显式集合正是本探针要对比的变量，其余字段保持默认。
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends,
        ..Default::default()
    });

    let surface = match create_canvas_surface(&instance, &canvas) {
        Ok(surface) => surface,
        Err(error) => {
            *out = json!({ "phase": "create_surface", "error": error });
            return Ok(());
        }
    };

    // WebGL2 下 `compatible_surface` 是**必须**的：wgpu 文档明确说默认 options 在 WebGL2 上
    // 不会成功，所以这里始终带上 surface。
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(adapter) => adapter,
        Err(error) => {
            *out = json!({
                "phase": "request_adapter",
                "create_surface_ok": true,
                "error": error.to_string(),
            });
            return Ok(());
        }
    };

    let info = adapter.get_info();
    let downlevel = adapter.get_downlevel_capabilities();
    let mut report = json!({
        "phase": "done",
        "create_surface_ok": true,
        "request_adapter_ok": true,
        "adapter_info": {
            "backend": format!("{:?}", info.backend),
            "name": info.name,
            "driver": info.driver,
            "device_type": format!("{:?}", info.device_type),
        },
        // features 用 Debug 打出名字（`Features` 的 Debug 是位标志名列表）；再补一个
        // `Display`（`A | B` 形式）与 bits 数组，方便和 engine 里
        // `adapter.features().contains(TIMESTAMP_QUERY)` 这类判断对齐。
        "adapter_features": format!("{:?}", adapter.features()),
        "adapter_features_display": format!("{}", adapter.features()),
        "adapter_features_bits": format!("{:?}", adapter.features().bits()),
        "downlevel_flags": format!("{:?}", downlevel.flags),
        "downlevel_shader_model": format!("{:?}", downlevel.shader_model),
        "adapter_limits": describe_limits(&adapter.limits()),
        // 两次 request_device 的结果是本探针最关键的一句话：WebGL2 上 `Limits::default()`
        // 应该请求失败（它要求 max_storage_buffers_per_shader_stage=8 等 WebGL2 没有的东西），
        // 只有 `downlevel_webgl2_defaults()` 能成功。失败文本必须留下。
        "request_device_default_limits": Value::Null,
        "request_device_downlevel_limits": Value::Null,
        "device_limits_after_request": Value::Null,
        "render_readback": Value::Null,
        "storage_buffer_bind_group_layout": Value::Null,
    });

    // 第一次：`Limits::default()` —— 复刻 engine 现在的写法，目的就是看它会不会失败。
    match adapter
        .request_device(&device_descriptor(
            "gpu-web-probe-default-limits",
            wgpu::Limits::default(),
        ))
        .await
    {
        Ok((_device, _queue)) => {
            report["request_device_default_limits"] = json!({ "ok": true });
        }
        Err(error) => {
            report["request_device_default_limits"] =
                json!({ "ok": false, "error": error.to_string() });
        }
    }

    // 第二次：`downlevel_webgl2_defaults()` —— 拿到设备后顺便做真渲染 + 读回，作为
    // "WebGL2 后端真的能画"的硬证据。error scope 也挂在这一次的设备上。
    let device_and_queue = adapter
        .request_device(&device_descriptor(
            "gpu-web-probe-downlevel-limits",
            wgpu::Limits::downlevel_webgl2_defaults(),
        ))
        .await;
    let (device, queue) = match device_and_queue {
        Ok(pair) => {
            report["request_device_downlevel_limits"] = json!({ "ok": true });
            pair
        }
        Err(error) => {
            report["request_device_downlevel_limits"] =
                json!({ "ok": false, "error": error.to_string() });
            *out = report;
            return Ok(());
        }
    };

    report["device_limits_after_request"] = describe_limits(&device.limits());
    report["render_readback"] = match render_and_read_back(&device, &queue).await {
        Ok(value) => value,
        Err(message) => json!({ "error": message }),
    };
    report["storage_buffer_bind_group_layout"] = storage_buffer_probe(&device).await;

    *out = report;
    Ok(())
}

/// 从游离 canvas 建 surface。
///
/// **实现已收编到引擎**（`engine::host::web_surface::canvas_surface`）：web 宿主 `game_web`
/// 与探针都要走这一步，两份带 `unsafe` 的副本就是两个"只修好一份"的机会。
/// 这里保留一个薄封装，只为把 `HtmlCanvasElement` 转成引擎那层要的 `JsValue`——
/// 引擎不该认识 DOM 类型，宿主本来就依赖 `web-sys`。
fn create_canvas_surface(
    instance: &wgpu::Instance,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<wgpu::Surface<'static>, String> {
    engine::host::web_surface::canvas_surface(instance, &JsValue::from(canvas.clone()))
}

/// 与 `engine/crates/engine/src/host/renderer/construct.rs` 里 `request_device` 的字段集逐字段
/// 对齐（这是本仓 wgpu 27 的"正确写法"基准），只把 `required_limits` 参数化。
fn device_descriptor(label: &str, required_limits: wgpu::Limits) -> wgpu::DeviceDescriptor<'_> {
    wgpu::DeviceDescriptor {
        label: Some(label),
        // 探针故意不要任何可选特性：WebGL2 上 TIMESTAMP_QUERY 之类本来就没有，
        // 要求它会污染"限制值够不够"这个结论。
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits,
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }
}

/// 只挑与 engine 渲染器直接相关的限制字段。全量 `Limits` 有几十个字段，全打出来反而看不出重点；
/// 这九个字段对应"能不能建存储缓冲/存储贴图、绑定组与顶点布局够不够大、能不能画满一张大贴图"。
fn describe_limits(limits: &wgpu::Limits) -> Value {
    json!({
        "max_storage_buffers_per_shader_stage": limits.max_storage_buffers_per_shader_stage,
        "max_storage_textures_per_shader_stage": limits.max_storage_textures_per_shader_stage,
        "max_compute_workgroup_storage_size": limits.max_compute_workgroup_storage_size,
        "max_texture_dimension_2d": limits.max_texture_dimension_2d,
        "max_bind_groups": limits.max_bind_groups,
        "max_vertex_buffers": limits.max_vertex_buffers,
        "max_vertex_attributes": limits.max_vertex_attributes,
        "max_color_attachments": limits.max_color_attachments,
        "max_uniform_buffer_binding_size": limits.max_uniform_buffer_binding_size,
    })
}

/// 真渲染 + 读回：清成绿色 → copy 到可映射缓冲 → map → 读第一个像素。
/// 这是"WebGL2 后端不只是能建对象，而是真能执行 pass 并把像素运回 CPU"的证据链。
async fn render_and_read_back(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Value, String> {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-web-probe-target"),
        size: wgpu::Extent3d {
            width: CANVAS_SIZE,
            height: CANVAS_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // MAP_READ 是这里的必要条件：WebGL2 的读回走缓冲映射，没有别的通路。
    // 256 字节行距（64×4）正好满足 COPY_BYTES_PER_ROW_ALIGNMENT，所以行数是 64 行整。
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu-web-probe-readback"),
        size: (ROW_STRIDE * CANVAS_SIZE) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gpu-web-probe-encoder"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpu-web-probe-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                // 离屏目标不是 mip/array 视图，所以不需要 depth_slice（wgpu 27 新增字段）。
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        // 故意不画几何：只验证 clear + copy + map 这条最底层的通路，
        // 把"管线/着色器能不能编译"留给别的 spike，免得一个失败掩盖另一个。
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(ROW_STRIDE),
                rows_per_image: Some(CANVAS_SIZE),
            },
        },
        wgpu::Extent3d {
            width: CANVAS_SIZE,
            height: CANVAS_SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let pixels = map_buffer(device, &readback).await?;
    if pixels.len() < 4 {
        return Err(format!("readback returned only {} bytes", pixels.len()));
    }
    Ok(json!({
        "clear_color": "GREEN",
        "format": "Rgba8UnormSrgb",
        "bytes_per_row": ROW_STRIDE,
        "first_pixel_rgba": [pixels[0], pixels[1], pixels[2], pixels[3]],
        "readback_bytes": pixels.len(),
    }))
}

/// 等待整段缓冲映射完成，并把字节拷出来。
///
/// wgpu 27 的 `BufferSlice::map_async` 是**回调式**的（`FnOnce(Result<(), BufferAsyncError>)`），
/// 没有返回 Future；所以这里用一个"回调写、Future 读"的共享槽手动把它变回可 await 的东西。
/// 为什么需要让出事件循环：wasm 上映射完成由浏览器的 promise 回调驱动，纯忙等（只调
/// `device.poll`）不会推进它——`device.poll` 在 wasm 上是自动轮询的空操作。
async fn map_buffer(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    let state = Rc::new(RefCell::new(None::<Result<(), String>>));
    {
        let state = Rc::clone(&state);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                *state.borrow_mut() = Some(result.map_err(|error| error.to_string()));
            });
    }

    // 上限只是防死循环：64×64 的读回在真机上通常几十毫秒内完成。
    let mut outcome = None;
    for _ in 0..2_000 {
        if let Some(result) = state.borrow_mut().take() {
            outcome = Some(result);
            break;
        }
        // 空转一次设备（wasm 上是空操作），再把控制权交回浏览器事件循环。
        let _ = device.poll(wgpu::PollType::Poll);
        yield_to_event_loop().await;
    }

    match outcome {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(format!("map_async failed: {error}")),
        None => return Err("map_async timed out waiting for the map callback".to_owned()),
    }

    // 先把 view 的作用域收掉（`unmap` 要求没有存活的映射视图），再 unmap。
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    // 映射未解除时缓冲不能被回收/再次映射，所以一定要 unmap（wgpu 27 的 `unmap` 在 `Buffer` 上，
    // 不在 `BufferSlice` 上）。
    buffer.unmap();
    Ok(bytes)
}

/// 让出一次浏览器事件循环。
///
/// 实现方式选 `setTimeout(0)` 而不是 `Promise.resolve().then()`：microtask（promise 链）
/// 只是把控制权还给当前 JS 栈之后的同步续体，**不产生新的宏任务**，而 WebGPU 的映射回调由
/// 浏览器在主循环里派发；用宏任务才能保证它一定有机会跑。
async fn yield_to_event_loop() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        // 拿不到 setTimeout 时退化为立即 resolve（至少不会永久卡住）。
        match web_sys::window() {
            Some(window) => {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
            }
            None => {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            }
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// error scope 探针：唯一 entry 是只读存储缓冲的 `BindGroupLayout`。
/// WebGL2 上 `max_storage_buffers_per_shader_stage = 0`，所以**预期**这里会收到一条校验错误；
/// 本函数把"有没有错、错是什么"当结论，而不是当异常。
async fn storage_buffer_probe(device: &wgpu::Device) -> Value {
    // 必须先 push：wgpu 对未被 error scope 捕获的校验错误会走默认 handler（panic），
    // 而本 crate 的 wasm 是 panic=abort，一旦发生整个模块就死了。
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gpu-web-probe-storage-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            // 与 engine 的 `vector_bind_group_layout` 同构：只读存储缓冲。
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let error = device.pop_error_scope().await;
    // 让 layout 活到 pop 之后，确保错误归属这一次创建（也避免被优化成"没有产生校验错误"）。
    drop(layout);

    match error {
        Some(error) => json!({ "validation_error": true, "error": error.to_string() }),
        None => json!({ "validation_error": false }),
    }
}

// ============ M73-00 WP5 第一片：让**真正的** engine 渲染器在 WebGL2 上出一帧 ============
//
// 上面那套探针回答的是"wgpu 的 GL 后端能不能用"；这一套回答的是 WP5 真正的问题：
// **本仓那台按 WebGPU 规格写的渲染器，原样接到 WebGL2 上能不能把一帧画对**。
// 两者的差别是本质的：能力探针挂了不代表渲染器挂了——渲染器的坑在别处（几十条管线的
// WGSL→GLSL 翻译、绑定组布局、深度附件格式、顶点步长、帧计时用的时钟、构造期的
// `pollster::block_on`）。所以这一片**不做玩具渲染器**，直接把 `engine::host::Renderer`
// 在浏览器里跑起来，用像素和它自己的帧计数说话。
//
// 场景刻意手工合成、且**只引用内建白贴图**：`RuntimeScene` 那条路要读工程文件，而 wasm 上
// 没有文件系统。把变量减到最少，"画对了"才归因得到渲染器本身。

/// 离屏目标边长。256×256：行距 `256 × 4 = 1024` 正好是 `bytes_per_row` 要求的 256 倍数，
/// 读回不需要补行（与上面 64×64 同一个理由）。
const RENDER_SIZE: u32 = 256;

/// 两个四边形在 NDC 里的范围 `[x0, x1, y0, y1]`：左下与右上，中间留出背景。
/// 这样"画到了"和"没画到"各有一组确定的采样点，两块几何的像素面积还能解析算出来对账。
const QUADS: [[f32; 4]; 2] = [[-0.8, -0.4, -0.8, -0.4], [0.4, 0.8, 0.4, 0.8]];

/// 贴图四边形：右下角，**既不压住背景采样点 (0,0)，也不与白四边形重叠**——
/// 否则"白色像素面积"这条判据会被贴图四边形的高亮纹素搅乱。
const TEXTURED_QUAD: [f32; 4] = [0.4, 0.8, -0.8, -0.4];

/// 上传用键：**故意不是文件路径**。web 上没有文件系统，按路径解析必然落白兜底；
/// 采样点若等于原纹素，只可能是"内存上传真的进了缓存并被采样到了"。
const CHECKER_KEY: &str = "probe://checker2x2";

/// 2×2 棋盘（行主序），四角四色且**没有白**——"上传失败退回白贴图"因此会在
/// 两处同时露馅：四个采样点全白、且白色像素面积超出解析值。
const CHECKER: [[u8; 4]; 4] = [
    [255, 0, 0, 255],   // texel (0,0)：红
    [0, 255, 0, 255],   // texel (1,0)：绿
    [0, 0, 255, 255],   // texel (0,1)：蓝
    [255, 255, 0, 255], // texel (1,1)：黄
];

/// 纹素中心采样：uv 取 0.25/0.75 时双线性过滤返回的**就是**那个纹素，
/// 所以期望值是原字节，不给容忍度（sRGB 的 0/255 端点在往返上是精确的）。
const TEXEL_SAMPLES: [(f32, f32, [u8; 4]); 4] = [
    (0.25, 0.25, [255, 0, 0, 255]),
    (0.75, 0.25, [0, 255, 0, 255]),
    (0.25, 0.75, [0, 0, 255, 255]),
    (0.75, 0.75, [255, 255, 0, 255]),
];

/// 清屏色（**线性**空间，`ClearPolicy::Color` 的约定）。`0.0` 在 `Rgba8UnormSrgb` 上正好写
/// 字节 0、顶点色 `1.0` 正好写 255——所以下面的断言是**精确值**比较，不给容忍度：
/// WebGL2 上"差一点点"通常意味着走了另一条路径（例如采样/混合状态不是预期那条）。
const CLEAR_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// WP5 第一片入口：离屏渲染一帧真场景，把像素与渲染器自报的帧计数一起交回去。
/// 与 `gpu_probe` 同一约定：**任何失败都进 JSON，不 reject**（wasm 是 panic=abort）。
#[wasm_bindgen]
pub fn gpu_render_probe() -> js_sys::Promise {
    wasm_bindgen_futures::future_to_promise(async move {
        let value = match run_render_probe().await {
            Ok(value) => value,
            Err(message) => json!({ "fatal": message }),
        };
        Ok(JsValue::from_str(&value.to_string()))
    })
}

/// 进度标记：把"最后到达的阶段"写进 `data-render-probe`（覆盖式，与页面最终的 `done`/`failed`
/// 同一个属性——CDP dump 里带的就是最后一次写入）。
///
/// 为什么需要它：探针**卡住**时（例如 SwiftShader 编十几条管线非常慢，或某个 await 永远不来），
/// 页面状态会一直停在初始值，从外面完全看不出卡在哪一步。有了阶段标记，dump 里最后那个值
/// 就是定位。它不影响 CDP 判落定：驱动只认 `done`/`failed`。
fn set_stage(stage: &str) {
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        if let Some(body) = document.body() {
            let _ = body.set_attribute("data-render-probe", stage);
        }
    }
}

async fn run_render_probe() -> Result<Value, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let document = window.document().ok_or_else(|| "no document".to_string())?;

    // 游离 canvas：**只为适配器协商存在**。wgpu 在 WebGL2 上要求 `compatible_surface`
    // （见 `probe_config` 的注释），而离屏渲染本身不需要画布。surface 拿到 adapter 之后
    // 仍要留着：GL 上下文挂在它上面，设备与它同生共死。
    let canvas = document
        .create_element("canvas")
        .map_err(|error| format!("create_element(canvas) failed: {error:?}"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| "create_element(canvas) did not produce HtmlCanvasElement".to_owned())?;
    canvas.set_width(CANVAS_SIZE);
    canvas.set_height(CANVAS_SIZE);

    // WP2 硬约束 1：wasm 上必须显式 `Backends::GL`（`default()`/`all()` 连适配器都拿不到）。
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    let surface = create_canvas_surface(&instance, &canvas)?;
    set_stage("surface_ok");
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|error| format!("request_adapter: {error}"))?;
    let info = adapter.get_info();
    set_stage("adapter_ok");

    // WP2 硬约束 2：起点用"WebGL2 一定接受"的地板（storage/compute 归零），再把 1D/2D/3D
    // 贴图上限抬到适配器实际值。直接写 `downlevel_webgl2_defaults()` 会把 2D 上限从适配器的
    // 8,192 压到 2,048，而字体图集（`font.rs`）是按长到 4,096 设计的——那是白白降级。
    let adapter_limits = adapter.limits();
    // `Limits` 不是 `Copy`（几十个 u32 的大结构体，`using_resolution`/`DeviceDescriptor` 都按值取），
    // 所以这里 clone 一份给请求、原件留着报告——报告要证明的正是"请求值 vs 设备拿到值"的差。
    let requested =
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits.clone());
    let (device, queue) = adapter
        .request_device(&device_descriptor(
            "gpu-render-probe-device",
            requested.clone(),
        ))
        .await
        .map_err(|error| format!("request_device: {error}"))?;
    set_stage("device_ok");

    // 真渲染器。走**异步**入口：wasm 上没有阻塞能力，`new_offscreen` 内部的
    // `pollster::block_on` 在 `thread::park_timeout` 未实现的平台上不该被使用。
    // `project_root = None` + 网格只引用内建白贴图 ⇒ 整条构造与渲染路径不碰文件系统。
    //
    // 构造期会**故意**踩到 WebGL2 的能力边界：矢量文字与 3D 蒙皮那两条路用了存储缓冲，
    // 而 WebGL2 的 `max_storage_buffers_per_shader_stage = 0`。wgpu 对**未被 error scope 捕获**
    // 的校验错误会走默认 handler → panic，而本 crate 的 wasm 是 `panic=abort`：整个模块会死，
    // 探针连"哪一条被拒"都报不出来。所以这里必须先 push_error_scope，让失败变成**数据**。
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    // 阶段标记放在构造**之前**：构造要建十几条管线（naga 翻 WGSL→GLSL + GL 链接），
    // 在软件光栅上可能非常慢——卡住时这个值就是答案。
    set_stage("constructing");
    let created = engine::host::Renderer::new_offscreen_async(
        device.clone(),
        queue.clone(),
        None,
        false,
        false,
    )
    .await;
    let construction_error = device.pop_error_scope().await;
    let mut renderer = match created {
        Ok(renderer) => renderer,
        Err(error) => return Err(format!("Renderer::new_offscreen_async: {error:#}")),
    };
    set_stage(if construction_error.is_some() {
        "constructed_with_error"
    } else {
        "renderer_ok"
    });
    // 捕获到构造期校验错误时就**到此为止**：设备已经被拒过一次，继续渲染只会再抛一串错，
    // 而"哪一条管线被拒 + 原话"才是这一片要的结论。
    if let Some(error) = construction_error {
        return Ok(json!({
            "backend": format!("{:?}", info.backend),
            "device_type": format!("{:?}", info.device_type),
            "adapter_name": info.name,
            "limits": {
                "adapter_2d": adapter_limits.max_texture_dimension_2d,
                "requested_2d": requested.max_texture_dimension_2d,
                "device_2d_after_request": device.limits().max_texture_dimension_2d,
                "device_storage_buffers": device.limits().max_storage_buffers_per_shader_stage,
            },
            "renderer_created": true,
            "construction_validation_error": error.to_string(),
            "blocker": "渲染器构造期被 WebGL2 拒绝：存储缓冲在 WebGL2 上不可用（max_storage_buffers_per_shader_stage=0）",
        }));
    }

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpu-render-probe-target"),
        size: wgpu::Extent3d {
            width: RENDER_SIZE,
            height: RENDER_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // 宿主声明视图：主视图负责清屏（黑），2D overlay 视图给出 2D pass 的视口。
    // 声明视图本身也是要验的一段——清屏与视口不再写死在渲染器里（WP5 的 FrameViews）。
    let viewport = rustlua_camera::camera::Viewport::new(RENDER_SIZE as f32, RENDER_SIZE as f32)
        .map_err(|error| format!("Viewport::new: {error}"))?;
    let camera = rustlua_camera::camera::Camera2D::new(
        rustlua_math::math2d::Vec2::new(RENDER_SIZE as f32 * 0.5, RENDER_SIZE as f32 * 0.5),
        1.0,
    )
    .map_err(|error| format!("Camera2D::new: {error}"))?;
    renderer.set_frame_views(rustlua_camera::camera::FrameViews::layered(
        rustlua_camera::camera::RenderView::main_2d(camera, viewport, CLEAR_BLACK),
        rustlua_camera::camera::RenderView::overlay_2d(camera, viewport),
    ));

    // 贴图路径：web 上没有文件系统，所以必须先把像素**交进去**再画。这一步就是 WP5 第二片的
    // 前置条件——没有它，"图集/精灵能不能画对"在浏览器里无从验证：按路径解析必然落到白兜底，
    // 而白色恰好也是"什么都没画"的样子。
    renderer
        .upload_texture_rgba8(CHECKER_KEY, 2, 2, &CHECKER.concat())
        .map_err(|error| format!("upload_texture_rgba8: {error}"))?;
    set_stage("texture_uploaded");

    let mesh = synthetic_mesh();
    let outcome = renderer.render_into_texture(&mesh, None, &view, RENDER_SIZE, RENDER_SIZE);
    let presented = matches!(outcome, engine::host::RenderOutcome::Presented);
    set_stage("rendered");
    let pixels = read_back(&mut renderer, &target).await?;
    set_stage("readback_ok");

    // ---- 判据侧：采样点与"白色像素面积"都是**解析结论**，不是再渲一遍 ----
    let mut samples = Vec::new();
    let mut mismatches = 0u64;
    for (index, quad) in QUADS.iter().enumerate() {
        let ndc = [(quad[0] + quad[1]) * 0.5, (quad[2] + quad[3]) * 0.5];
        let (px, py) = ndc_to_pixel(ndc[0], ndc[1]);
        let actual = pixel_at(&pixels, px, py);
        let expected = [255u8, 255, 255, 255];
        if actual != expected {
            mismatches += 1;
        }
        samples.push(json!({
            "kind": "quad_center",
            "quad": index,
            "ndc": ndc,
            "pixel": [px, py],
            "expected_rgba": expected,
            "actual_rgba": actual,
        }));
    }
    let (bx, by) = ndc_to_pixel(0.0, 0.0);
    let background = pixel_at(&pixels, bx, by);
    let expected_background = [0u8, 0, 0, 255];
    if background != expected_background {
        mismatches += 1;
    }
    samples.push(json!({
        "kind": "background",
        "pixel": [bx, by],
        "expected_rgba": expected_background,
        "actual_rgba": background,
    }));

    // ---- 内存上传的贴图：四个纹素中心必须**逐通道**等于上传的字节 ----
    // uv → NDC（贴图四边形）→ 像素。`TEXTURED_QUAD` 的 uv 与 NDC 同向，
    // 与 `synthetic_mesh` 里 corner 的取值一一对应。
    let [tx0, tx1, ty0, ty1] = TEXTURED_QUAD;
    let mut texture_mismatches = 0u64;
    for (u, v, expected) in TEXEL_SAMPLES {
        let ndc_x = tx0 + u * (tx1 - tx0);
        let ndc_y = ty0 + v * (ty1 - ty0);
        let (px, py) = ndc_to_pixel(ndc_x, ndc_y);
        let actual = pixel_at(&pixels, px, py);
        if actual != expected {
            mismatches += 1;
            texture_mismatches += 1;
        }
        samples.push(json!({
            "kind": "texture_texel",
            "uv": [u, v],
            "ndc": [ndc_x, ndc_y],
            "pixel": [px, py],
            "expected_rgba": expected,
            "actual_rgba": actual,
        }));
    }

    // 结构检查：白色像素数必须≈两块四边形的解析面积。位置对、尺寸不对（例如 overlay 视口
    // 没生效、整帧被缩到四分之一）单点采样看不出来，面积能。
    let white_pixels = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200)
        .count();
    let expected_white_pixels: usize = QUADS
        .iter()
        .map(|quad| {
            let width = f64::from(quad[1] - quad[0]) * 0.5 * f64::from(RENDER_SIZE);
            let height = f64::from(quad[3] - quad[2]) * 0.5 * f64::from(RENDER_SIZE);
            (width.round() * height.round()) as usize
        })
        .sum();

    let metrics = renderer.last_frame_metrics();

    // ---- 画布表面（present）路径：web 宿主要用的**正是**这条路 ----
    //
    // 为什么这一节不能省：离屏出帧只证明"渲染器能画"，而 web 宿主必须把这一帧**present 到画布**上。
    // `game_web` 现在连 `Renderer` 都没有（画布只交给 JS 的 Canvas2D），所以这条路从来没被跑过。
    //
    // 第一个要问清的问题是**格式**：渲染器的颜色附件格式是构造时烘死的 `Rgba8UnormSrgb`
    // （`construct.rs`），管线与附件格式不匹配会被 wgpu 校验拒绝，而 wasm 是 `panic=abort`
    // ——**不能**抱着"试试看"的心态往不匹配的 surface 上渲。所以这里先问画布给什么格式：
    // 给 `Rgba8UnormSrgb` 就接着真渲一帧并读回画布纹理；不给就**只报告**（这是能力事实，
    // 不是回归），把"引擎必须把目标格式参数化"这条结论钉进报告。
    // 先把画布能力问清楚（`usages` 也要报：能不能把画布纹理拷出来读回，全看它）。
    set_stage("surface_caps_begin");
    let capabilities = surface.get_capabilities(&adapter);
    set_stage("surface_caps_ok");
    let formats: Vec<String> = capabilities
        .formats
        .iter()
        .map(|format| format!("{format:?}"))
        .collect();
    let mut surface_present = json!({
        "capabilities_formats": formats,
        "capabilities_present_modes": capabilities
            .present_modes
            .iter()
            .map(|mode| format!("{mode:?}"))
            .collect::<Vec<_>>(),
        "capabilities_usages": format!("{:?}", capabilities.usages),
        "capabilities_copy_src": capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC),
        "requested_format": "Rgba8UnormSrgb",
        "format_offered": capabilities
            .formats
            .contains(&wgpu::TextureFormat::Rgba8UnormSrgb),
        "any_srgb_offered": capabilities.formats.iter().any(|format| format.is_srgb()),
        "configured": false,
        "presented": false,
    });

    if !capabilities
        .formats
        .contains(&wgpu::TextureFormat::Rgba8UnormSrgb)
    {
        // 只看不渲：把"引擎必须支持哪些格式"这条结论连证据一起交出去。
        surface_present["blocker"] =
            json!("WebGL2 画布不提供 Rgba8UnormSrgb，而渲染器的颜色附件格式是构造时烘死的；要 present 到画布，必须先把目标格式参数化");
    } else {
        // **在错误域里跑**：这一节第一次把渲染结果交给画布，wgpu 的校验错误默认会 panic，
        // 而 wasm 是 `panic=abort`——整个模块会死，探针连"哪一步被拒"都报不出来（第一次跑就
        // 是这样死在 `get_capabilities`/`configure` 上的）。错误域把失败变成**数据**。
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        // 只申请 `RENDER_ATTACHMENT`：画布纹理能不能当 copy 源是**能力问题**，不该由我们替它假设。
        // 结果会写进 `capabilities_usages`/`capabilities_copy_src`，读回策略随之选择。
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                // 与渲染器烘死的格式必须一致：不一致就该报错而不是渲进去。
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                width: RENDER_SIZE,
                height: RENDER_SIZE,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: capabilities.alpha_modes[0],
                view_formats: vec![],
            },
        );
        let configure_error = device.pop_error_scope().await;
        set_stage("surface_configured");
        if let Some(error) = configure_error {
            surface_present["configure_error"] = json!(error.to_string());
            surface_present["blocker"] =
                json!("画布 surface 拒绝按渲染器的格式/用法配置（详见 configure_error）");
        } else {
            surface_present["configured"] = json!(true);
            // 渲染 + present **整段**都在错误域里：wgpu 对未捕获的校验错误直接 panic，
            // 而 wasm 是 `panic=abort`——一次不可见的校验错误就会让整个探针死掉、什么也报不出来
            // （第一次跑正是这样）。错误域把"哪一步被拒 + 原话"变成数据。
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            match surface.get_current_texture() {
                Ok(frame) => {
                    set_stage("surface_frame_ok");
                    // 直接渲进**画布纹理**：不需要"离屏再拷过去"的中间步骤，也不要求渲染器认识 surface。
                    let frame_view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let outcome = renderer.render_into_texture(
                        &mesh,
                        None,
                        &frame_view,
                        RENDER_SIZE,
                        RENDER_SIZE,
                    );
                    surface_present["outcome_presented"] =
                        json!(matches!(outcome, engine::host::RenderOutcome::Presented));
                    set_stage("surface_rendered");
                    frame.present();
                    set_stage("surface_presented");
                    surface_present["presented"] = json!(true);
                }
                Err(error) => {
                    // `get_current_texture` 自己返回 Result，不会 panic；失败也要留原话。
                    surface_present["get_current_texture_error"] = json!(error.to_string());
                }
            }
            if let Some(error) = device.pop_error_scope().await {
                surface_present["render_present_error"] = json!(error.to_string());
            }
            // 画布纹理能不能读回是**另一件事**，单独一个错误域：读得回来就逐通道对拍（与离屏同一套
            // 判据），读不回来就记下原话——它是能力缺口，不该把"present 成功"这条结论一起拖红。
            if surface_present["presented"] == json!(true) {
                if !capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC) {
                    // WebGL2 实测：画布只给 `RENDER_ATTACHMENT`，连 COPY_SRC 都申请不到。
                    // 所以"present 之后画布上到底是什么"无法在页内读回；离屏那一帧的逐像素判据
                    // 仍然有效，两者合起来才是完整证据——这个缺口必须写在报告里，不许含糊。
                    surface_present["canvas_readback"] = json!(
                        "画布纹理只支持 RENDER_ATTACHMENT（WebGL2 实测），无法逐像素对拍；逐像素判据取自离屏那一帧"
                    );
                } else {
                    match surface.get_current_texture() {
                        Ok(frame) => {
                            device.push_error_scope(wgpu::ErrorFilter::Validation);
                            match read_back(&mut renderer, &frame.texture).await {
                                Ok(pixels) => {
                                    set_stage("surface_readback_ok");
                                    let mut mismatches = 0u64;
                                    let mut checks: Vec<(u32, u32, [u8; 4])> = Vec::new();
                                    for quad in QUADS {
                                        let ndc_x = (quad[0] + quad[1]) * 0.5;
                                        let ndc_y = (quad[2] + quad[3]) * 0.5;
                                        let (px, py) = ndc_to_pixel(ndc_x, ndc_y);
                                        checks.push((px, py, [255, 255, 255, 255]));
                                    }
                                    let (bx, by) = ndc_to_pixel(0.0, 0.0);
                                    checks.push((bx, by, [0, 0, 0, 255]));
                                    for (u, v, expected) in TEXEL_SAMPLES {
                                        let ndc_x = tx0 + u * (tx1 - tx0);
                                        let ndc_y = ty0 + v * (ty1 - ty0);
                                        let (px, py) = ndc_to_pixel(ndc_x, ndc_y);
                                        checks.push((px, py, expected));
                                    }
                                    let mut samples = Vec::with_capacity(checks.len());
                                    for (px, py, expected) in checks {
                                        let actual = pixel_at(&pixels, px, py);
                                        if actual != expected {
                                            mismatches += 1;
                                        }
                                        samples.push(json!({
                                            "pixel": [px, py],
                                            "expected_rgba": expected,
                                            "actual_rgba": actual,
                                        }));
                                    }
                                    surface_present["canvas_mismatches"] = json!(mismatches);
                                    surface_present["canvas_samples"] = json!(samples);
                                }
                                Err(error) => {
                                    surface_present["canvas_readback_error"] = json!(error);
                                }
                            }
                            if let Some(error) = device.pop_error_scope().await {
                                surface_present["canvas_readback_validation_error"] =
                                    json!(error.to_string());
                            }
                        }
                        Err(error) => {
                            surface_present["canvas_frame_error"] = json!(error.to_string());
                        }
                    }
                }
            }
        }
    }

    Ok(json!({
        "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "adapter_name": info.name,
        "limits": {
            "adapter_2d": adapter_limits.max_texture_dimension_2d,
            "requested_2d": requested.max_texture_dimension_2d,
            "device_2d_after_request": device.limits().max_texture_dimension_2d,
            "device_storage_buffers": device.limits().max_storage_buffers_per_shader_stage,
        },
        "renderer_created": true,
        "outcome_presented": presented,
        "samples": samples,
        "mismatches": mismatches,
        "texture_mismatches": texture_mismatches,
        "white_pixels": white_pixels,
        "expected_white_pixels": expected_white_pixels,
        "draw_calls": metrics.draw_calls,
        "triangles": metrics.triangles,
        "dropped_draws": metrics.dropped_draws,
        "cpu_frame_us": metrics.cpu_frame_us,
        "pass_count": renderer.last_frame_passes().len(),
        "loaded_texture_count": renderer.loaded_texture_count(),
        "readback_bytes": pixels.len(),
        "surface_present": surface_present,
    }))
}

/// 手工合成最小 2D 场景：三个四边形（两块内建白贴图 + 一块内存上传的 2×2 棋盘），
/// NDC 坐标（`OverlayVertex` 就是 NDC）。
fn synthetic_mesh() -> engine::host::SceneMesh {
    let rects = [QUADS[0], QUADS[1], TEXTURED_QUAD];
    let mut vertices = Vec::with_capacity(rects.len() * 6);
    let mut draws = Vec::with_capacity(rects.len());
    for (index, quad) in rects.iter().enumerate() {
        let [x0, x1, y0, y1] = *quad;
        let start = vertices.len() as u32;
        let corner = |x: f32, y: f32, u: f32, v: f32| engine::host::OverlayVertex {
            position: [x, y],
            uv: [u, v],
            // 顶点色全白：白贴图那块因此恒白，棋盘那块**逐纹素等于上传的字节**。
            // 顶点色不参与"配方"，任何偏差都只能来自渲染路径本身。
            color: [1.0, 1.0, 1.0, 1.0],
            sdf: 0,
        };
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y0, 1.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y0, 0.0, 0.0));
        vertices.push(corner(x1, y1, 1.0, 1.0));
        vertices.push(corner(x0, y1, 0.0, 1.0));
        // 前两块 `texture == 0` = 内建白贴图，完全不解析路径；第三块 `texture == 1` 指向
        // `paths[0]`，也就是上面上传进缓存的那个**非文件**键。
        let texture = if index == rects.len() - 1 { 1 } else { 0 };
        draws.push(engine::host::SceneDraw::screen_ui(texture, start, 6));
    }
    engine::host::SceneMesh {
        vertices,
        draws,
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![CHECKER_KEY.to_owned()],
        // 空 = 宿主**手写**的 mesh（没有资源身份）：渲染器回退到按路径解析，这正是本片的情形。
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

/// NDC → 像素。NDC 的 y 向上（+1 在上），而纹理行 0 在**上**，所以行要翻过来。
fn ndc_to_pixel(ndc_x: f32, ndc_y: f32) -> (u32, u32) {
    let max = (RENDER_SIZE - 1) as f32;
    let x = ((ndc_x + 1.0) * 0.5 * RENDER_SIZE as f32)
        .floor()
        .clamp(0.0, max);
    let y = ((1.0 - ndc_y) * 0.5 * RENDER_SIZE as f32)
        .floor()
        .clamp(0.0, max);
    (x as u32, y as u32)
}

fn pixel_at(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = (y as usize * RENDER_SIZE as usize + x as usize) * 4;
    match pixels.get(index..index + 4) {
        Some(pixel) => [pixel[0], pixel[1], pixel[2], pixel[3]],
        None => [0, 0, 0, 0],
    }
}

/// 读回一帧：`readback_frame`（提交一次 copy）之后只用 `readback_pending`（**不**再提交）轮询。
///
/// 为什么不能循环调 `readback_frame`：它每次调用都新提交一次 copy，而读回槽只有两个——
/// 连调会把"丢弃"打满，测到的是自己的重试而不是渲染结果。
/// 为什么每轮都要让出**宏任务**：`map_async` 的回调由浏览器主循环派发（GL fence 在同一个 JS
/// 宏任务里通常还没 SIGNALED），promise microtask 不够，必须 `setTimeout(0)`
/// （见 `yield_to_event_loop` 的注释）。
async fn read_back(
    renderer: &mut engine::host::Renderer,
    target: &wgpu::Texture,
) -> Result<Vec<u8>, String> {
    let mut status = renderer.readback_frame(target, RENDER_SIZE, RENDER_SIZE);
    for _ in 0..60 {
        match status {
            engine::host::ReadbackStatus::Ready { pixels, .. } => return Ok(pixels),
            engine::host::ReadbackStatus::Failed(error) => {
                return Err(format!("readback failed: {error}"));
            }
            engine::host::ReadbackStatus::Pending => {
                yield_to_event_loop().await;
                status = renderer.readback_pending();
            }
        }
    }
    Err("readback 让出 60 次事件循环仍未就绪".to_owned())
}
