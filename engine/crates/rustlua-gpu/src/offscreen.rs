//! 离屏设备与目标纹理的公共管子（M77-02 offscreen 合并）。
//!
//! `engine/examples/wp0_*` 过去每个例子各写一份「建 instance → 取 fallback adapter →
//! `request_device` → 建 `RENDER_ATTACHMENT | COPY_SRC` 目标纹理 → 建 view」的样板。这 22 个例子里
//! 有 21 个的目标纹理描述符**逐字段相同**（`Rgba8UnormSrgb` / 1 mip / 1 sample / `D2` /
//! `RENDER_ATTACHMENT | COPY_SRC` / 空 `view_formats`），只有标签和尺寸不同；设备侧的
//! `LowPower` + `force_fallback_adapter: true` + `Limits::default()` + `MemoryHints::Performance`
//! 也全部相同。这里收成一个入口，例子侧只声明「叫什么、多大、要不要时间戳特性」。两个粒度：
//! [`OffscreenTarget`]（要自己那张目标纹理的）与 [`OffscreenDevice`]（只建设备，画面缓冲由
//! `Renderer::new_offscreen(.., None, ..)` 自己造的）。
//!
//! 只覆盖**基准/示例**这一路的设备获取；编辑器预览那一套（`editor/src/preview_session.rs`）
//! 仍住在 editor 里，它带的是编辑器的取景与错误显示语义，不在本模块的口径里。

use thiserror::Error;

/// 一次离屏 fixture 的失败原因。两个变体的 `Display` 都直接透传 wgpu 的原文，
/// 所以例子侧原有的 `error={error}` / `blocked(&format!("…：{error}"))` 文案不变。
#[derive(Debug, Error)]
pub enum OffscreenError {
    /// 取不到 adapter（本机没有可用后端，或 `WGPU_ADAPTER` 之类的环境要求不成立）。
    #[error("{0}")]
    Adapter(wgpu::RequestAdapterError),
    /// 取到了 adapter，但建设备失败。
    #[error("{0}")]
    Device(wgpu::RequestDeviceError),
}

/// 一次离屏 fixture 的声明。默认值就是 `wp0_*` 例子共用的那一套。
#[derive(Debug, Clone)]
pub struct OffscreenRequest<'a> {
    /// 打在 device 上的标签，同时也是目标纹理标签的前缀（`"wp0-2d-golden"` → `"wp0-2d-golden-target"`）。
    pub label: &'a str,
    pub width: u32,
    pub height: u32,
    /// 目标纹理格式；`wp0_*` 全部用 `Rgba8UnormSrgb`。
    pub format: wgpu::TextureFormat,
    /// 要 `TIMESTAMP_QUERY` 一族。**adapter 不支持时自动降级为空特性**——这一手不改变
    /// 画面，只决定帧耗时能不能报出 GPU 侧数字；买没买到由渲染器自己报
    /// （`Renderer::gpu_timestamp_supported` / `gpu_timestamp_reason`）。
    pub want_timestamps: bool,
    /// 强制走 fallback（软件光栅）adapter——基准要的是可复现的绘制次数而不是最快帧率。
    pub force_fallback_adapter: bool,
}

impl<'a> OffscreenRequest<'a> {
    /// 默认：`Rgba8UnormSrgb`、不要时间戳、强制 fallback adapter。
    pub fn new(label: &'a str, width: u32, height: u32) -> Self {
        Self {
            label,
            width,
            height,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            want_timestamps: false,
            force_fallback_adapter: true,
        }
    }

    pub fn with_timestamps(mut self) -> Self {
        self.want_timestamps = true;
        self
    }

    /// 不强制 fallback：要真实 GPU 的目视/性能证据时用这个。
    pub fn on_real_adapter(mut self) -> Self {
        self.force_fallback_adapter = false;
        self
    }
}

/// 已建好的离屏设备。
///
/// 给「纹理由渲染器自己造」的例子用（`Renderer::new_offscreen(.., None, ..)`）；要自己控制
/// 目标纹理尺寸与格式的用 [`OffscreenTarget`]，它就是在这层上面加一张纹理。
///
/// `queue` 用借用式访问器交出：`Renderer::new_offscreen` 要按值拿 `Queue`，而 `wgpu::Queue`
/// 是 `Arc` 包着的廉价克隆，例子侧沿用原来的 `queue.clone()` 即可。
pub struct OffscreenDevice {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
}

impl OffscreenDevice {
    /// 只要设备与队列的例子用：默认口径（不要时间戳、强制 fallback adapter、`Limits::default()`）。
    pub fn acquire(label: &str) -> Result<Self, OffscreenError> {
        Self::acquire_with(&OffscreenRequest::new(label, 0, 0))
    }

    fn acquire_with(request: &OffscreenRequest<'_>) -> Result<Self, OffscreenError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: request.force_fallback_adapter,
            })) {
                Ok(adapter) => adapter,
                Err(error) => return Err(OffscreenError::Adapter(error)),
            };
        let adapter_info = adapter.get_info();
        let timestamp_features =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let timestamps = request.want_timestamps && adapter.features().contains(timestamp_features);
        let required_features = if timestamps {
            timestamp_features
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some(request.label),
                required_features,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(pair) => pair,
                Err(error) => return Err(OffscreenError::Device(error)),
            };
        Ok(Self {
            device,
            queue,
            adapter_info,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }
}

/// 已建好的设备 + 离屏目标纹理 + 它的 view。
pub struct OffscreenTarget {
    inner: OffscreenDevice,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OffscreenTarget {
    pub fn acquire(request: &OffscreenRequest<'_>) -> Result<Self, OffscreenError> {
        let inner = OffscreenDevice::acquire_with(request)?;
        let texture_label = format!("{}-target", request.label);
        let texture = inner.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&texture_label),
            size: wgpu::Extent3d {
                width: request.width,
                height: request.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: request.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            inner,
            texture,
            view,
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.inner.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.inner.queue
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.inner.adapter_info
    }
}
