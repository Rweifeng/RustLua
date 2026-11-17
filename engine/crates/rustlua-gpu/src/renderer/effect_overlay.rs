//! 自定义特效叠加 pass（Shader Graph 的运行时入口）。
//!
//! 为什么是"叠加"而不是"改材质"：整条管线里唯一**接口稳定、和材质/网格无关**的位置是
//! 最后一帧的全屏叠加层。自定义 WGSL 只要满足下面这份固定契约，就能在这里画出东西：
//!
//! ```wgsl
//! struct EffectParams {
//!     time: f32,       // 秒；宿主每帧写
//!     width: f32,      // 目标宽（像素）
//!     height: f32,     // 目标高（像素）
//!     _pad0: f32,
//!     tint: vec4<f32>, // 预留：宿主可传的着色/强度
//! };
//! @group(0) @binding(0) var<uniform> effect: EffectParams;
//!
//! struct EffectOut {
//!     @builtin(position) position: vec4<f32>,
//!     @location(0) uv: vec2<f32>,
//! };
//! @vertex fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut { ... }
//! @fragment fn fs_main(input: EffectOut) -> @location(0) vec4<f32> { ... }
//! ```
//!
//! 约束与取舍：
//! - 默认关闭（`None`）⇒ 渲染路径与以前逐字节相同，像素门不受影响；
//! - 只读 `time/宽高/tint`，**不采样场景颜色**（那是后处理的职责）；要"叠在画面上"用
//!   alpha 混合，要"改画面"得等后续把场景颜色也接进来；
//! - 编译失败返回错误并保留旧管线，不会把画面整块弄黑。

use std::fmt;

/// 叠加层 uniform 的字节数（4 个 f32 + 一个 vec4）。
const EFFECT_UNIFORM_BYTES: u64 = 32;
/// Shader Graph 参数表 uniform 的字节数（16 个 vec4）。
const GRAPH_PARAM_BYTES: u64 = 16 * 16;

/// 自定义特效叠加的编译/布局错误。
#[derive(Debug)]
pub enum EffectOverlayError {
    /// WGSL 编译或管线校验失败（含 wgpu 的原始诊断）。
    Compile(String),
}

impl fmt::Display for EffectOverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(detail) => write!(formatter, "特效叠加着色器不可用：{detail}"),
        }
    }
}

impl std::error::Error for EffectOverlayError {}

/// 一台已编译好的自定义特效叠加层。
pub(crate) struct EffectOverlay {
    pipeline: wgpu::RenderPipeline,
    /// 每帧重建：场景色视图随帧/尺寸变化，不能只建一次。
    bind_group: Option<wgpu::BindGroup>,
    device: wgpu::Device,
    layout: wgpu::BindGroupLayout,
    scene_sampler: wgpu::Sampler,
    _white_view: wgpu::TextureView,
    /// 不采样场景/深度的叠加层走"直接叠最终目标"快路径，需要一张占位深度满足布局。
    dummy_depth_view: wgpu::TextureView,
    _dummy_depth: wgpu::Texture,
    /// 着色器源码是否引用场景色/深度：决定要不要走离屏 + 回拷。
    needs_scene: bool,
    params_buffer: wgpu::Buffer,
    /// Shader Graph 参数表（`binding 3`，16 个 `vec4`）。改参数只重写这块 buffer。
    graph_params_buffer: wgpu::Buffer,
    /// 1×1 白色占位纹理：Shader Graph 生成的片段固定在 `binding 1/2` 采样它，
    /// 没有纹理输入的模板也能原样通过管线校验。
    _white_texture: wgpu::Texture,
    /// 宿主写入的时间（秒）；绘制时连同目标尺寸与参数槽一起上传。
    time: f32,
    /// Shader Graph 的可调参数（16 个 vec4 槽）；`set_params` 改这里，不重编管线。
    param_slots: [[f32; 4]; 16],
}

impl EffectOverlay {
    /// 用宿主给的 WGSL 源建管线。契约见模块注释；`format` 必须与最终绘制目标一致。
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        source: &str,
    ) -> Result<Self, EffectOverlayError> {
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-effect-overlay-params"),
            size: EFFECT_UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let graph_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-effect-overlay-graph-params"),
            size: GRAPH_PARAM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rustlua-effect-overlay-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(EFFECT_UNIFORM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 3：Shader Graph 的参数表（16 个 vec4）。改参数只重写它。
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(GRAPH_PARAM_BYTES),
                    },
                    count: None,
                },
                // binding 4/5：场景颜色（M91：离屏渲染后回拷，叠加层可采样"画面"）。
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 6/7：深度（M92：描边/雾/软粒子）。深度纹理用普通采样器取 0..1。
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // Shader Graph 生成物的纹理/采样器固定在 1/2；这里给一张 1×1 白贴图，
        // 没有纹理输入的模板也用不上，但统一绑定让两边的编号规则只有一套。
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-effect-overlay-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // 1×1 白贴图必须**真的写成白色**：默认内容是 0（透明黑），采样类模板会全黑。
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &white_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dummy_depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-effect-overlay-dummy-depth"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_depth_view = dummy_depth.create_view(&wgpu::TextureViewDescriptor::default());
        {
            // 深度格式不能 write_texture，用一次清屏 pass 写成远平面。
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-effect-overlay-dummy-depth-clear"),
            });
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rustlua-effect-overlay-dummy-depth-clear-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dummy_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            queue.submit([encoder.finish()]);
        }
        // 结构性判据：着色器只要声明了 binding 4（场景色）或 6（深度），就必须走离屏路径。
        // 生成器只在图里真的用到对应节点时才emit这些声明，纯程序化模板因此能走快路径。
        let needs_scene = source.contains("@binding(4)") || source.contains("@binding(6)");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rustlua-effect-overlay-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 绑定组每帧在 prepare 里重建（含场景色 4/5 与深度 6/7），这里不预建。
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rustlua-effect-overlay-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rustlua-effect-overlay-module"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rustlua-effect-overlay-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(EffectOverlayError::Compile(error.to_string()));
        }
        Ok(Self {
            pipeline,
            bind_group: None,
            device: device.clone(),
            layout: layout.clone(),
            scene_sampler: sampler.clone(),
            _white_view: white_view,
            dummy_depth_view,
            _dummy_depth: dummy_depth,
            needs_scene,
            params_buffer: params,
            graph_params_buffer: graph_params,
            _white_texture: white_texture,
            time: 0.0,
            param_slots: [[0.0; 4]; 16],
        })
    }

    /// 这层是否依赖场景色/深度（决定要不要离屏渲染 + 回拷）。
    pub(crate) fn needs_scene(&self) -> bool {
        self.needs_scene
    }

    pub(crate) fn set_time(&mut self, time: f32) {
        self.time = time;
    }

    /// 写入 Shader Graph 的参数槽（不足 16 个的部分保持 0）。
    pub(crate) fn set_params(&mut self, params: &[[f32; 4]]) {
        for (slot, value) in self.param_slots.iter_mut().zip(params.iter()) {
            *slot = *value;
        }
    }

    /// 每帧准备：上传时间/参数，并用**本帧场景色视图**重建绑定组（`binding 4/5`）。
    pub(crate) fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        scene: Option<(&wgpu::TextureView, &wgpu::TextureView)>,
    ) {
        let (scene_view, depth_view) = match scene {
            Some((scene, depth)) => (scene, depth),
            // 不采样场景/深度的模板：绑占位，管线仍完整但走快路径。
            None => (&self._white_view, &self.dummy_depth_view),
        };
        self.upload_params(queue, width, height);
        self.bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-effect-overlay-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self._white_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.scene_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.graph_params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(scene_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.scene_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.scene_sampler),
                },
            ],
        }));
    }
    pub(crate) fn upload_params(&self, queue: &wgpu::Queue, width: u32, height: u32) {
        let bytes: Vec<u8> = [
            self.time,
            width as f32,
            height as f32,
            0.0,
            1.0,
            1.0,
            1.0,
            1.0,
        ]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
        queue.write_buffer(&self.params_buffer, 0, &bytes);
        let graph_bytes: Vec<u8> = self
            .param_slots
            .iter()
            .flat_map(|slot| slot.iter().flat_map(|value| value.to_le_bytes()))
            .collect();
        queue.write_buffer(&self.graph_params_buffer, 0, &graph_bytes);
    }

    /// 在 `pass` 上画出整屏叠加三角（调用方负责 `Load` 语义与视口）。
    pub(crate) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        if let Some(bind_group) = &self.bind_group {
            pass.set_bind_group(0, bind_group, &[]);
        }
        pass.draw(0..3, 0..1);
    }
}
