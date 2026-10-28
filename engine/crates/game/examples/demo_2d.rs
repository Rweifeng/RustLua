use bytemuck::{Pod, Zeroable};
use rustlua_camera::camera::{Camera2D, Viewport};
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_render::atlas::SpriteAtlas;
use rustlua_render::sprite::Sprite;
use rustlua_render::sprite_batch::{SpriteBatch, SpriteBatchItem};
use rustlua_render::text::{TextAtlas, TextLayout, TextStyle};
use rustlua_render::texture::{TextureAsset, TextureColorSpace};
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
    let (sprite_asset, atlas) = sprite_atlas()?;
    let font_atlas = TextAtlas::built_in();
    let font_asset = TextureAsset::from_rgba8(
        font_atlas.width(),
        font_atlas.height(),
        font_atlas.pixels().to_vec(),
        TextureColorSpace::Linear,
    )?;
    let mut sprite_batch = SpriteBatch::with_capacity(8);
    for (index, (name, position)) in [
        ("red", (-4.5, 1.0)),
        ("green", (-1.5, 1.0)),
        ("blue", (1.5, 1.0)),
        ("yellow", (4.5, 1.0)),
        ("yellow", (-3.0, -3.0)),
        ("blue", (0.0, -3.0)),
        ("green", (3.0, -3.0)),
    ]
    .into_iter()
    .enumerate()
    {
        let region = atlas
            .get(name)
            .ok_or_else(|| format!("missing region {name}"))?;
        let sprite = region
            .apply(Sprite::new(Vec2::new(2.2, 2.2))?)?
            .with_tint([1.0, 1.0 - index as f32 * 0.04, 1.0, 1.0])?;
        sprite_batch.push(SpriteBatchItem::new(
            0,
            if index < 4 { 0 } else { 1 },
            sprite,
            Transform::from_position(Vec2::new(position.0, position.1)),
        ));
    }
    let viewport = Viewport::new(256.0, 256.0)?;
    let camera = Camera2D::new(Vec2::new(0.0, -0.5), 16.0)?;
    let sprite_mesh = sprite_batch.build(camera, viewport)?;
    let text_mesh = TextLayout::build_bitmap(
        "RUST LUA 2D DEMO\nATLAS + SPRITEBATCH",
        [16.0, 16.0],
        TextStyle {
            scale: 2.0,
            spacing: 1.0,
            line_spacing: 5.0,
            color: [0.85, 0.95, 1.0, 1.0],
            ..Default::default()
        },
        &font_atlas,
    )?;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| format!("no adapter available: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("m3-08-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await?;
    let sprite_texture = upload_texture(
        &device,
        &queue,
        &sprite_asset,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        "m3-08-sprite-atlas",
    );
    let font_texture = upload_texture(
        &device,
        &queue,
        &font_asset,
        wgpu::TextureFormat::Rgba8Unorm,
        "m3-08-font-atlas",
    );
    let sprite_view = sprite_texture.create_view(&Default::default());
    let font_view = font_texture.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("m3-08-texture-layout"),
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
    let sprite_bind = texture_bind(
        &device,
        &layout,
        &sprite_view,
        &sampler,
        "m3-08-sprite-bind",
    );
    let font_bind = texture_bind(&device, &layout, &font_view, &sampler, "m3-08-font-bind");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("m3-08-shader"), source: wgpu::ShaderSource::Wgsl(r#"struct I{@location(0)p:vec2<f32>,@location(1)u:vec2<f32>,@location(2)c:vec4<f32>};struct O{@builtin(position)p:vec4<f32>,@location(0)u:vec2<f32>,@location(1)c:vec4<f32>};@group(0)@binding(0)var t:texture_2d<f32>;@group(0)@binding(1)var s:sampler;@vertex fn vs(i:I)->O{var o:O;o.p=vec4(i.p,0,1);o.u=i.u;o.c=i.c;return o;}@fragment fn fs(i:O)->@location(0)vec4<f32>{return textureSample(t,s,i.u)*i.c;}"#.into()) });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("m3-08-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("m3-08-pipeline"),
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
    let sprite_vertices: Vec<GpuVertex> = sprite_mesh
        .vertices
        .iter()
        .map(|v| world_vertex(v.position, v.uv, v.color))
        .collect();
    let text_vertices: Vec<GpuVertex> = text_mesh
        .vertices
        .iter()
        .map(|v| screen_vertex(v.position, v.uv, v.color))
        .collect();
    let sprite_vb = vertex_buffer(&device, &sprite_vertices, "m3-08-sprite-vertices");
    let text_vb = vertex_buffer(&device, &text_vertices, "m3-08-text-vertices");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m3-08-target"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
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
        label: Some("m3-08-encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m3-08-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.015,
                        g: 0.025,
                        b: 0.06,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &sprite_bind, &[]);
        pass.set_vertex_buffer(0, sprite_vb.slice(..));
        for draw in &sprite_mesh.draws {
            pass.draw(
                draw.vertex_start as u32..(draw.vertex_start + draw.vertex_count) as u32,
                0..1,
            );
        }
        pass.set_bind_group(0, &font_bind, &[]);
        pass.set_vertex_buffer(0, text_vb.slice(..));
        pass.draw(0..text_vertices.len() as u32, 0..1);
    }
    queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    })?;
    println!(
        "demo_2d_ok adapter={:?} sprites={} sprite_draws={} text_glyphs={} text_vertices={} textures=2 target=256x256",
        adapter.get_info().name,
        sprite_mesh.sprite_count(),
        sprite_mesh.draws.len(),
        text_mesh.glyph_count,
        text_mesh.vertices.len(),
    );
    Ok(())
}

fn sprite_atlas() -> Result<(TextureAsset, SpriteAtlas), Box<dyn std::error::Error>> {
    let colors = [
        [240, 48, 64, 255],
        [48, 220, 100, 255],
        [64, 112, 240, 255],
        [245, 200, 48, 255],
    ];
    let mut pixels = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for x in 0..4 {
            pixels.extend_from_slice(&colors[(y / 2 * 2 + x / 2) as usize]);
        }
    }
    let asset = TextureAsset::from_rgba8(4, 4, pixels, TextureColorSpace::Srgb)?;
    let mut atlas = SpriteAtlas::from_asset(&asset)?;
    atlas.insert("red", 0, 0, 2, 2)?;
    atlas.insert("green", 2, 0, 2, 2)?;
    atlas.insert("blue", 0, 2, 2, 2)?;
    atlas.insert("yellow", 2, 2, 2, 2)?;
    Ok((asset, atlas))
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    asset: &TextureAsset,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: asset.width(),
            height: asset.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
    texture
}
fn texture_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
fn vertex_buffer(device: &wgpu::Device, vertices: &[GpuVertex], label: &str) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}
fn world_vertex(position: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> GpuVertex {
    GpuVertex {
        position: [position[0] / 128.0 - 1.0, 1.0 - position[1] / 128.0],
        uv,
        color,
    }
}
fn screen_vertex(position: [f32; 2], uv: [f32; 2], color: [f32; 4]) -> GpuVertex {
    GpuVertex {
        position: [position[0] / 128.0 - 1.0, 1.0 - position[1] / 128.0],
        uv,
        color,
    }
}
