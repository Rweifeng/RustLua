use bytemuck::{Pod, Zeroable};
use engine_core::{
    math2d::{Camera2D, Viewport},
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    texture::{TextureAsset, TextureColorSpace},
    transform::{Transform, Vec2},
};
use wgpu::util::DeviceExt;
const PNG: &[u8] = include_bytes!("../../../../assets/m3-02/red-1x1.png");
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    pollster::block_on(run())
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let asset = TextureAsset::from_png_bytes(PNG, TextureColorSpace::Srgb)?;
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| format!("no adapter available: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("m3-04-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m3-04-texture"),
        size: wgpu::Extent3d {
            width: asset.width(),
            height: asset.height(),
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
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        asset.pixels(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(asset.bytes_per_row()),
            rows_per_image: Some(asset.height()),
        },
        wgpu::Extent3d {
            width: asset.width(),
            height: asset.height(),
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let camera = Camera2D::new(Vec2::zero(), 2.0)?;
    let viewport = Viewport::new(256.0, 256.0)?;
    let mut batch = SpriteBatch::with_capacity(1000);
    for index in 0..1000 {
        let x = (index % 40) as f32 - 19.5;
        let y = (index / 40) as f32 - 12.0;
        let layer = index % 3;
        batch.push(SpriteBatchItem::new(
            0,
            layer,
            Sprite::new(Vec2::new(0.8, 0.8))?.with_tint([
                1.0,
                0.35 + (index % 5) as f32 * 0.1,
                0.35,
                1.0,
            ])?,
            Transform::from_position(Vec2::new(x, y)),
        ));
    }
    let mesh = batch.build(camera, viewport)?;
    let vertices: Vec<GpuVertex> = mesh
        .vertices
        .iter()
        .map(|v| GpuVertex {
            position: [v.position[0] / 128.0 - 1.0, 1.0 - v.position[1] / 128.0],
            uv: v.uv,
            color: v.color,
        })
        .collect();
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("m3-04-vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("m3-04-layout"),
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
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("m3-04-bind"),
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
    let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("m3-04-shader"),source:wgpu::ShaderSource::Wgsl(r#"struct I{@location(0)p:vec2<f32>,@location(1)u:vec2<f32>,@location(2)c:vec4<f32>};struct O{@builtin(position)p:vec4<f32>,@location(0)u:vec2<f32>,@location(1)c:vec4<f32>};@group(0)@binding(0)var t:texture_2d<f32>;@group(0)@binding(1)var s:sampler;@vertex fn vs(i:I)->O{var o:O;o.p=vec4(i.p,0,1);o.u=i.u;o.c=i.c;return o;}@fragment fn fs(i:O)->@location(0)vec4<f32>{return textureSample(t,s,i.u)*i.c;}"#.into())});
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("m3-04-pl"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("m3-04-pipeline"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Float32x4],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m3-04-target"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("m3-04-encoder"),
    });
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m3-04-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
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
        pass.set_vertex_buffer(0, vb.slice(..));
        for draw in &mesh.draws {
            pass.draw(
                draw.vertex_start as u32..(draw.vertex_start + draw.vertex_count) as u32,
                0..1,
            );
        }
    }
    queue.submit([enc.finish()]);
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    })?;
    println!(
        "sprite_batch_demo_ok adapter={:?} sprites={} vertices={} draw_calls={} texture={}x{} target=256x256",
        adapter.get_info().name,
        mesh.sprite_count(),
        mesh.vertices.len(),
        mesh.draws.len(),
        asset.width(),
        asset.height()
    );
    Ok(())
}
