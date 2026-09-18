use bytemuck::{Pod, Zeroable};
use engine::core::{
    atlas::SpriteAtlas,
    camera::{Camera2D, Viewport},
    sprite::Sprite,
    sprite_batch::{SpriteBatch, SpriteBatchItem},
    texture::{TextureAsset, TextureColorSpace},
    transform::{Transform, Vec2},
};
use wgpu::util::DeviceExt;

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
    let colors = [
        [255, 32, 32, 255],
        [32, 255, 32, 255],
        [32, 32, 255, 255],
        [255, 220, 32, 255],
    ];
    let mut pixels = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            let color = colors[(y / 2 * 2 + x / 2) as usize];
            pixels.extend_from_slice(&color);
        }
    }
    let asset = TextureAsset::from_rgba8(4, 4, pixels, TextureColorSpace::Srgb)?;
    let mut atlas = SpriteAtlas::from_asset(&asset)?;
    atlas.insert("red", 0, 0, 2, 2)?;
    atlas.insert("green", 2, 0, 2, 2)?;
    atlas.insert("blue", 0, 2, 2, 2)?;
    atlas.insert("yellow", 2, 2, 2, 2)?;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| format!("no adapter available: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("m3-06-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m3-06-atlas"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
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
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    let mut batch = SpriteBatch::with_capacity(4);
    for (name, position) in [
        ("red", (-2.0, 2.0)),
        ("green", (2.0, 2.0)),
        ("blue", (-2.0, -2.0)),
        ("yellow", (2.0, -2.0)),
    ] {
        let sprite = atlas
            .get(name)
            .ok_or_else(|| format!("missing atlas region: {name}"))?
            .apply(Sprite::new(Vec2::new(3.0, 3.0))?)?;
        batch.push(SpriteBatchItem::new(
            0,
            0,
            sprite,
            Transform::from_position(Vec2::new(position.0, position.1)),
        ));
    }
    let mesh = batch.build(
        Camera2D::new(Vec2::zero(), 1.0)?,
        Viewport::new(128.0, 128.0)?,
    )?;
    let vertices: Vec<GpuVertex> = mesh
        .vertices
        .iter()
        .map(|v| GpuVertex {
            position: [v.position[0] / 64.0 - 1.0, 1.0 - v.position[1] / 64.0],
            uv: v.uv,
            color: v.color,
        })
        .collect();
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("m3-06-vertices"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("m3-06-layout"),
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
        label: Some("m3-06-bind"),
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
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("m3-06-shader"), source: wgpu::ShaderSource::Wgsl(r#"struct I{@location(0)p:vec2<f32>,@location(1)u:vec2<f32>,@location(2)c:vec4<f32>};struct O{@builtin(position)p:vec4<f32>,@location(0)u:vec2<f32>,@location(1)c:vec4<f32>};@group(0)@binding(0)var t:texture_2d<f32>;@group(0)@binding(1)var s:sampler;@vertex fn vs(i:I)->O{var o:O;o.p=vec4(i.p,0,1);o.u=i.u;o.c=i.c;return o;}@fragment fn fs(i:O)->@location(0)vec4<f32>{return textureSample(t,s,i.u)*i.c;}"#.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("m3-06-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("m3-06-pipeline"),
        layout: Some(&pipeline_layout),
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
        label: Some("m3-06-target"),
        size: wgpu::Extent3d {
            width: 128,
            height: 128,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("m3-06-encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m3-06-pass"),
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
    queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    })?;
    println!(
        "atlas_demo_ok adapter={:?} regions={} sprites={} vertices={} draw_calls={} texture=4x4 target=128x128",
        adapter.get_info().name,
        atlas.len(),
        mesh.sprite_count(),
        mesh.vertices.len(),
        mesh.draws.len()
    );
    Ok(())
}
