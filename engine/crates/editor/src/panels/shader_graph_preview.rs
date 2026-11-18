//! Shader Graph 的离屏预览器：把生成的片段包装成完整全屏着色器，真实渲染并回读像素。
//!
//! 两个消费者共用同一台实现：
//! - 编辑器面板：每帧渲染一张小图上传成 egui 纹理，作者改模板/参数后立刻看到画面；
//! - 测试：用 fallback adapter 跑同样的管线，把"能编译"升级成"能画出可复现的像素"。

/// 编辑器和测试共用的预览尺寸（16:10，够看清效果又不拖慢逐帧回读）。
pub(crate) const PREVIEW_W: u32 = 256;
pub(crate) const PREVIEW_H: u32 = 160;

/// 把生成的 Shader Graph 片段包装成完整全屏 WGSL 着色器。
///
/// 统一绑定：`@group(0) @binding(0) texture`、`(1) sampler`、`(2) uniform { time, intensity }`。
/// `time` 走 uniform 而不是写死 0，预览才能看到会动的模板；`intensity` 是面板「效果强度」
/// 滑块的总强度：同时缩放 RGB 与 alpha，置 0 即完全关掉效果（不是画一层黑）。
pub(crate) fn preview_shader_source(graph_wgsl: &str) -> String {
    format!(
        r#"{graph_wgsl}
struct PreviewParams {{
    time: f32,
    intensity: f32,
    _pad: vec2<f32>,
    _tail: vec4<f32>,
}};
@group(0) @binding(2) var<uniform> preview: PreviewParams;
struct VertexOutput {{
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}};
@vertex
fn vertex_main(@builtin(vertex_index) index: u32) -> VertexOutput {{
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: VertexOutput;
    let position = positions[index];
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = position * 0.5 + vec2<f32>(0.5);
    output.color = vec4<f32>(1.0);
    return output;
}}
@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {{
    let color = shader_graph_color(input.uv, preview.time, input.color);
    return vec4<f32>(
        color.rgb * preview.intensity,
        clamp(color.a * preview.intensity, 0.0, 1.0)
    );
}}
"#
    )
}

/// 一台可复用的离屏预览器：布局/纹理/采样器只建一次，换着色器只重编管线。
pub(crate) struct ShaderPreview {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    /// 棋盘输入纹理：采样类模板要有可预期内容，预览才不是"看运气"。
    _source_texture: wgpu::Texture,
    source_view: wgpu::TextureView,
    /// M92：深度占位（1×1 远平面），让 SceneDepth 类模板也能预览。
    _depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    bytes_per_row: u32,
    bind_group_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    graph_params: wgpu::Buffer,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: Option<wgpu::RenderPipeline>,
}

impl ShaderPreview {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shader-graph-preview-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let source_size: u32 = 8;
        let mut pixels = Vec::with_capacity((source_size * source_size * 4) as usize);
        for y in 0..source_size {
            for x in 0..source_size {
                let even = (x + y) % 2 == 0;
                pixels.extend_from_slice(if even {
                    &[230u8, 40, 40, 255]
                } else {
                    &[30u8, 60, 230, 255]
                });
            }
        }
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-graph-preview-source"),
            size: wgpu::Extent3d {
                width: source_size,
                height: source_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(source_size * 4),
                rows_per_image: Some(source_size),
            },
            wgpu::Extent3d {
                width: source_size,
                height: source_size,
                depth_or_array_layers: 1,
            },
        );
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-graph-preview-depth"),
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
        // 深度格式不能用 write_texture 直接拷（wgpu 禁止），用一次清屏 pass 写成远平面 1.0。
        {
            let view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shader-graph-preview-depth-clear"),
            });
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shader-graph-preview-depth-clear-pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
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
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-graph-preview-target"),
            size: wgpu::Extent3d {
                width: PREVIEW_W,
                height: PREVIEW_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let row_bytes = 4 * PREVIEW_W;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-graph-preview-readback"),
            size: u64::from(bytes_per_row * PREVIEW_H),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader-graph-preview-bind-layout"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // M91：场景颜色（预览用棋盘占位，与运行时同编号 4/5）。
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
                // binding 3：Shader Graph 的参数槽（16 个 vec4 = 256 字节）。
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(wgpu::BufferSize::new(256).expect("nonzero")),
                    },
                    count: None,
                },
                // M92：深度（预览用 1×1 远平面占位）。
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
        // `struct { time: f32, _pad: vec3<f32> }` 的 WGSL 大小是 32（vec3 对齐 16）。
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-graph-preview-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let graph_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-graph-preview-graph-params"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader-graph-preview-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            sampler,
            _source_texture: source_texture,
            source_view,
            _depth_texture: depth_texture,
            depth_view,
            target,
            target_view,
            readback,
            bytes_per_row,
            bind_group_layout,
            params,
            graph_params,
            pipeline_layout,
            pipeline: None,
        }
    }

    /// 写参数槽（16 个 `vec4`）。改参数只重写这块 uniform，不重编管线。
    pub(crate) fn set_params(&mut self, slots: &[[f32; 4]; 16]) {
        let bytes: Vec<u8> = slots
            .iter()
            .flat_map(|slot| slot.iter().flat_map(|value| value.to_le_bytes()))
            .collect();
        self.queue.write_buffer(&self.graph_params, 0, &bytes);
    }

    /// 换一份完整 WGSL 源（`preview_shader_source` 的输出）。编译失败返回中文错误，
    /// 调用方据此在面板上显示诊断，而不是整块预览黑掉。
    pub(crate) fn set_shader(&mut self, source: &str) -> Result<(), String> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("shader-graph-preview-module"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("shader-graph-preview-pipeline"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vertex_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fragment_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            });
        let validation =
            pollster::block_on(self.device.pop_error_scope()).map(|error| error.to_string());
        match validation {
            Some(error) => {
                self.pipeline = None;
                Err(format!("WGSL 编译/管线校验失败：{error}"))
            }
            None => {
                self.pipeline = Some(pipeline);
                Ok(())
            }
        }
    }

    /// 以 `time` / `intensity` 渲染一帧并同步回读紧凑 RGBA8（`PREVIEW_W * PREVIEW_H * 4` 字节）。
    /// `intensity` 与 `time` 同属 binding 2 的 uniform：改它只写 32 字节 buffer，不重编管线。
    pub(crate) fn render(&mut self, time: f32, intensity: f32) -> Result<Vec<u8>, String> {
        let Some(pipeline) = &self.pipeline else {
            return Err("预览管线尚未编译".to_owned());
        };
        let params_bytes: Vec<u8> = [time, intensity, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.queue.write_buffer(&self.params, 0, &params_bytes);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader-graph-preview-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.graph_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&self.depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shader-graph-preview-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shader-graph-preview-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let row_bytes = 4 * PREVIEW_W;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: Some(PREVIEW_H),
                },
            },
            wgpu::Extent3d {
                width: PREVIEW_W,
                height: PREVIEW_H,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let map_ok = rx.recv().ok().and_then(Result::ok).is_some();
        let mut pixels = Vec::with_capacity((row_bytes * PREVIEW_H) as usize);
        if map_ok {
            let padded = slice.get_mapped_range();
            for row in 0..PREVIEW_H as usize {
                let start = row * self.bytes_per_row as usize;
                pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
            }
        }
        // 复用同一块回读 buffer：不 unmapped 下一次 `map_async` 会报 "Buffer is already mapped"
        // （预览每帧调用一次，画廊连续渲染 27 张时必然踩到）。
        self.readback.unmap();
        let validation =
            pollster::block_on(self.device.pop_error_scope()).map(|error| error.to_string());
        if let Some(error) = validation {
            return Err(format!("渲染校验失败：{error}"));
        }
        if pixels.is_empty() {
            return Err("像素回读失败（映射未完成）".to_owned());
        }
        Ok(pixels)
    }
}

/// 把生成的 Shader Graph 片段包装成**运行时叠加层** WGSL，并返回参数槽位表。
///
/// 契约与 `rustlua-gpu` 的 `renderer::effect_overlay` 模块一致：`binding 0` 是本层 uniform，
/// 图的纹理/采样器因此绑到 `1/2`；参数表固定在 `binding 3`（与预览一致）。
/// 参数走 uniform ⇒ 改参数只需 `set_effect_params`，不必重编管线。
pub(crate) fn runtime_overlay_source(
    graph: &rustlua_visual_script::shader_graph::ShaderGraph,
) -> Result<(String, rustlua_visual_script::shader_graph::CompiledShader), String> {
    let compiled = graph
        .compile_wgsl_uniform(1, 3)
        .map_err(|error| error.to_string())?;
    let body = &compiled.source;
    let source = format!(
        r#"{body}
struct EffectParams {{
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
}};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct EffectOut {{
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {{
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {{
    return shader_graph_color(input.uv, effect.time, vec4<f32>(1.0));
}}
"#
    );
    Ok((source, compiled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_visual_script::shader_graph::{ShaderGraph, ShaderTemplate};

    struct TestGpu {
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    fn test_gpu() -> Option<TestGpu> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("shader-graph-preview-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()?;
        Some(TestGpu { device, queue })
    }

    fn render_template(gpu: &TestGpu, wgsl: &str, time: f32) -> Result<Vec<u8>, String> {
        let mut preview = ShaderPreview::new(&gpu.device, &gpu.queue);
        preview.set_shader(&preview_shader_source(wgsl))?;
        preview.render(time, 1.0)
    }

    /// 用 uniform 参数版源码渲染一张图，并把图上的参数写进槽位。
    fn render_graph(
        gpu: &TestGpu,
        graph: &rustlua_visual_script::shader_graph::ShaderGraph,
        time: f32,
    ) -> Result<Vec<u8>, String> {
        let compiled = graph
            .compile_wgsl_uniform(0, 3)
            .map_err(|error| error.to_string())?;
        let mut preview = ShaderPreview::new(&gpu.device, &gpu.queue);
        preview.set_shader(&preview_shader_source(&compiled.source))?;
        let values = graph.param_values(&compiled);
        let mut slots = [[0.0_f32; 4]; 16];
        for (slot, value) in slots.iter_mut().zip(values.iter()) {
            *slot = *value;
        }
        preview.set_params(&slots);
        preview.render(time, 1.0)
    }

    /// 每个模板都要能真渲染出非全透明的一帧，且同一输入两次渲染逐字节一致。
    #[test]
    fn every_template_renders_deterministic_non_empty_pixels() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph pixel preview: no fallback adapter");
            return;
        };
        for template in ShaderTemplate::ALL {
            let name = template.name_zh();
            let wgsl = template.graph().compile_wgsl().expect(name);
            let first = render_template(&gpu, &wgsl, 0.25)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                first.len(),
                (PREVIEW_W * PREVIEW_H * 4) as usize,
                "{name}: 回读像素数量不对"
            );
            // 深度描边只依赖深度差：预览的深度占位是均匀远平面，没有轮廓 ⇒ 全透明是**正确结果**。
            // 其它模板必须真的输出像素，防止"画了个透明的东西"也算通过。
            if !matches!(template, ShaderTemplate::DepthOutline) {
                assert!(
                    first.chunks(4).any(|pixel| pixel[3] > 0),
                    "{name}: 渲染结果全透明，说明片元没有真正输出"
                );
            }
            let second = render_template(&gpu, &wgsl, 0.25)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(first, second, "{name}: 相同输入必须渲染出相同像素");
        }
    }

    /// 至少要有模板能画出「不止一种颜色」，否则只能证明清了屏。
    #[test]
    fn preview_pixels_show_varied_colors() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph pixel preview: no fallback adapter");
            return;
        };
        let mut varied = 0;
        for template in [
            ShaderTemplate::Fire,
            ShaderTemplate::Scanlines,
            ShaderTemplate::Hologram,
        ] {
            let name = template.name_zh();
            let wgsl = template.graph().compile_wgsl().expect(name);
            let pixels =
                render_template(&gpu, &wgsl, 0.5).unwrap_or_else(|error| panic!("{name}: {error}"));
            let mut unique = std::collections::BTreeSet::new();
            for pixel in pixels.chunks(4) {
                unique.insert([pixel[0], pixel[1], pixel[2], pixel[3]]);
            }
            if unique.len() >= 8 {
                varied += 1;
            }
        }
        assert!(
            varied >= 2,
            "纹理/波形类模板应当画出多种颜色，实测只有 {varied} 个模板达标"
        );
    }

    /// 用时间的模板必须随时间变化：这条把"动画参数真的接进 shader"钉成回归门。
    #[test]
    fn animated_templates_change_with_time() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph pixel preview: no fallback adapter");
            return;
        };
        for template in [
            ShaderTemplate::Scanlines,
            ShaderTemplate::Fire,
            ShaderTemplate::SpeedLines,
        ] {
            let name = template.name_zh();
            let wgsl = template.graph().compile_wgsl().expect(name);
            let zero =
                render_template(&gpu, &wgsl, 0.0).unwrap_or_else(|error| panic!("{name}: {error}"));
            let half =
                render_template(&gpu, &wgsl, 0.5).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_ne!(zero, half, "{name}: 时间 uniform 没有影响输出");
        }
    }

    /// 纹理采样必须真的从绑定纹理取色：棋盘输入 → 输出不应是单一颜色。
    #[test]
    fn texture_sampling_reads_the_bound_texture() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph pixel preview: no fallback adapter");
            return;
        };
        let graph: ShaderGraph = {
            use rustlua_visual_script::shader_graph::{
                ShaderEdge, ShaderNode, ShaderNodeKind, ShaderPort,
            };
            let node = |id: &str, kind| ShaderNode {
                id: id.into(),
                kind,
                position: [0.0, 0.0],
                params: Default::default(),
            };
            let edge = |a: &str, ap: &str, b: &str, bp: &str| ShaderEdge {
                from: ShaderPort {
                    node: a.into(),
                    port: ap.into(),
                },
                to: ShaderPort {
                    node: b.into(),
                    port: bp.into(),
                },
            };
            ShaderGraph {
                version: 1,
                name: "checker".into(),
                nodes: vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    node("out", ShaderNodeKind::Output),
                ],
                edges: vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "out", "tint"),
                ],
            }
        };
        let wgsl = graph.compile_wgsl().expect("checker graph");
        let pixels = render_template(&gpu, &wgsl, 0.0).expect("checker render");
        let mut unique = std::collections::BTreeSet::new();
        for pixel in pixels.chunks(4) {
            unique.insert([pixel[0], pixel[1], pixel[2], pixel[3]]);
        }
        assert!(
            unique.len() >= 2,
            "棋盘纹理应当采样出至少两种颜色，实测 {}",
            unique.len()
        );
    }

    /// 坏 shader 必须返回中文错误而不是让编辑器 panic（面板据此显示诊断）。
    #[test]
    fn invalid_shader_is_reported_instead_of_panicking() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph pixel preview: no fallback adapter");
            return;
        };
        let mut preview = ShaderPreview::new(&gpu.device, &gpu.queue);
        let error = preview
            .set_shader("fn broken( -> vec4<f32> { return vec4<f32>(1.0); }")
            .expect_err("坏 WGSL 必须报错");
        assert!(error.contains("WGSL"), "错误信息应说明是 WGSL：{error}");
        assert!(preview.render(0.0, 1.0).is_err(), "坏管线不能渲染");
    }

    fn empty_scene_mesh() -> rustlua_render::frame::SceneMesh {
        rustlua_render::frame::SceneMesh {
            vertices: Vec::new(),
            draws: Vec::new(),
            world_sprites: Vec::new(),
            world_sprite_draws: Vec::new(),
            paths: Vec::new(),
            path_handles: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        }
    }

    /// 编辑器生成 → 运行时渲染器消费：这是"面板里调好的效果能真的画出来"的端到端契约。
    ///
    /// 覆盖三件事：生成的叠加层 WGSL 能被 `Renderer::set_effect_overlay` 接受（绑定编号与
    /// uniform 契约一致）、开档后像素真的变了、关档后像素回到基线。
    #[test]
    fn generated_overlay_source_changes_real_renderer_pixels() {
        use rustlua_visual_script::shader_graph::ShaderTemplate;

        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph runtime overlay: no fallback adapter");
            return;
        };
        let mut renderer = rustlua_gpu::Renderer::new_offscreen(
            gpu.device.clone(),
            gpu.queue.clone(),
            None,
            false,
            false,
        )
        .expect("offscreen renderer");
        let render = |renderer: &mut rustlua_gpu::Renderer| {
            let (width, height) = (32u32, 32u32);
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("shader-graph-overlay-target"),
                size: wgpu::Extent3d {
                    width,
                    height,
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
            renderer.render_into_texture(&empty_scene_mesh(), None, &view, width, height, true);
            renderer
                .copy_texture_to_cpu(&texture, width, height)
                .expect("readback")
        };

        let baseline = render(&mut renderer);
        // 火焰流动是纹理采样模板：同时验证 binding 1/2 的重映射是对的。
        let (source, compiled) =
            runtime_overlay_source(&ShaderTemplate::Fire.graph()).expect("overlay source");
        renderer
            .set_effect_overlay(Some(&source))
            .expect("运行时渲染器必须接受编辑器生成的叠加层");
        renderer.set_effect_time(0.5);
        let slots: Vec<[f32; 4]> = compiled.slots.clone();
        renderer.set_effect_params(&slots);
        let painted = render(&mut renderer);
        assert_ne!(baseline, painted, "叠加层必须真的改变渲染器输出像素");

        // 参数走 uniform：换一组参数（不重编管线）画面随之变化。
        let mut shifted = compiled.slots.clone();
        shifted[0] = [1.0, 0.0, 0.0, 0.0];
        renderer.set_effect_params(&shifted);
        renderer.set_effect_time(0.5);
        let repainted = render(&mut renderer);
        assert_ne!(painted, repainted, "参数 uniform 必须影响输出");

        renderer.set_effect_overlay(None).expect("disable overlay");
        let restored = render(&mut renderer);
        assert_eq!(baseline, restored, "关档后像素必须回到基线");
    }

    /// Inspector 改参数 → 只重写 uniform，不重编管线：源码不变、像素变。
    #[test]
    fn parameter_edits_change_pixels_without_recompiling() {
        use rustlua_visual_script::shader_graph::{ShaderNodeKind, ShaderTemplate};

        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph params: no fallback adapter");
            return;
        };
        let mut graph = ShaderTemplate::Scanlines.graph();
        let before = render_graph(&gpu, &graph, 0.3).expect("first render");
        let compiled = graph.compile_wgsl_uniform(0, 3).expect("compile");

        // 把扫描线的频率从 140 改成 3：同一份源码，只换 uniform 值。
        if let Some(node) = graph
            .nodes
            .iter_mut()
            .find(|node| node.kind == ShaderNodeKind::Sine)
        {
            node.params.insert("frequency".into(), 3.0);
        }
        let after = render_graph(&gpu, &graph, 0.3).expect("second render");
        assert_ne!(before, after, "改参数必须改变像素");

        // 源码与槽位表都不变（没有重编），只是参数值变了。
        let recompiled = graph.compile_wgsl_uniform(0, 3).expect("recompile");
        assert_eq!(
            compiled.source, recompiled.source,
            "改参数不应改变生成的 WGSL"
        );
        let values = graph.param_values(&compiled);
        let slot = compiled
            .params
            .iter()
            .find(|param| param.param == "frequency")
            .map(|param| param.slot)
            .expect("frequency slot");
        assert_eq!(values[slot][0], 3.0, "参数值应当写进槽位");
    }

    /// 「效果强度」是面板滑杆，但真正改变画面的是 binding 2 里的 uniform：
    /// 同一份源码、同一组图参数，只改强度也必须改变像素；0 表示完全透明（不是一层黑）。
    #[test]
    fn preview_intensity_scales_pixels_without_recompiling() {
        use rustlua_visual_script::shader_graph::ShaderTemplate;

        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph intensity: no fallback adapter");
            return;
        };
        let graph = ShaderTemplate::Fire.graph();
        let compiled = graph.compile_wgsl_uniform(0, 3).expect("compile");
        let mut preview = ShaderPreview::new(&gpu.device, &gpu.queue);
        preview
            .set_shader(&preview_shader_source(&compiled.source))
            .expect("shader");
        let mut slots = [[0.0_f32; 4]; 16];
        for (slot, value) in slots.iter_mut().zip(graph.param_values(&compiled).iter()) {
            *slot = *value;
        }
        preview.set_params(&slots);

        let full = preview.render(0.3, 1.0).expect("full intensity");
        let half = preview.render(0.3, 0.5).expect("half intensity");
        let zero = preview.render(0.3, 0.0).expect("zero intensity");

        assert!(full.iter().any(|channel| *channel != 0), "满强度应画出像素");
        assert_ne!(full, half, "只改强度 uniform 就必须改变画面");
        assert!(
            zero.iter().all(|channel| *channel == 0),
            "强度 0 应当完全透明，而不是画一层黑"
        );
    }

    /// 热浪/折射的扭曲真的改变了采样坐标：强度 0 与 0.05 渲染结果不同。
    #[test]
    fn distorted_scene_differs_from_undistorted_sample() {
        use rustlua_visual_script::shader_graph::ShaderTemplate;

        let Some(gpu) = test_gpu() else {
            eprintln!("skip Shader Graph distortion: no fallback adapter");
            return;
        };
        let graph = ShaderTemplate::HeatHaze.graph();
        let compiled = graph.compile_wgsl_uniform(0, 3).expect("compile");
        let mut preview = ShaderPreview::new(&gpu.device, &gpu.queue);
        preview
            .set_shader(&preview_shader_source(&compiled.source))
            .expect("pipeline");
        let mut slots = [[0.0_f32; 4]; 16];
        for (slot, value) in slots.iter_mut().zip(graph.param_values(&compiled).iter()) {
            *slot = *value;
        }
        let slot = compiled
            .params
            .iter()
            .find(|param| param.param == "strength")
            .map(|param| param.slot)
            .expect("strength slot");
        slots[slot] = [0.0, 0.0, 0.0, 0.0];
        preview.set_params(&slots);
        let flat = preview.render(0.3, 1.0).expect("flat render");
        slots[slot] = [0.05, 0.0, 0.0, 0.0];
        preview.set_params(&slots);
        let warped = preview.render(0.3, 1.0).expect("warped render");
        assert_ne!(
            flat, warped,
            "扭曲强度为 0 时应采样原画面，>0 时坐标必须偏移"
        );
    }

    /// 实战资产炎刃斩击：从磁盘读真实 `.sgraph`，走与运行时相同的编译+渲染路径。
    /// 证明它不是"能编译的空图"——像素非空、随时间变、改命名参数也变。
    #[test]
    fn flame_blade_slash_asset_renders_and_responds_to_tuning() {
        let Some(gpu) = test_gpu() else {
            eprintln!("skip 炎刃斩击像素门：no fallback adapter");
            return;
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../examples/game_api/assets/fx/flame_blade_slash.sgraph");
        let text = std::fs::read_to_string(&path).expect("读取炎刃斩击 .sgraph 失败");
        let graph = ShaderGraph::from_json(&text).expect("炎刃斩击 .sgraph 必须是合法图");
        graph.validate().expect("炎刃斩击图校验");
        let compiled = graph
            .compile_wgsl_uniform(0, 3)
            .expect("炎刃斩击必须能编译成 uniform 叠加层");

        let at_0 = render_graph(&gpu, &graph, 0.25).expect("t=0.25 渲染");
        assert_eq!(at_0.len(), (PREVIEW_W * PREVIEW_H * 4) as usize);
        assert!(
            at_0.chunks(4).any(|pixel| pixel[3] > 0),
            "炎刃斩击渲染结果全透明：刀光弧遮罩没有真正输出"
        );
        // 白热核心：刃口必须有一段高不透明 + 高亮的像素（RGB 全高），
        // 否则就是"一团暗红雾"——用户反馈的退化样貌，仅查 alpha>0 抓不住。
        let hot_pixels = at_0
            .chunks(4)
            .filter(|pixel| pixel[3] > 180 && pixel[0] > 200 && pixel[1] > 140 && pixel[2] > 90)
            .count();
        assert!(
            hot_pixels >= 12,
            "刀光弧缺少白热高亮核心：满足条件的像素只有 {hot_pixels} 个"
        );

        // 时间推进：刀光弧旋转 + 冲击波环扩散，画面必须变。
        let at_1 = render_graph(&gpu, &graph, 1.75).expect("t=1.75 渲染");
        assert_ne!(at_0, at_1, "改时间必须改变炎刃斩击画面");

        // 改命名参数 spin（刀光弧转速）后，同一时刻画面必须变。
        let mut tuned = graph.clone();
        if let Some(node) = tuned.nodes.iter_mut().find(|node| node.id == "blade") {
            node.params.insert("spin".into(), 5.0);
        }
        let retuned = render_graph(&gpu, &tuned, 0.25).expect("调 spin 后渲染");
        assert_ne!(at_0, retuned, "改 spin 参数必须改变刀光弧画面");

        // 门：命名参数表真的暴露了演示脚本逐帧调的那几个名字。
        let names: Vec<_> = compiled
            .named_params()
            .into_iter()
            .map(|param| param.name)
            .collect();
        for required in ["edge_width", "color_r", "frequency", "speed", "spin"] {
            assert!(
                names.iter().any(|name| name == required),
                "炎刃斩击应暴露 {required}，现有 {names:?}"
            );
        }
    }
}
