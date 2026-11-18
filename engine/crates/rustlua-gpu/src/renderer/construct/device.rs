//! `Renderer::new_async` 的**设备与能力探测**段（拆分批 1 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `docs/62` 的 1.7 版更正记下了 `new_async` 2,371 行的来历：WP12 当时定的是
//! "只搬位置、不拆函数体"，拆函数体留给"PBR/HDR 落地时顺势做"的工作包；
//! PBR/HDR 已于 2026-10-23/24 交付，而那个包没开。这一段就是那个工作包的第一批。
//!
//! 搬家前的位置是 `new_async` 里从 `// Surface mode: …` 到
//! "设备没有存储缓冲"那条告警的 `}` 为止——148 行、**0 处 `device.create_*`**。
//! 它只做三件事：建设备、采能力快照、按能力算几个派生开关。
//!
//! ## 为什么先搬这一段
//!
//! 它是**唯一不含资源创建**的段落。拆函数体的风险主要在"资源创建的顺序被改动"
//! （见 `tests/construct_creation_order.rs`），而这一段一处创建都没有，
//! 所以它是验证拆分手法本身的最低风险样板：如果这里出问题，那是拆分方式错了，
//! 而不是某条管线被搬坏了。
//!
//! ## 语义没有任何变化
//!
//! 逐行搬运 + 把局部变量收进 [`BootDevice`] 返回。以下三处是**必须留意**的等价点：
//!
//! 1. `adapter_for_caps` 只在窗口路径被赋值（离屏路径没有 adapter）——收进结构体后
//!    仍是 `Option`，调用方拿它去探能力，判据不变。
//! 2. **两条路径的 `format` 不同**：窗口路径按 surface 能力挑 sRGB，离屏路径硬编码
//!    `Rgba8UnormSrgb`。这个差异是**故意的**（离屏输出要确定性），不是遗漏。
//! 3. 告警与 info 日志的**触发条件与文案**逐字保留——日志文案本身就是诊断契约
//!    （`docs/62` 里多次以"日志里一个字都没有"批评过静默失败）。

use std::sync::Arc;

use wgpu::{
    CompositeAlphaMode, Device, DeviceDescriptor, Instance, InstanceDescriptor, PresentMode, Queue,
    Surface, SurfaceConfiguration, TextureUsages,
};
use winit::window::Window;

use crate::renderer::RendererCapabilities;

/// 设备探测的产物：一次 `new_async` 只在开头建一次，后面所有管线/缓冲都靠它。
pub(crate) struct BootDevice {
    pub device: Device,
    pub queue: Queue,
    /// 只有窗口路径才有 surface；离屏渲染器（golden / 编辑器预览）没有。
    pub surface: Option<Surface<'static>>,
    /// 窗口路径按 surface 能力挑的 sRGB 格式；离屏路径固定 `Rgba8UnormSrgb`。
    pub format: wgpu::TextureFormat,
    pub config: SurfaceConfiguration,
    /// 日志与 Profiler 用的后端名（窗口路径取 adapter 名，离屏固定 `offscreen`）。
    pub adapter_name: String,
    /// 设备有没有存储缓冲。构造路径上有三处要看它：矢量文字的两个绑定、
    /// 3D 材质的逐实例矩阵（storage 数组 ↔ 64 字节 uniform）、以及实例环/实例化合批。
    pub storage_capable: bool,
    /// 已按设备能力收敛过的实例化开关（`instancing_enabled && storage_capable`）。
    pub instancing_enabled: bool,
    /// `STORAGE` 或空——给后面所有"要不要按存储缓冲建"的 `BufferUsages` 用。
    pub storage_usage: wgpu::BufferUsages,
    /// 构造期采一次的能力快照（M74-03 P0）。
    pub capabilities: RendererCapabilities,
}

/// 建设备、采能力、算派生开关。
///
/// 参数与 `new_async` 的对应段一一对应；`instancing_enabled` 是**入参经能力收敛后**的值
/// （能力不足时会被关掉，因为逐实例模型矩阵吃存储缓冲）。
pub(crate) async fn boot_device(
    window: Option<Arc<Window>>,
    external_device: Option<(Device, Queue)>,
    instancing_enabled: bool,
) -> anyhow::Result<BootDevice> {
    // Surface mode: create instance/adapter/device from the window.
    // Offscreen mode: use the caller-provided device/queue.
    // 窗口路径的 adapter 只用于能力探测；离屏路径没有 adapter，靠限制值判定。
    let mut adapter_for_caps: Option<wgpu::Adapter> = None;
    let (device, queue, surface, format, config, adapter_name) = if let Some(window) = window {
        let size = window.inner_size();
        let instance = Instance::new(&InstanceDescriptor::default());
        let surface = instance
            .create_surface(window)
            .map_err(|error| anyhow::anyhow!("could not create surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| anyhow::anyhow!("could not find a compatible GPU adapter: {error}"))?;
        let adapter_name = adapter.get_info().name;
        let timestamp_features =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let required_features = if adapter.features().contains(timestamp_features) {
            timestamp_features
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("rustlua-render-device"),
                required_features,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| anyhow::anyhow!("could not create GPU device: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("GPU surface reported no supported formats"))?;
        let present_mode = if capabilities.present_modes.contains(&PresentMode::Mailbox) {
            PresentMode::Mailbox
        } else {
            PresentMode::Fifo
        };
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(CompositeAlphaMode::Auto);
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        adapter_for_caps = Some(adapter);
        (device, queue, Some(surface), format, config, adapter_name)
    } else {
        let (device, queue) = external_device
            .ok_or_else(|| anyhow::anyhow!("offscreen renderer requires a device/queue"))?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1280,
            height: 720,
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        (device, queue, None, format, config, "offscreen".to_owned())
    };
    // 设备有没有存储缓冲（WebGL2 / 任何 downlevel 设备上都是 0）。构造路径上有三处要看它：
    // 矢量文字的两个绑定、3D 材质的逐实例矩阵（storage 数组 ↔ 64 字节 uniform 两种取法）、
    // 以及实例环/实例化合批。不该建的去建**不是**"功能少一点"，而是在
    // `create_bind_group_layout` 上直接吃一条校验错误——wgpu 对未被 error scope 捕获的
    // 校验错误走默认 handler，wasm 上是 `panic=abort`，整个渲染器就此死掉。
    // 所以这里按**设备能力**（而不是按平台 `cfg`）分叉：原生桌面照旧走存储缓冲，
    // web 走"没有矢量文字、3D 换 uniform 取法"的构造。
    let storage_capable = device.limits().max_storage_buffers_per_shader_stage > 0;
    // M74-03 P0：能力快照在构造期采一次。`adapter_for_caps` 只有窗口路径才有——
    // 离屏渲染器拿的是外部 device（编辑器 eframe），没有 downlevel flag 可查，
    // 由 `RendererCapabilities::probe` 退到限制值判定。
    let capabilities =
        RendererCapabilities::probe(&device, &queue, adapter_for_caps.as_ref()).await;
    if let Some(reason) = capabilities.compute_unavailable_reason() {
        tracing::info!(
            target: "render",
            backend = capabilities.backend_name(),
            reason = %reason,
            "[render] compute 通道不可用：GPU 粒子等消费者必须走 CPU 档"
        );
    } else {
        tracing::info!(
            target: "render",
            backend = capabilities.backend_name(),
            workgroup = ?capabilities.max_compute_workgroup_size,
            "[render] compute 通道可用"
        );
    }
    // P3-R01：HDR 能力是**两轴独立证据**（可渲染/读回 + 可线性过滤）。这里只报告，不决定
    // 是否启用——是否启用由 `RenderSettings.hdr` 与本帧内容在 `render_passes` 里解析。
    if let Some(reason) = capabilities.hdr_unavailable_reason() {
        tracing::info!(
            target: "render",
            backend = capabilities.backend_name(),
            reason = %reason,
            "[render] HDR 场景颜色不可用：HDR 策略将回退到 LDR"
        );
    } else {
        tracing::info!(
            target: "render",
            backend = capabilities.backend_name(),
            "[render] HDR 场景颜色可用（Rgba16Float + 线性过滤）；默认策略仍为关闭"
        );
    }
    let storage_usage = if storage_capable {
        wgpu::BufferUsages::STORAGE
    } else {
        wgpu::BufferUsages::empty()
    };
    // 实例化本身也吃存储缓冲（逐实例模型矩阵），能力不足时把参数一并关掉：
    // 这是"同一件事的两条实现里选不需要存储缓冲的那条"，不是少画。
    let instancing_enabled = instancing_enabled && storage_capable;
    if !storage_capable {
        // 一次告警，而不是每帧：矢量文字的两个绑定是存储缓冲，这条设备上只剩 SDF 文字
        // 与 2D 通路；3D 网格/阴影改走 64 字节 uniform 的模型矩阵（**照样画**，只是
        // 逐 draw 一次绑定、不做实例化合批）。漏画而日志里一个字都没有，是本仓最不能
        // 接受的失败类型，所以这里明确说清"哪条路换了实现、哪条路真的没有"。
        tracing::warn!(target: "game",
            "设备没有存储缓冲（max_storage_buffers_per_shader_stage=0，WebGL2 即如此）：\
             3D 网格/阴影改走 per-draw uniform 模型矩阵（实例化合批关闭），\
             矢量文字不构造（宿主应改用 SDF 文字，未改的 draw 会计入 dropped_draws）");
    }

    Ok(BootDevice {
        device,
        queue,
        surface,
        format,
        config,
        adapter_name,
        storage_capable,
        instancing_enabled,
        storage_usage,
        capabilities,
    })
}
