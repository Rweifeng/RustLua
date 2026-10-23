//! 渲染器**设备能力**的单一采集点（M74-03 P0；P3-R01 扩 HDR 两轴）。
//!
//! 为什么要有这个模块：渲染器里已经散着两套"这台机器能不能用某条路"的判断——
//! `storage_capable`（`device.limits().max_storage_buffers_per_shader_stage > 0`）和
//! `timestamp_enabled`（`device.features()`）。compute 如果各自再写一套，就会出现
//! "构造期说能、调度期说不能"或反过来的分叉。这里把**本批次用到的能力**收成一份只读快照：
//! 构造期采一次，之后不变，`Renderer::capabilities()` 与所有消费点都读同一份。
//!
//! 口径（与 `docs/62` §6 / `tasks/done/M74-03` 的 P0 一致）：
//!
//! - `compute` 必须同时满足**后端声明**与**限制值非零**。WebGL2 的
//!   `DownlevelFlags::COMPUTE_SHADERS` 不置位，且 `Limits::downlevel_webgl2_defaults()` 里
//!   `max_compute_workgroup_size_*` 全为 0——两条独立证据都指向 false，不能只看其一。
//! - 离屏渲染器（编辑器/eframe）拿到的是**外部 device**，没有 adapter 可查 downlevel
//!   flags：这时只按 `device.limits()` 判定，并在 `backend` 里保留 `None`（"外部设备"）。
//! - 这个结构体只描述**设备能做什么**，不描述**用户开没开**。后者是
//!   [`super::Renderer::set_render_settings`] 的策略位，两者不能混成一个字段。
//! - HDR（P3-R01）不看"后端名字"：在真实 device 上创建 `Rgba16Float` 纹理、清成 8.0、
//!   回读并验证 `> 1`，再单独建一次**线性过滤**绑定。两条证据都给过才算 `hdr_color` 可用；
//!   失败只让能力位变 false，不 panic（`push_error_scope` 把校验错误变成数据）。

#[cfg(test)]
use wgpu::Limits;
use wgpu::{Backend, DownlevelFlags};

#[cfg(not(target_arch = "wasm32"))]
use super::readback::f16_bits_to_f32;

/// HDR 探针纹理边长（4×4 足够验证格式，不为探针分配可见显存）。
const HDR_PROBE_SIZE: u32 = 4;
/// HDR 探针清屏值：红=8.0，任何把浮点目标夹到 1.0 的实现都会在回读里暴露。
const HDR_PROBE_RED: f64 = 8.0;

/// 构造期采集的设备能力快照（只读，之后不变）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RendererCapabilities {
    /// 后端。`None` = 设备由调用方提供（离屏渲染器），没有可查询的 adapter。
    pub backend: Option<Backend>,
    /// 是否具备 compute 通道（后端 + 限制值双条件）。
    pub compute: bool,
    /// 是否有存储缓冲（WebGL2 上为 false；矢量文字/3D 实例环/GPU 粒子都依赖它）。
    pub storage_buffers: bool,
    /// 是否启用了 GPU 时间戳查询。
    pub timestamp: bool,
    /// `Rgba16Float` 是否能作为**可读回**的渲染目标（清 8.0 后回读仍 >1）。
    pub hdr_color: bool,
    /// `Rgba16Float` 是否能被**线性过滤**采样（Tonemap 的全屏采样依赖它）。
    pub float_linear_filter: bool,
    /// 计算着色器各维度的 workgroup 上限（不可用时全 0）。
    pub max_compute_workgroup_size: [u32; 3],
    /// 单个 workgroup 的调用数上限。
    pub max_compute_invocations_per_workgroup: u32,
    /// 单次 dispatch 的 workgroup 数上限（每维）。
    pub max_compute_workgroups_per_dimension: u32,
    /// 单个存储缓冲绑定可覆盖的字节数上限。
    pub max_storage_buffer_binding_size: u64,
}

impl RendererCapabilities {
    /// 采集一份能力快照。
    ///
    /// `adapter` 为 `None` 时（外部 device）跳过 downlevel flag 判定——那种 device 不是本
    /// 渲染器创建的，拿不到 adapter，只能相信 `device.limits()`。这也是编辑器离屏路径的现状。
    /// HDR 的能力位**不依赖 adapter**：外部 device 同样跑真实的纹理/渲染/过滤探针。
    pub(crate) async fn probe(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        adapter: Option<&wgpu::Adapter>,
    ) -> Self {
        let limits = device.limits();
        let backend = adapter.map(|adapter| adapter.get_info().backend);
        // WebGL2 的两条证据：downlevel flag 不置位、compute 限制全 0。两条都要，缺一不可。
        let downlevel_compute = adapter.is_none_or(|adapter| {
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(DownlevelFlags::COMPUTE_SHADERS)
        });
        let limits_compute = limits.max_compute_workgroup_size_x > 0
            && limits.max_compute_workgroup_size_y > 0
            && limits.max_compute_workgroup_size_z > 0
            && limits.max_compute_invocations_per_workgroup > 0
            && limits.max_compute_workgroups_per_dimension > 0;
        let storage_buffers = limits.max_storage_buffers_per_shader_stage > 0;
        let (hdr_color, float_linear_filter) = probe_hdr(device, queue, adapter).await;
        Self {
            backend,
            compute: downlevel_compute && limits_compute && storage_buffers,
            storage_buffers,
            timestamp: device.features().contains(
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
            ),
            hdr_color,
            float_linear_filter,
            max_compute_workgroup_size: [
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ],
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_storage_buffer_binding_size: u64::from(limits.max_storage_buffer_binding_size),
        }
    }

    /// 纯函数口径的构造入口（单测用来钉死"WebGL2 必须读到 false"这条硬门）。
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_limits(
        limits: &Limits,
        downlevel_flags: Option<DownlevelFlags>,
        backend: Option<Backend>,
        timestamp: bool,
        hdr_color: bool,
        float_linear_filter: bool,
    ) -> Self {
        let downlevel_compute =
            downlevel_flags.is_none_or(|flags| flags.contains(DownlevelFlags::COMPUTE_SHADERS));
        let limits_compute = limits.max_compute_workgroup_size_x > 0
            && limits.max_compute_workgroup_size_y > 0
            && limits.max_compute_workgroup_size_z > 0
            && limits.max_compute_invocations_per_workgroup > 0
            && limits.max_compute_workgroups_per_dimension > 0;
        let storage_buffers = limits.max_storage_buffers_per_shader_stage > 0;
        Self {
            backend,
            compute: downlevel_compute && limits_compute && storage_buffers,
            storage_buffers,
            timestamp,
            hdr_color,
            float_linear_filter,
            max_compute_workgroup_size: [
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ],
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_storage_buffer_binding_size: u64::from(limits.max_storage_buffer_binding_size),
        }
    }

    /// 后端短名（日志/诊断用；外部 device 为 `"external"`）。
    #[must_use]
    pub const fn backend_name(&self) -> &'static str {
        match self.backend {
            Some(Backend::Noop) => "noop",
            Some(Backend::Vulkan) => "vulkan",
            Some(Backend::Metal) => "metal",
            Some(Backend::Dx12) => "dx12",
            Some(Backend::Gl) => "gl",
            Some(Backend::BrowserWebGpu) => "webgpu",
            None => "external",
        }
    }

    /// 没有 compute 时的**可读原因**（有 compute 时为 `None`）。
    ///
    /// 为什么要返回字符串而不是一个 bool：降级路径必须能说清"为什么慢/为什么没画"，
    /// 否则日志里只剩"没效果"。
    #[must_use]
    pub fn compute_unavailable_reason(&self) -> Option<String> {
        if self.compute {
            return None;
        }
        let mut reasons = Vec::new();
        if !self.storage_buffers {
            reasons.push("max_storage_buffers_per_shader_stage=0".to_owned());
        }
        if self.max_compute_workgroup_size == [0, 0, 0] {
            reasons.push("max_compute_workgroup_size_*=0".to_owned());
        }
        if self.max_compute_invocations_per_workgroup == 0 {
            reasons.push("max_compute_invocations_per_workgroup=0".to_owned());
        }
        if self.max_compute_workgroups_per_dimension == 0 {
            reasons.push("max_compute_workgroups_per_dimension=0".to_owned());
        }
        if reasons.is_empty() {
            reasons.push("backend does not expose COMPUTE_SHADERS".to_owned());
        }
        Some(format!(
            "backend={} 不支持 compute（{}）",
            self.backend_name(),
            reasons.join("; ")
        ))
    }

    /// HDR 场景颜色是否**完整可用**（渲染 + 读回 + 线性过滤三条都成立）。
    #[must_use]
    pub const fn hdr_usable(&self) -> bool {
        self.hdr_color && self.float_linear_filter
    }

    /// HDR 不可用时的**可读原因**（可用时为 `None`）。
    ///
    /// 分层报告是刻意的：`hdr_color=false` 与 `float_linear_filter=false` 是两条独立证据，
    /// 只给一个"不支持 HDR"会让硬件/后端问题无从查起。
    #[must_use]
    pub fn hdr_unavailable_reason(&self) -> Option<String> {
        if self.hdr_usable() {
            return None;
        }
        let mut reasons = Vec::new();
        if !self.hdr_color {
            reasons
                .push("Rgba16Float 不能作为可读回的渲染目标（清 8.0 后回读未保持 >1）".to_owned());
        }
        if !self.float_linear_filter {
            reasons.push("Rgba16Float 不支持线性过滤（Tonemap 采样不可用）".to_owned());
        }
        Some(format!(
            "backend={} 不支持 HDR 场景颜色（{}）",
            self.backend_name(),
            reasons.join("; ")
        ))
    }
}

/// 在真实 device 上验证 `Rgba16Float` 的三件事：可渲染、可读回、可线性过滤。
///
/// 顺序是先看 adapter 的格式声明（有 adapter 时），再做 device 级探针——格式声明是**必要
/// 条件**，device 探针才是**充分证据**（离屏外部 device 根本没有 adapter）。
async fn probe_hdr(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    adapter: Option<&wgpu::Adapter>,
) -> (bool, bool) {
    if let Some(adapter) = adapter {
        let features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
        let render = features
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
            && features
                .allowed_usages
                .contains(wgpu::TextureUsages::TEXTURE_BINDING)
            && features
                .allowed_usages
                .contains(wgpu::TextureUsages::COPY_SRC);
        if !render
            || !features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
        {
            return (false, false);
        }
    }

    // ① 渲染 + 读回：清 8.0，拷贝到 staging buffer，映射后验证红色分量仍 > 7.9。
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-hdr-capability-probe"),
        size: wgpu::Extent3d {
            width: HDR_PROBE_SIZE,
            height: HDR_PROBE_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bytes_per_row = 256_u32;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-hdr-capability-probe-readback"),
        size: u64::from(bytes_per_row) * u64::from(HDR_PROBE_SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rustlua-hdr-capability-probe-encoder"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-hdr-capability-probe-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: HDR_PROBE_RED,
                        g: 0.5,
                        b: 0.25,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(HDR_PROBE_SIZE),
            },
        },
        wgpu::Extent3d {
            width: HDR_PROBE_SIZE,
            height: HDR_PROBE_SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let readback_ok = read_hdr_probe(device, &staging).await;
    let render_error = pop_error_scope(device).await;
    // wasm 上无法同步等映射回调：`None` 表示"读回未验"，此时把校验错误当作唯一证据，
    // 真正的读回由 native golden 与 `gpu-web-probe` 覆盖。
    let hdr_color = render_error.is_none() && readback_ok != Some(false);

    // ② 线性过滤：建一个 Filtering 采样器 + 声明 filterable 的纹理绑定。
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("rustlua-hdr-capability-probe-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-hdr-capability-probe-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let _bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rustlua-hdr-capability-probe-bind-group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let filter_error = pop_error_scope(device).await;
    (hdr_color, filter_error.is_none())
}

/// 读取 HDR 探针的 4×4 回读，验证红色分量确实超过 LDR 上限。
///
/// 仅 native 走真实映射；wasm 返回 `None`（"未验"），由校验错误与 web 探针兜底。
#[cfg(not(target_arch = "wasm32"))]
async fn read_hdr_probe(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Option<bool> {
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let _ = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    receiver.recv().ok()?.ok()?;
    let data = slice.get_mapped_range();
    // 每像素 RGBA 各 2 字节；第一像素的红色就是清屏的 8.0。
    let red_bits = u16::from_le_bytes([data[0], data[1]]);
    let red = f16_bits_to_f32(red_bits);
    drop(data);
    buffer.unmap();
    Some(red > 7.9)
}

#[cfg(target_arch = "wasm32")]
async fn read_hdr_probe(_device: &wgpu::Device, _buffer: &wgpu::Buffer) -> Option<bool> {
    None
}

/// 弹出校验错误域。native 用 `pollster` 驱动（与 `compute.rs` 同一条路），wasm 直接 await。
async fn pop_error_scope(device: &wgpu::Device) -> Option<wgpu::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        pollster::block_on(device.pop_error_scope())
    }
    #[cfg(target_arch = "wasm32")]
    {
        device.pop_error_scope().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external_caps(hdr_color: bool, float_linear_filter: bool) -> RendererCapabilities {
        RendererCapabilities::from_limits(
            &Limits::default(),
            Some(DownlevelFlags::COMPUTE_SHADERS),
            Some(Backend::Vulkan),
            true,
            hdr_color,
            float_linear_filter,
        )
    }

    /// **硬门**（M74-03 P0）：WebGL2 的默认限制必须推出 `compute=false`。
    ///
    /// 这条若失败，说明对 WebGL2 的判断错了，整条 compute 排期都要重写——所以它先于任何
    /// GPU 用例，且不依赖真实适配器。
    #[test]
    fn webgl2_limits_report_no_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            Some(DownlevelFlags::empty()),
            Some(Backend::Gl),
            false,
            false,
            false,
        );
        assert!(
            !capabilities.compute,
            "WebGL2 必须读到 compute=false：{capabilities:?}"
        );
        assert!(!capabilities.storage_buffers);
        let reason = capabilities
            .compute_unavailable_reason()
            .expect("无能力必须给出原因");
        assert!(reason.contains("gl"), "{reason}");
        assert!(reason.contains("storage"), "{reason}");
    }

    /// 只有限制值、没有 downlevel flags 时（外部 device）也必须按限制值判 false。
    #[test]
    fn external_device_with_zero_compute_limits_reports_no_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            None,
            None,
            false,
            false,
            false,
        );
        assert!(!capabilities.compute);
        assert_eq!(capabilities.backend_name(), "external");
    }

    /// 桌面默认限制必须推出 `compute=true`（否则 P1 的接缝没有可验证的档位）。
    #[test]
    fn desktop_default_limits_report_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::default(),
            Some(DownlevelFlags::COMPUTE_SHADERS),
            Some(Backend::Vulkan),
            true,
            true,
            true,
        );
        assert!(capabilities.compute, "{capabilities:?}");
        assert!(capabilities.storage_buffers);
        assert_eq!(capabilities.max_compute_workgroup_size[0], 256);
        assert!(capabilities.compute_unavailable_reason().is_none());
        assert!(capabilities.hdr_usable());
        assert!(capabilities.hdr_unavailable_reason().is_none());
    }

    /// 两条证据缺一不可：downlevel 置位但限制全 0，仍然不可用。
    #[test]
    fn flag_and_limits_are_both_required() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            Some(DownlevelFlags::COMPUTE_SHADERS),
            Some(Backend::Gl),
            false,
            false,
            false,
        );
        assert!(!capabilities.compute);
    }

    /// HDR 的两条证据也缺一不可：能渲染不能过滤、或反过来，都必须判 `hdr_usable=false`，
    /// 且原因里点名是哪一条缺。
    #[test]
    fn hdr_requires_render_and_linear_filter() {
        assert!(!external_caps(true, false).hdr_usable());
        assert!(!external_caps(false, true).hdr_usable());
        assert!(external_caps(true, true).hdr_usable());

        let no_filter = external_caps(true, false)
            .hdr_unavailable_reason()
            .expect("缺过滤能力必须给出原因");
        assert!(no_filter.contains("线性过滤"), "{no_filter}");
        let no_render = external_caps(false, true)
            .hdr_unavailable_reason()
            .expect("缺渲染能力必须给出原因");
        assert!(no_render.contains("渲染目标"), "{no_render}");
    }

    /// f16 解码器必须正确处理 8.0/0.5 这两个探针与 tonemap 读回会用到的值。
    #[test]
    fn f16_decoding_matches_known_values() {
        assert_eq!(f16_bits_to_f32(0x4800), 8.0);
        assert_eq!(f16_bits_to_f32(0x3800), 0.5);
        assert_eq!(f16_bits_to_f32(0x0000), 0.0);
        assert_eq!(f16_bits_to_f32(0x3C00), 1.0);
    }
}
