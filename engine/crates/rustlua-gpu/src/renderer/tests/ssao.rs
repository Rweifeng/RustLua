//! SSAO-lite 的像素级门：在前后两个不透明世界 Sprite 的深度断层处，开启 SSAO 后
//! 背景像素必须比关闭时更暗；平坦区域不能被整体压暗。
//!
//! 这里故意不走 3D 网格，只用一个全屏背景 + 一个小前景四边形：深度断层、遮挡方向
//! 和采样半径都可控，失败时能直接判断是 SSAO 没执行、还是法线/深度重建符号错了。

use super::*;
use rustlua_render::frame::{SceneMesh, WHITE_TEXTURE_PATH, WorldSpriteVertex};
use rustlua_render::layer::RenderLayer;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};
use rustlua_render::sprite_batch::SpriteDrawBatch;

const SIZE: u32 = 128;
const BACKGROUND_Z: f32 = -1.0;
const FOREGROUND_Z: f32 = 0.0;

fn target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-ssao-target"),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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
    (texture, view)
}

fn quad(half: f32, z: f32) -> Vec<WorldSpriteVertex> {
    [
        (-half, -half),
        (half, -half),
        (-half, half),
        (half, -half),
        (half, half),
        (-half, half),
    ]
    .into_iter()
    .map(|(x, y)| WorldSpriteVertex {
        position: [x, y, z],
        uv: [0.5, 0.5],
        color: [1.0, 1.0, 1.0, 1.0],
    })
    .collect()
}

fn scene_mesh() -> SceneMesh {
    let mut mesh = SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![WHITE_TEXTURE_PATH.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    // 背景占大部分视口；前景占中间约 32%，两者都写深度，形成一条清晰的深度断层。
    mesh.world_sprites.extend(quad(4.0, BACKGROUND_Z));
    mesh.world_sprites.extend(quad(1.5, FOREGROUND_Z));
    mesh.world_sprite_draws = vec![
        SpriteDrawBatch::opaque(0, 0, RenderLayer::World2D, 0, 6),
        SpriteDrawBatch::opaque(0, 1, RenderLayer::World2D, 6, 6),
    ];
    mesh
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * SIZE + x) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

fn render_with(
    renderer: &mut Renderer,
    target: &wgpu::Texture,
    mesh: &SceneMesh,
    scene: &Scene3d,
    ssao: bool,
) -> Vec<u8> {
    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_ssao(ssao),
    );
    renderer.render_into_texture(
        mesh,
        Some(scene),
        &target.create_view(&wgpu::TextureViewDescriptor::default()),
        SIZE,
        SIZE,
        true,
    );
    renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("readback")
}

/// 半球 SSAO 必须真的用重建法线生成 12 个样本，同时保留深度断层回退。
#[test]
fn ssao_shader_builds_a_normal_oriented_hemisphere() {
    let source = include_str!("../../../shaders/ssao.wgsl");
    assert!(source.contains("fn fs_main"));
    assert!(source.contains("let tangent = normalize(cross(reference, normal))"));
    assert!(source.contains("let bitangent = cross(normal, tangent)"));
    assert!(source.contains("for (var i: i32 = 0; i < 12; i = i + 1)"));
    assert!(source.contains("edge_dirs"));
}

/// 双边模糊必须按深度差加权，且 8 个邻域都用 AO 图。
#[test]
fn ssao_blur_shader_weights_neighbours_by_depth() {
    let source = include_str!("../../../shaders/ssao_blur.wgsl");
    assert!(source.contains("exp(-abs(sample_depth - center_depth) * 240.0)"));
    assert!(source.contains("array<vec2<f32>, 8>"));
    assert!(source.contains("textureLoad(ao_source"));
}

#[test]
fn ssao_darkens_background_next_to_a_depth_discontinuity() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-ssao-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("renderer");
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_usable");
    }
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(view),
        overlay: None,
    });
    let scene = Scene3d {
        camera,
        view,
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: Vec::new(),
    };
    let (target, _view) = target(&device);
    let mesh = scene_mesh();

    let off = render_with(&mut renderer, &target, &mesh, &scene, false);
    let on = render_with(&mut renderer, &target, &mesh, &scene, true);
    assert!(
        renderer.last_frame_metrics().ssao_active,
        "HDR + SSAO 必须报告生效"
    );

    // 前景右边界在 x≈89；采样 x=95 仍在背景上，但落在 22 px 采样半径内。
    let off_pixel = pixel(&off, 95, 64);
    let on_pixel = pixel(&on, 95, 64);
    assert!(
        on_pixel[0] < off_pixel[0].saturating_sub(2),
        "深度断层旁的背景必须被 SSAO 压暗：off={off_pixel:?} on={on_pixel:?}"
    );

    // 远离前景的平坦背景不能被整体压暗。
    let flat_off = pixel(&off, 8, 8);
    let flat_on = pixel(&on, 8, 8);
    assert!(
        flat_on[0].abs_diff(flat_off[0]) <= 2,
        "平坦区域不应被 SSAO 整体压暗：off={flat_off:?} on={flat_on:?}"
    );
}
