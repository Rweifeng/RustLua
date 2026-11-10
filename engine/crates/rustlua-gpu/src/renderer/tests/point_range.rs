//! P3-R186：点光 range 边界精确性门——range 外贡献必须精确为 0（与无灯逐字节一致），
//! range 内（0.9R）必须仍被照亮。这条性质在「range 外早退」优化前后都成立：
//! 优化前靠 attenuation=0 乘出 0，优化后靠早退跳过；门保证两者不可区分。
//! 反面对照口径：把早退边界改成 0.5R ⇒ 0.9R 像素失去光照，第一条断言红。

use super::*;

const SIZE: u32 = 128;

fn target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-point-range-target"),
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

fn ground() -> rustlua_render::frame::Scene3dDrawable {
    rustlua_render::frame::Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(1),
        name: "ground".to_owned(),
        mesh_path: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, 0.0, -2.0))
            * Mat4::from_quat(
                Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).expect("quat"),
            )
            * Mat4::scale(Vec3::new(8.0, 8.0, 8.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: rustlua_render::frame::Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-8.0, -8.0, -0.1),
                max: Vec3::new(8.0, 8.0, 0.1),
            },
            source: rustlua_render::frame::BoundsSource::Fallback,
        },
    }
}

fn empty_mesh() -> SceneMesh {
    SceneMesh {
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

fn pixel(pixels: &[u8], (x, y): (u32, u32)) -> [u8; 4] {
    let index = ((y * SIZE + x) * 4) as usize;
    [
        pixels[index],
        pixels[index + 1],
        pixels[index + 2],
        pixels[index + 3],
    ]
}

fn test_device(label: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    Some(
        pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some(label),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device"),
    )
}

#[test]
fn point_light_range_boundary_is_exact() {
    let Some((device, queue)) = test_device("rustlua-point-range-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 5.0, 4.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    // 灯在 (0,0,-1)、range=2：近点 d=1.0（0.5R）、边缘点 d≈1.803（0.90R）、
    // 外点 d≈2.69（1.35R）。方向光颜色全 0 ⇒ 差异全部来自这盏点光。
    let light = PointLight {
        position: Vec3::new(0.0, 0.0, -1.0),
        color: [1.0, 1.0, 1.0],
        intensity: 500.0,
        range: 2.0,
    };
    let scene = |points: Vec<PointLight>| Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: points,
        drawables: vec![ground()],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let near_pixel = sample(Vec3::new(0.0, 0.0, -2.0));
    let edge_pixel = sample(Vec3::new(1.5, 0.0, -2.0));
    let outside_pixel = sample(Vec3::new(2.5, 0.0, -2.0));

    renderer.render_into_texture(&mesh, Some(&scene(vec![light])), &view, SIZE, SIZE, true);
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback on");
    renderer.render_into_texture(&mesh, Some(&scene(Vec::new())), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback off");

    let near_on = pixel(&on, near_pixel);
    let near_off = pixel(&off, near_pixel);
    let edge_on = pixel(&on, edge_pixel);
    let edge_off = pixel(&off, edge_pixel);
    let outside_on = pixel(&on, outside_pixel);
    let outside_off = pixel(&off, outside_pixel);
    assert!(
        edge_on[0] > edge_off[0].saturating_add(10),
        "0.9R 处仍应被照亮（早退边界不许咬进 range 内）：on={edge_on:?} off={edge_off:?}"
    );
    assert!(
        near_on[0] > near_off[0].saturating_add(10),
        "0.5R 处应被照亮：on={near_on:?} off={near_off:?}"
    );
    assert_eq!(
        outside_on, outside_off,
        "1.35R 处贡献必须精确为 0（与无灯逐字节一致）"
    );
}
