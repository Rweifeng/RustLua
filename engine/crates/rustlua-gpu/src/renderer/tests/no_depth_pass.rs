//! 诊断门（M100 追查）：**不挂深度附件的 pass 能不能真的画出像素**、以及
//! **深度纹理能不能在同一个不带深度附件的 pass 里被采样**。
//!
//! 这两件事是「世界 Sprite 软边」方案的前提。上一次尝试里像素门显示 sprite 完全没出现，
//! 但当时无法区分是「管线没出像素」还是「软边逻辑算错」。这里用两个最小用例把前提单独钉住：
//!
//! ① 纯色三角形 + 无深度附件的 pass → 必须出像素；
//! ② 深度纹理（先清成远平面 1.0）在无附件 pass 里 `textureLoad` 出来当颜色 → 必须出像素，
//!    且值 ≈ 1.0（证明「GPU 粒子那套写法」在本引擎的目标格式下可用）。
//!
//! 两个用例都是**最小装置**：不依赖 `Renderer`、不依赖世界 Sprite 管线，失败即说明前提不成立。

use super::*;

fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-no-depth-pass-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .ok()
}

fn read_center(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    size: u32,
) -> [u8; 4] {
    let bytes_per_row = (size * 4).max(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-no-depth-pass-readback"),
        size: u64::from(bytes_per_row) * u64::from(size),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rustlua-no-depth-pass-copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let data = buffer.slice(..).get_mapped_range();
    let row = (size / 2) as usize;
    let col = (size / 2) as usize;
    let index = row * bytes_per_row as usize + col * 4;
    let pixel = [
        data[index],
        data[index + 1],
        data[index + 2],
        data[index + 3],
    ];
    drop(data);
    buffer.unmap();
    pixel
}

fn create_target(device: &wgpu::Device, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-no-depth-pass-target"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

const SOLID_TRIANGLE: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // 覆盖全屏的两个三角形用 3 个顶点（超大三角形即可）：这里用常见技巧。
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.5, 1.0);
}
@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;

const DEPTH_AS_COLOR: &str = r#"
@group(0) @binding(0) var scene_depth: texture_depth_2d;
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.5, 1.0);
}
@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(scene_depth));
    let coord = clamp(vec2<i32>(frag.xy), vec2<i32>(0), dims - vec2<i32>(1));
    let depth = textureLoad(scene_depth, coord, 0);
    return vec4<f32>(vec3<f32>(depth), 1.0);
}
"#;

/// ① 无深度附件的 pass：纯色三角形必须出像素。
#[test]
fn quad_renders_without_a_depth_attachment() {
    const SIZE: u32 = 16;
    let Some((device, queue)) = headless_device() else {
        return gpu_skipped("request_adapter");
    };
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-no-depth-solid"),
        source: wgpu::ShaderSource::Wgsl(SOLID_TRIANGLE.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-no-depth-solid-layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rustlua-no-depth-solid-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    let target = create_target(&device, SIZE);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rustlua-no-depth-solid-passes"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-no-depth-solid-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    queue.submit([encoder.finish()]);
    let pixel = read_center(&device, &queue, &target, SIZE);
    assert_eq!(
        pixel,
        [255, 0, 0, 255],
        "无深度附件的 pass 必须出像素：{pixel:?}"
    );
}

/// ② 深度纹理在无深度附件的 pass 里 `textureLoad`：能出像素、值≈远平面。
#[test]
fn depth_texture_is_readable_in_a_pass_without_a_depth_attachment() {
    const SIZE: u32 = 16;
    let Some((device, queue)) = headless_device() else {
        return gpu_skipped("request_adapter");
    };
    // 1×1 深度纹理，先清成远平面 1.0（深度格式不能用 write_texture，走清屏 pass）。
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-no-depth-source"),
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
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rustlua-no-depth-clear"),
        });
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-no-depth-clear-pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
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

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("rustlua-no-depth-sample"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_AS_COLOR.into()),
    });
    let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rustlua-no-depth-sample-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("rustlua-no-depth-sample-bind"),
        layout: &bind_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&depth_view),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("rustlua-no-depth-sample-pipeline-layout"),
        bind_group_layouts: &[&bind_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rustlua-no-depth-sample-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    let target = create_target(&device, SIZE);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("rustlua-no-depth-sample-passes"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rustlua-no-depth-sample-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    queue.submit([encoder.finish()]);
    let pixel = read_center(&device, &queue, &target, SIZE);
    assert!(
        pixel[0] > 200 && pixel[0] == pixel[1] && pixel[1] == pixel[2],
        "深度纹理应能读出来当颜色（≈远平面白）：{pixel:?}"
    );
}
