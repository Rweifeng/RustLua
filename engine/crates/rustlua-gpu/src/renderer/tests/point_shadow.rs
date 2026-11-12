//! 点光六面阴影的 GPU 门：面必须真的进 plan、真的画，硬/软两条采样路径都只压暗
//! 遮挡点、不误伤阴影外的地面；八盏点光还要各自写 6 层、各自投影。

use super::*;

const SIZE: u32 = 128;

fn target(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-point-shadow-target"),
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

fn plane_drawable(
    name: &str,
    z: f32,
    half: f32,
    scale: f32,
) -> rustlua_render::frame::Scene3dDrawable {
    plane_drawable_at(name, Vec3::new(0.0, 0.0, z), half, scale)
}

fn plane_drawable_at(
    name: &str,
    center: Vec3,
    half: f32,
    scale: f32,
) -> rustlua_render::frame::Scene3dDrawable {
    rustlua_render::frame::Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(1),
        name: name.to_owned(),
        mesh_path: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(center)
            * Mat4::from_quat(
                Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).expect("quat"),
            )
            * Mat4::scale(Vec3::new(scale, scale, scale)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: rustlua_render::frame::Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-half, -half, -0.1),
                max: Vec3::new(half, half, 0.1),
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
fn point_shadow_camera_projects_blocker_closer_than_ground() {
    let cameras = point_shadow_view_projections(Vec3::new(0.0, 0.0, 3.0), 24.0);
    let project = |point: Vec3| {
        let c = &cameras[5].columns;
        let x = c[0][0] * point.x + c[1][0] * point.y + c[2][0] * point.z + c[3][0];
        let y = c[0][1] * point.x + c[1][1] * point.y + c[2][1] * point.z + c[3][1];
        let z = c[0][2] * point.x + c[1][2] * point.y + c[2][2] * point.z + c[3][2];
        let w = c[0][3] * point.x + c[1][3] * point.y + c[2][3] * point.z + c[3][3];
        Vec3::new(x / w, y / w, z / w)
    };
    let blocker = project(Vec3::new(0.0, 0.0, 0.0));
    let ground = project(Vec3::new(0.0, 0.0, -2.0));
    assert!(
        blocker.z > 0.0 && blocker.z < 1.0,
        "blocker z={}",
        blocker.z
    );
    assert!(ground.z > 0.0 && ground.z < 1.0, "ground z={}", ground.z);
    assert!(blocker.z < ground.z, "blocker should be nearer");
}

#[test]
fn point_shadow_darkens_only_the_occluded_ground() {
    let Some((device, queue)) = test_device("rustlua-point-shadow-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(-2.0, 0.0, 3.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 20.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |enabled: bool, soft: bool| {
        RenderSettings::default()
            .with_pbr(true)
            .with_point_shadows(enabled)
            .with_soft_shadows(soft)
    };
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(-2.0, 0.0, -2.0));
    let lit_pixel = sample(Vec3::new(2.0, 0.0, -2.0));

    renderer.set_render_settings(settings(false, false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback off");

    renderer.set_render_settings(settings(true, false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    assert!(
        metrics.point_shadows_active,
        "第一盏点光应触发立方体阴影 pass：{metrics:?}"
    );
    assert!(metrics.draw_calls > 0, "点光阴影帧不应是空帧：{metrics:?}");
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback on");

    // 软阴影开关必须走到点光 3×3 PCF 分支：中心遮挡点仍应被压暗，阴影外仍不应误伤。
    renderer.set_render_settings(settings(true, true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let soft = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback soft");

    let shadow_off = pixel(&off, shadow_pixel);
    let shadow_on = pixel(&on, shadow_pixel);
    let lit_on = pixel(&on, lit_pixel);
    let lit_off = pixel(&off, lit_pixel);
    let soft_shadow = pixel(&soft, shadow_pixel);
    let soft_lit = pixel(&soft, lit_pixel);
    assert!(
        soft_shadow[0].saturating_add(20) < shadow_off[0],
        "软阴影下遮挡点仍应被压暗：off={shadow_off:?} soft={soft_shadow:?}"
    );
    assert!(
        soft_lit[0].saturating_add(5) >= lit_off[0],
        "软阴影不应误伤阴影外：off={lit_off:?} soft={soft_lit:?}"
    );
    assert!(
        shadow_on[0].saturating_add(20) < shadow_off[0],
        "点光阴影应压暗遮挡点：off={shadow_off:?} on={shadow_on:?}"
    );
    assert!(
        lit_on[0].saturating_add(5) >= lit_off[0],
        "阴影外不应被误压暗：off={lit_off:?} on={lit_on:?}"
    );
    assert!(
        lit_on[0] > shadow_on[0].saturating_add(20),
        "阴影外的地面仍应被点光照亮：lit={lit_on:?} shadow={shadow_on:?}"
    );
}

/// 两盏点光必须各写 6 层并各自投影：左红光照亮左遮挡块、右绿光照亮右遮挡块，
/// 左块影子只该丢红光，右块影子只该丢绿光。
#[test]
fn two_point_lights_cast_two_independent_shadow_sets() {
    let Some((device, queue)) = test_device("rustlua-two-point-shadow-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![
            PointLight {
                position: Vec3::new(-2.0, 0.0, 3.0),
                color: [1.0, 0.0, 0.0],
                intensity: 20.0,
                range: 20.0,
            },
            PointLight {
                position: Vec3::new(2.0, 0.0, 3.0),
                color: [0.0, 1.0, 0.0],
                intensity: 20.0,
                range: 20.0,
            },
        ],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("left_blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
            plane_drawable_at("right_blocker", Vec3::new(2.0, 0.0, 0.0), 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |enabled: bool| {
        RenderSettings::default()
            .with_pbr(true)
            .with_point_shadows(enabled)
    };
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let left_shadow = sample(Vec3::new(-2.0, 0.0, -2.0));
    let right_shadow = sample(Vec3::new(2.0, 0.0, -2.0));

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().point_shadows_active,
        "两盏点光应同时触发阴影 pass"
    );
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback on");

    let left_on = pixel(&on, left_shadow);
    let left_off = pixel(&off, left_shadow);
    let right_on = pixel(&on, right_shadow);
    let right_off = pixel(&off, right_shadow);
    assert!(
        left_on[0].saturating_add(10) < left_off[0],
        "左块影子应丢掉左红光的贡献：off={left_off:?} on={left_on:?}"
    );
    assert!(
        right_on[1].saturating_add(10) < right_off[1],
        "右块影子应丢掉右绿光的贡献：off={right_off:?} on={right_on:?}"
    );
    assert!(
        left_on[1].saturating_add(10) >= left_off[1],
        "左块影子不应误伤右绿光：off={left_off:?} on={left_on:?}"
    );
    assert!(
        right_on[0].saturating_add(10) >= right_off[0],
        "右块影子不应误伤左红光：off={right_off:?} on={right_on:?}"
    );
}

/// P3-R21：8 盏 × 6 面 = 48 层，48×64B 矩阵 + 8×16B 参数 = 3200B。
#[test]
fn eight_point_shadow_uniform_layout_is_3200_bytes() {
    assert_eq!(POINT_SHADOW_LIGHTS, 8);
    assert_eq!(POINT_SHADOW_LAYERS, 48);
    assert_eq!(POINT_SHADOW_UNIFORM_BYTES, 3200);
}

/// P3-R21：8 盏点光都要各写 6 层、各自投影；第 8 个槽位的遮挡点必须变暗。
#[test]
fn eight_point_lights_each_cast_a_shadow_set() {
    let Some((device, queue)) = test_device("rustlua-four-point-shadow-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.0, 8.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let xs = [-7.0, -5.0, -3.0, -1.0, 1.0, 3.0, 5.0, 7.0];
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: xs
            .iter()
            .enumerate()
            .map(|(index, x)| PointLight {
                position: Vec3::new(*x, 0.0, 3.0),
                color: [1.0, 1.0, 1.0],
                // 只有第 8 盏给光：前 7 盏仍占槽位/跑 6 面，避免八盏叠加把
                // 单盏阴影稀释到像素阈值以下。
                intensity: if index == 7 { 20.0 } else { 0.0 },
                range: 30.0,
            })
            .collect(),
        drawables: std::iter::once(plane_drawable("ground", -2.0, 10.0, 10.0))
            .chain(xs.iter().enumerate().map(|(index, x)| {
                plane_drawable_at(
                    &format!("blocker-{index}"),
                    Vec3::new(*x, 0.0, 0.0),
                    0.5,
                    0.1,
                )
            }))
            .collect(),
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |enabled: bool| {
        RenderSettings::default()
            .with_pbr(true)
            .with_point_shadows(enabled)
    };
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(7.0, 0.0, -2.0));

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().point_shadows_active,
        "8 盏点光应触发 48 个面 pass"
    );
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback on");

    let shadow_on = pixel(&on, shadow_pixel);
    let shadow_off = pixel(&off, shadow_pixel);
    assert!(
        shadow_on[0].saturating_add(20) < shadow_off[0],
        "第 8 个槽位的遮挡点应被压暗：off={shadow_off:?} on={shadow_on:?}"
    );
}

/// P3-R18：方向光软阴影走 PCSS-lite 时，遮挡点仍必须被压暗、阴影外仍不能被误伤；
/// 点光的 3×3 PCF 路径不受影响。
#[test]
fn directional_pcss_keeps_the_shadow_under_soft_shadows() {
    let Some((device, queue)) = test_device("rustlua-directional-pcss-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |soft: bool| {
        RenderSettings::default()
            .with_pbr(true)
            .with_soft_shadows(soft)
    };
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(0.0, 0.0, -2.0));
    let lit_pixel = sample(Vec3::new(2.0, 0.0, -2.0));

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let hard = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback hard");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let soft = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback soft");

    let hard_shadow = pixel(&hard, shadow_pixel);
    let soft_shadow = pixel(&soft, shadow_pixel);
    let hard_lit = pixel(&hard, lit_pixel);
    let soft_lit = pixel(&soft, lit_pixel);
    assert!(
        soft_shadow[0].saturating_add(20) < hard_lit[0],
        "PCSS 下遮挡点仍应被压暗：hard_lit={hard_lit:?} soft_shadow={soft_shadow:?}"
    );
    assert!(
        soft_lit[0].saturating_add(5) >= hard_lit[0],
        "PCSS 不应误伤阴影外：hard_lit={hard_lit:?} soft_lit={soft_lit:?}"
    );
    assert!(
        hard_shadow[0] < hard_lit[0],
        "硬阴影基准本身应有遮挡：hard_shadow={hard_shadow:?} hard_lit={hard_lit:?}"
    );
}

fn floor_drawable(name: &str, center: Vec3) -> rustlua_render::frame::Scene3dDrawable {
    rustlua_render::frame::Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(1),
        name: name.to_owned(),
        mesh_path: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(center) * Mat4::scale(Vec3::new(4.0, 1.0, 4.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: rustlua_render::frame::Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-4.0, -0.1, -4.0),
                max: Vec3::new(4.0, 0.1, 4.0),
            },
            source: rustlua_render::frame::BoundsSource::Fallback,
        },
    }
}

/// P3-R24：ESM 生成 shader 写 `exp(depth*10)`，主 shader 以 `exp(-10*receiver)` 还原；
/// 两张主着色器都必须声明 binding 8。
#[test]
fn esm_shader_writes_and_samples_the_exponential_map() {
    let generate = include_str!("../../../shaders/esm_shadow.wgsl");
    assert!(generate.contains("exp(depth * 10.0)"));
    assert!(generate.contains("textureLoad(shadow_depth"));
    for source in [
        include_str!("../../../shaders/mesh3d_pbr.wgsl"),
        include_str!("../../../shaders/mesh3d.wgsl"),
    ] {
        assert!(source.contains("@group(0) @binding(8) var esm_map: texture_2d_array<f32>"));
        assert!(source.contains("fn sample_esm"));
        assert!(source.contains("exp(-10.0 * (shadow_depth - 0.002))"));
    }
}

/// 景深像素门：全画面都在远焦点外时，打开后必须改变至少一批像素。
#[test]
fn depth_of_field_blurs_off_focus_pixels() {
    let Some((device, queue)) = test_device("rustlua-dof-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.5, 6.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            floor_drawable("floor", Vec3::new(0.0, -2.0, 0.0)),
            plane_drawable("wall", -4.0, 4.0, 2.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |dof: bool| {
        RenderSettings::default()
            .with_hdr(rustlua_render::render_settings::ToggleMode::On)
            .with_pbr(true)
            .with_dof(dof)
            .with_dof_focus(0.0)
            .with_dof_range(1.0)
            .with_dof_strength(1.0)
    };

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback dof off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().dof_active);
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback dof on");

    let mut changed = 0usize;
    for index in 0..(SIZE * SIZE) as usize {
        if on[index * 4].abs_diff(off[index * 4]) > 1 {
            changed += 1;
        }
    }
    assert!(changed > 0, "景深必须改变至少一批像素：changed={changed}");
}

/// 神光的像素门：太阳在屏幕内、画面有亮墙时，打开后必须至少改变一批像素。
#[test]
fn god_rays_add_energy_when_the_sun_is_on_screen() {
    let Some((device, queue)) = test_device("rustlua-god-rays-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.5, 6.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.18, 0.98)
                .normalized()
                .expect("light direction"),
            color: [1.0, 1.0, 1.0],
            intensity: 3.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("wall", -4.0, 4.0, 2.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |god_rays: bool| {
        RenderSettings::default()
            .with_hdr(rustlua_render::render_settings::ToggleMode::On)
            .with_pbr(true)
            .with_god_rays(god_rays)
    };

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback god rays off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().god_rays_active);
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback god rays on");

    let mut changed = 0usize;
    for index in 0..(SIZE * SIZE) as usize {
        if on[index * 4].abs_diff(off[index * 4]) > 1 {
            changed += 1;
        }
    }
    assert!(changed > 0, "神光必须改变至少一批像素：changed={changed}");
}

/// 距离雾的像素门：同一场景打开雾后，远处像素必须被雾色替换一部分。
#[test]
fn distance_fog_tints_distant_pixels() {
    let Some((device, queue)) = test_device("rustlua-fog-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.5, 6.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            floor_drawable("floor", Vec3::new(0.0, -2.0, 0.0)),
            plane_drawable("wall", -4.0, 4.0, 2.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |fog: bool| {
        RenderSettings::default()
            .with_hdr(rustlua_render::render_settings::ToggleMode::On)
            .with_pbr(true)
            .with_fog(fog)
            .with_fog_density(0.5)
    };

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback fog off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().fog_active);
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback fog on");

    let mut changed = 0usize;
    for index in 0..(SIZE * SIZE) as usize {
        if on[index * 4].abs_diff(off[index * 4]) > 2 {
            changed += 1;
        }
    }
    assert!(changed > 0, "雾必须改变至少一批像素：changed={changed}");
}

/// P3-R24：方向光 ESM 必须真的生成 4 层 1024² 滤波图，并让遮挡点比不开时更暗。
#[test]
fn esm_filtered_shadows_darken_the_occluded_floor() {
    let Some((device, queue)) = test_device("rustlua-esm-shadow-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |filtered: bool| {
        RenderSettings::default()
            .with_pbr(true)
            .with_filtered_shadows(filtered)
    };
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(0.0, 0.0, -2.0));
    let lit_pixel = sample(Vec3::new(2.0, 0.0, -2.0));

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback esm off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().filtered_shadows_active,
        "ESM 打开后必须报告 4 层滤波图已生成"
    );
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback esm on");

    let lit = pixel(&off, lit_pixel);
    let esm_shadow = pixel(&on, shadow_pixel);
    assert!(
        esm_shadow[0].saturating_add(10) < lit[0],
        "ESM 必须压暗遮挡点：lit={lit:?} esm_shadow={esm_shadow:?}"
    );
}

/// 接触阴影的像素门：方向光从斜上方照地面 + 立墙，打开后墙根/遮挡处必须变暗，
/// 关闭时保持基线。
#[test]
fn contact_shadows_darken_the_base_of_a_wall() {
    let Some((device, queue)) = test_device("rustlua-contact-shadow-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.5, 6.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(-0.4, -1.0, 0.3)
                .normalized()
                .expect("light direction"),
            color: [1.0, 1.0, 1.0],
            intensity: 2.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            floor_drawable("floor", Vec3::new(0.0, -2.0, 0.0)),
            plane_drawable("wall", -4.0, 4.0, 2.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |contact: bool| {
        RenderSettings::default()
            .with_hdr(rustlua_render::render_settings::ToggleMode::On)
            .with_pbr(true)
            .with_contact_shadows(contact)
    };

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback contact off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().contact_shadows_active,
        "HDR + 接触阴影打开后必须报告生效"
    );
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback contact on");

    let mut darkened = 0usize;
    for index in 0..(SIZE * SIZE) as usize {
        if on[index * 4] + 2 < off[index * 4] {
            darkened += 1;
        }
    }
    assert!(
        darkened > 0,
        "接触阴影必须压暗墙根像素：darkened={darkened}"
    );
}

/// SSR-lite 的像素门：地面反射一盏亮墙的能量，必须让至少一批像素比关反射时更亮；
/// 关掉 SSR 的帧必须完全回到基线。
#[test]
fn screen_space_reflection_adds_energy_only_when_enabled() {
    let Some((device, queue)) = test_device("rustlua-ssr-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 1.5, 6.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(0.0, 3.0, 2.0),
            color: [1.0, 1.0, 1.0],
            intensity: 30.0,
            range: 40.0,
        }],
        drawables: vec![
            floor_drawable("floor", Vec3::new(0.0, -2.0, 0.0)),
            plane_drawable("wall", -4.0, 4.0, 2.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = |ssr: bool| {
        RenderSettings::default()
            .with_hdr(rustlua_render::render_settings::ToggleMode::On)
            .with_pbr(true)
            .with_ssr(ssr)
    };

    renderer.set_render_settings(settings(false));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let off = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback ssr off");

    renderer.set_render_settings(settings(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().ssr_active,
        "HDR + SSR 打开后必须报告生效"
    );
    let on = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback ssr on");

    let mut changed = 0usize;
    let mut max_delta = 0i32;
    for index in 0..(SIZE * SIZE) as usize {
        let delta = (0..3)
            .map(|channel| (on[index * 4 + channel] as i32 - off[index * 4 + channel] as i32).abs())
            .max()
            .unwrap_or(0);
        if delta > 2 {
            changed += 1;
        }
        max_delta = max_delta.max(delta);
    }
    assert!(
        changed > 0,
        "SSR 帧必须至少改变一批像素：changed={changed} max_delta={max_delta}"
    );
}

/// P3-R199：方向光 CSM 级联上限必须真的少录制 pass（不是只改 uniform）。
///
/// `shadow_passes_recorded` 是确定量，不看时间也不受 GPU 时钟漂移影响。
#[test]
fn shadow_cascade_limit_records_only_the_requested_passes() {
    let Some((device, queue)) = test_device("rustlua-shadow-cascade-limit-device") else {
        return gpu_skipped("request_adapter");
    };
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![plane_drawable("ground", -2.0, 4.0, 4.0)],
    };
    let mesh = empty_mesh();
    let (_texture, view) = target(&device);
    let count = |limit: u32| {
        let mut renderer = Renderer::new_offscreen_with_shadow_settings(
            device.clone(),
            queue.clone(),
            None,
            false,
            false,
            512,
            limit,
            DEFAULT_POINT_SHADOW_RESOLUTION,
        )
        .expect("offscreen renderer");
        renderer.set_render_settings(RenderSettings::default().with_pbr(true));
        let outcome = renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "帧必须正常出图"
        );
        let recorded = renderer.last_frame_metrics().shadow_passes_recorded;
        // swiftshader 上渲染器 drop 会等待最后一次提交；先显式排空，避免 10 s 超时误红。
        drop(renderer);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        recorded
    };
    assert_eq!(count(4), 4, "默认 4 级联必须与旧行为一致");
    assert_eq!(count(2), 2, "2 档只能录 2 个方向光级联 pass");
}

/// P3-R200：点光 cube 阴影面边长必须真的按档建出来（读纹理实际尺寸，不是参数回显）。
/// 512 档把 8×6 面的显存从 192 MiB 压到 48 MiB；默认仍是 1024² ⇒ 旧路径不变。
#[test]
fn point_shadow_resolution_creates_the_requested_face_size() {
    let Some((device, queue)) = test_device("rustlua-point-shadow-resolution-device") else {
        return gpu_skipped("request_adapter");
    };
    let make = |size: u32| {
        let renderer = Renderer::new_offscreen_with_shadow_settings(
            device.clone(),
            queue.clone(),
            None,
            false,
            false,
            512,
            4,
            size,
        )
        .expect("offscreen renderer");
        let built = renderer.point_shadow_resolution();
        drop(renderer);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        built
    };
    assert_eq!(make(512), 512, "512 档必须真的建 512² 的面");
    assert_eq!(
        make(DEFAULT_POINT_SHADOW_RESOLUTION),
        1024,
        "默认档必须与旧行为一致"
    );
}

/// P3-R180：提交阶段的 CPU 归因必须真的接在阴影链上。
///
/// 宿主的 `host_phases.submit`（`game_app.rs`）包住 acquire + 命令录制 + `queue.submit` +
/// present 四件事，而渲染器此前只报一个 `cpu_frame_us` 总数 ⇒「下一刀砍哪」只能靠猜。
/// 这条门钉三件事：
/// * **pass 计数是确定量**：1 盏点光 ⇒ 6 个立方体面 pass；方向光 4 级联
///   （`filtered_shadows` 默认关 ⇒ 没有 ESM 全屏）⇒ 恰好 4；关掉 `point_shadows` ⇒ 0；
/// * **计时挂在正确的段上**：有点光那一帧的点光录制时间必须显著大于关掉的那一帧，
///   且「阴影两桶之和 ≤ 总录制 ≤ `cpu_frame_us`」；
/// * **离屏路径没有 acquire/present**：两桶恒 0，否则前台的帧步调阻塞会被误算成录制开销。
#[test]
fn submit_cpu_attribution_separates_the_shadow_chain() {
    let Some((device, queue)) = test_device("rustlua-submit-attribution-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(-2.0, 0.0, 3.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 20.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);

    let frame = |renderer: &mut Renderer, point_shadows: bool| {
        renderer.set_render_settings(
            RenderSettings::default()
                .with_pbr(true)
                .with_point_shadows(point_shadows),
        );
        let outcome = renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "归因帧必须正常出图"
        );
        renderer.last_frame_metrics()
    };

    // 预热两帧：首帧的 `cpu_record_us` 里全是 PBR/阴影管线变体的编译（实测 136 ms vs
    // 稳态 ~2 ms），不预热的话量到的是编译而不是录制。
    let _warm = frame(&mut renderer, true);
    let _warm = frame(&mut renderer, true);
    let on = frame(&mut renderer, true);
    let off = frame(&mut renderer, false);
    println!(
        "submit attribution: on(cascades={} point_faces={} shadow_us={} point_us={} record_us={} frame_us={}) off(point_faces={} point_us={} record_us={})",
        on.shadow_passes_recorded,
        on.point_shadow_passes_recorded,
        on.record_shadow_us,
        on.record_point_shadow_us,
        on.cpu_record_us,
        on.cpu_frame_us,
        off.point_shadow_passes_recorded,
        off.record_point_shadow_us,
        off.cpu_record_us,
    );

    assert_eq!(
        on.shadow_passes_recorded, 4,
        "方向光阴影应是 4 个级联 pass（filtered_shadows 默认关 ⇒ 没有 ESM 全屏）"
    );
    assert!(on.point_shadows_active, "点光阴影必须报告生效");
    assert_eq!(
        on.point_shadow_passes_recorded, 6,
        "1 盏点光 ⇒ 6 个立方体面 pass"
    );
    assert_eq!(
        off.point_shadow_passes_recorded, 0,
        "关掉 point_shadows 后不应再录制任何点光面 pass"
    );
    assert!(
        on.record_point_shadow_us > off.record_point_shadow_us * 8,
        "点光录制时间没接在点光段上：开 {}us vs 关 {}us",
        on.record_point_shadow_us,
        off.record_point_shadow_us
    );
    assert!(
        on.record_shadow_us > 0,
        "4 个级联 pass 的录制时间不可能是 0"
    );
    assert!(
        on.record_shadow_us + on.record_point_shadow_us <= on.cpu_record_us,
        "阴影两桶之和 {}us 超过总录制 {}us",
        on.record_shadow_us + on.record_point_shadow_us,
        on.cpu_record_us
    );
    // P3-R285：planar / 主视图不透明网格两段录制子桶必须接在自己的段上。这个场景有网格
    // 没水面 ⇒ depth 桶必须 > 0；挂错桶（例如累加进 shadow）画面上看不出来，只有这里能抓住。
    assert!(
        on.record_depth_us > 0,
        "主视图不透明网格 pass 的录制时间不可能是 0"
    );
    assert!(
        on.record_shadow_us
            + on.record_point_shadow_us
            + on.record_planar_us
            + on.record_depth_us
            + on.record_prep_us
            + on.record_tail_us
            + on.record_binds_us
            <= on.cpu_record_us,
        "四条 pass 子桶之和超过总录制 {}us",
        on.cpu_record_us
    );
    // P3-R286：准备段恒 > 0（计划推导与 uniform 更新每帧都跑）；挂错桶会在这里红。
    assert!(on.record_prep_us > 0, "pass 前准备段的录制时间不可能是 0");
    // P3-R288：命名桶必须覆盖 ≥90% 的录制时间。残差是渲染器内逐帧算的，挂错任何一段
    // 都会把它顶大；这条断言就是「录制已被完全命名」的判据（不是 p50 相减的口径）。
    assert!(
        on.record_other_us * 10 <= on.cpu_record_us,
        "逐帧残差 {}us 超过总录制 {}us 的 10%",
        on.record_other_us,
        on.cpu_record_us
    );
    println!(
        "record split: shadow={} point_shadow={} planar={} depth={} prep={} tail={} binds={} other={} of record={}us",
        on.record_shadow_us,
        on.record_point_shadow_us,
        on.record_planar_us,
        on.record_depth_us,
        on.record_prep_us,
        on.record_tail_us,
        on.record_binds_us,
        on.record_other_us,
        on.cpu_record_us
    );
    assert!(
        on.cpu_record_us <= on.cpu_frame_us,
        "总录制 {}us 超过 cpu_frame_us {}us",
        on.cpu_record_us,
        on.cpu_frame_us
    );
    assert_eq!(
        (on.cpu_acquire_us, on.cpu_present_us),
        (0, 0),
        "离屏路径没有 acquire/present，这两桶必须恒 0"
    );
    // 阴影链在总录制里的占比：这条读数就是「下一刀砍哪」的依据，别只留在断言里。
    let chain_us = on.record_shadow_us + on.record_point_shadow_us;
    println!(
        "submit attribution share: chain_us={chain_us} of record_us={} ({:.1}%) drawables={} record_delta_us={}",
        on.cpu_record_us,
        100.0 * f64::from(chain_us as u32) / f64::from(on.cpu_record_us.max(1) as u32),
        scene.drawables.len(),
        on.cpu_record_us.saturating_sub(off.cpu_record_us),
    );
    // 收尾回读 = 显式完成栅栏：离屏连发 4 帧不回读时，swiftshader 上最后一条提交会在
    // device drop 的等待里超时（wgpu-core `queue.rs`：「timed out while waiting on the last
    // successful submission」），整条门会以 exit 125 假红。
    let pixels = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("attribution readback");
    assert!(!pixels.is_empty(), "回读必须拿到像素");
}

/// P3-R188：点光阴影按光照 range 剔除投射者——range 外的组不算进任何面，
/// range 内的投射者一个都不许误剔（阴影照常压暗遮挡点）。远场块放在 x=2000，
/// 相机剔除关闭，保证它真进 `frame_draws`、由 range 剔除负责挡下。
#[test]
fn point_shadow_culls_casters_outside_light_range_only() {
    let Some((device, queue)) = test_device("rustlua-point-shadow-cull-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let far = plane_drawable_at("far", Vec3::new(2000.0, 0.0, -2.0), 4.0, 4.0);
    let scene = |drawables: Vec<rustlua_render::frame::Scene3dDrawable>| Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(-2.0, 0.0, 3.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 20.0,
        }],
        drawables,
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = RenderSettings::default()
        .with_pbr(true)
        .with_point_shadows(true);
    renderer.set_render_settings(settings);

    // 对照：全部 drawable 都在 range 内 ⇒ 一个都不许剔。
    renderer.render_into_texture(
        &mesh,
        Some(&scene(vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
        ])),
        &view,
        SIZE,
        SIZE,
        true,
    );
    assert_eq!(
        renderer.last_frame_metrics().point_shadow_draws_culled,
        0,
        "range 内的投射者不许被剔除"
    );

    // 远场块（x=2000，远在 range=20 外）⇒ 每个面都要被剔。
    renderer.render_into_texture(
        &mesh,
        Some(&scene(vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
            far,
        ])),
        &view,
        SIZE,
        SIZE,
        true,
    );
    let metrics = renderer.last_frame_metrics();
    assert!(
        metrics.point_shadow_draws_culled >= 6,
        "range 外远场块应在六个面上都被剔除：{metrics:?}"
    );

    // 阴影仍须压暗遮挡点（剔除没把 range 内的投射者带走）：与关阴影的同场景对照。
    let same_scene = || {
        scene(vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
            plane_drawable_at("far", Vec3::new(2000.0, 0.0, -2.0), 4.0, 4.0),
        ])
    };
    let ndc = camera
        .world_to_ndc(Vec3::new(-2.0, 0.0, -2.0), viewport)
        .expect("ndc");
    let shadow_pixel = (
        ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
    );
    let shadowed = pixel(
        &renderer
            .copy_texture_to_cpu(&texture, SIZE, SIZE)
            .expect("readback"),
        shadow_pixel,
    );
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    renderer.render_into_texture(&mesh, Some(&same_scene()), &view, SIZE, SIZE, true);
    let lit = pixel(
        &renderer
            .copy_texture_to_cpu(&texture, SIZE, SIZE)
            .expect("readback"),
        shadow_pixel,
    );
    assert!(
        shadowed[0].saturating_add(5) < lit[0],
        "遮挡点仍应被点光阴影压暗：shadowed={shadowed:?} lit={lit:?}"
    );
}

/// P3-R189：方向光逐级联剔除——级联盒外的远场块（x=2000）在四个级联里全部被剔；
/// 视锥内的地面/遮挡块一级都不许误剔（远场块每级一次 ⇒ 恰好 4）。方向光阴影的像素
/// 正确性由 `directional_pcss_keeps_the_shadow_under_soft_shadows` 守护。
#[test]
fn shadow_cascade_culls_draws_outside_every_cascade() {
    let Some((device, queue)) = test_device("rustlua-shadow-cascade-cull-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
            plane_drawable_at("far", Vec3::new(2000.0, 0.0, -2.0), 4.0, 4.0),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.shadow_cascade_draws_culled, 4,
        "远场块应在四个级联里各被剔一次、盒内 draw 一个不许剔：{metrics:?}"
    );
    // P3-R247：隐藏计数必须与剔除计数对得上——3 条 draw × 4 级联 = 12 次尝试，
    // 剔 4 次 ⇒ 提交 8 次。只报剔除不报提交时，"阴影便宜了"仍然只能靠猜。
    assert_eq!(
        metrics.shadow_draws_drawn, 8,
        "每条级联应提交 2 条 draw：{metrics:?}"
    );
    assert_eq!(
        metrics.shadow_instances_drawn, 8,
        "每个平面 1 份实例 ⇒ 提交实例数应等于提交 draw 数：{metrics:?}"
    );
    assert!(metrics.draw_calls > 0, "阴影帧不应是空帧：{metrics:?}");
    // 收尾回读 = 显式完成栅栏：离屏帧不回读时 swiftshader 会在 device drop 的等待里
    // 超时（wgpu-core queue.rs「timed out while waiting on the last successful submission」）。
    let pixels = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback");
    assert!(!pixels.is_empty(), "回读必须拿到像素");
}

/// P3-R194：点光逐面方向锥剔除。遮挡块在灯的正 −z 方向 ⇒ 只有 −Z 面能看到它，
/// 其余 5 个面必须把它剔掉；range 剔除一个都不许报。range 外的块由 R188 门守护。
#[test]
fn point_shadow_culls_casters_outside_each_face_frustum() {
    let Some((device, queue)) = test_device("rustlua-point-face-cull-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(-2.0, 0.0, 3.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 20.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable_at("blocker", Vec3::new(-2.0, 0.0, 0.0), 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_point_shadows(true),
    );
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.point_shadow_draws_culled, 0,
        "全部在 range 内，不许报 range 剔除：{metrics:?}"
    );
    assert!(
        metrics.point_shadow_face_draws_culled >= 4,
        "遮挡块只在 −Z 面视野内，其余面应被方向锥剔掉：{metrics:?}"
    );
    // P3-R261：点光链的**提交**必须可数（此前只有剔除/跳过计数）。2 条 draw × 6 面 =
    // 12 次尝试；range 剔除、距离预算、分类排除这一帧都是 0 ⇒ 提交 + 面剔除 = 12。
    // 这条守恒式就是门：少记一次提交、或把面剔除漏记，等式立刻不成立。
    assert_eq!(
        metrics.point_shadow_draws_drawn + metrics.point_shadow_face_draws_culled,
        12,
        "点光链守恒式：提交 + 面剔除 = drawable × 面：{metrics:?}"
    );
    assert!(
        metrics.point_shadow_draws_drawn > 0,
        "ground 在每个面的视野内，提交数不应为 0：{metrics:?}"
    );
    assert_eq!(
        metrics.point_shadow_instances_drawn, metrics.point_shadow_draws_drawn,
        "每个平面 1 份实例 ⇒ 提交实例数应等于提交 draw 数：{metrics:?}"
    );
    assert!(metrics.draw_calls > 0, "阴影帧不应是空帧：{metrics:?}");
    // 收尾回读栅栏（swiftshader 不回读会在 device drop 等待里超时）。
    let pixels = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback");
    assert!(!pixels.is_empty(), "回读必须拿到像素");
}

/// P3-R202：盏数预算必须真的少录 pass：3 盏灯、预算 1 ⇒ 6 面；预算 8 ⇒ 18 面。
#[test]
fn point_shadow_light_budget_caps_recorded_passes() {
    let Some((device, queue)) = test_device("point-shadow-light-budget") else {
        return;
    };
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            color: [0.0, 0.0, 0.0],
            ..Light::default()
        },
        point_lights: [-2.0, 0.0, 2.0]
            .into_iter()
            .map(|x| PointLight {
                position: Vec3::new(x, 0.0, 3.0),
                color: [1.0, 1.0, 1.0],
                intensity: 20.0,
                range: 20.0,
            })
            .collect(),
        drawables: vec![plane_drawable("ground", -2.0, 4.0, 4.0)],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let count = |budget: u32| {
        let mut renderer =
            Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
                .expect("offscreen renderer");
        renderer.set_render_settings(
            RenderSettings::default()
                .with_pbr(true)
                .with_point_shadows(true),
        );
        renderer.set_point_shadow_light_budget(budget);
        assert_eq!(
            renderer.point_shadow_light_budget(),
            budget.clamp(1, 8),
            "预算读数必须是 clamp 后的实际值"
        );
        let outcome = renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "帧必须正常出图"
        );
        let recorded = renderer.last_frame_metrics().point_shadow_passes_recorded;
        // swiftshader 上渲染器 drop 会等待最后一次提交；先显式排空，避免 10 s 超时误红。
        drop(renderer);
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        recorded
    };
    // 默认带宽 8：3 盏灯各录 6 面 = 18（与旧行为一致）。
    assert_eq!(count(8), 18, "默认预算必须给 3 盏灯各录 6 面");
    assert_eq!(count(1), 6, "预算 1 只能录 6 个面");
    drop(texture);
}

/// P3-R248：阴影投射分类排除档只掐方向光 CSM 链——遮挡块被排除后地面阴影点变亮，
/// 主视图照画（`draw_calls` 不变），排除量单独计数且与提交量守恒：
/// `提交(带档) + 排除 = 提交(默认)`。还原后阴影必须回来。
#[test]
fn shadow_caster_skip_drops_the_shadow_and_keeps_the_main_pass() {
    let Some((device, queue)) = test_device("rustlua-shadow-caster-skip-device") else {
        return gpu_skipped("request_adapter");
    };
    // 关实例合批：两个同网格路径的 drawable 才能各自按**名字**分类（合批组只认路径）。
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(0.0, 0.0, -2.0));
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));

    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let baseline = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback baseline");
    let baseline_metrics = renderer.last_frame_metrics();
    let shadowed = pixel(&baseline, shadow_pixel);
    assert_eq!(
        baseline_metrics.shadow_draws_skipped, 0,
        "默认档不许排除任何 caster"
    );
    assert!(
        baseline_metrics.shadow_draws_drawn > 0,
        "默认档应真提交阴影 draw：{baseline_metrics:?}"
    );

    renderer.set_shadow_caster_skip(Some("blocker".to_owned()));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let excluded = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback excluded");
    let excluded_metrics = renderer.last_frame_metrics();
    let unshadowed = pixel(&excluded, shadow_pixel);
    assert!(
        excluded_metrics.shadow_draws_skipped >= 1,
        "排除档必须计数：{excluded_metrics:?}"
    );
    assert_eq!(
        excluded_metrics.shadow_draws_drawn + excluded_metrics.shadow_draws_skipped,
        baseline_metrics.shadow_draws_drawn,
        "排除量必须与提交量守恒：带档提交 + 排除 = 默认提交"
    );
    assert_eq!(
        excluded_metrics.draw_calls, baseline_metrics.draw_calls,
        "主视图仍须画遮挡块（只掐阴影链）"
    );
    assert!(
        shadowed[0].saturating_add(10) < unshadowed[0],
        "排除 caster 后地面阴影点应变亮：shadowed={shadowed:?} unshadowed={unshadowed:?}"
    );

    renderer.set_shadow_caster_skip(None);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let restored = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback restored");
    let restored_metrics = renderer.last_frame_metrics();
    let restored_shadow = pixel(&restored, shadow_pixel);
    assert_eq!(restored_metrics.shadow_draws_skipped, 0, "还原后不许再排除");
    assert_eq!(
        restored_metrics.shadow_draws_drawn, baseline_metrics.shadow_draws_drawn,
        "还原后提交量必须回到默认"
    );
    assert!(
        restored_shadow[0].saturating_add(10) < unshadowed[0],
        "还原后阴影必须回来：restored={restored_shadow:?} unshadowed={unshadowed:?}"
    );
    // 收尾回读 = 显式完成栅栏：swiftshader 下不回读时会在 device drop 的等待里超时。
    drop(texture);
}

/// P3-R249：同一分类排除档必须**同时**掐点光 6 面链（P3-R248 只管方向光 CSM）。
/// 遮挡块被排除后点光链的排除计数必须涨、方向光提交量照旧；还原后两边都清零。
///
/// 抓的假绿：只改方向光链时点光计数恒 0——本门会红在「点光链必须记到排除」。
#[test]
fn shadow_caster_skip_also_drops_point_shadow_casters() {
    let Some((device, queue)) = test_device("rustlua-point-caster-skip-device") else {
        return gpu_skipped("request_adapter");
    };
    // 关实例合批：两个同网格路径的 drawable 才能各自按**名字**分类。
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(2.0, 2.0, 2.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 30.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = RenderSettings::default()
        .with_pbr(true)
        .with_point_shadows(true);
    renderer.set_render_settings(settings);

    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback baseline");
    let baseline = renderer.last_frame_metrics();
    assert!(
        baseline.point_shadows_active,
        "点光应触发立方体阴影 pass：{baseline:?}"
    );
    assert_eq!(
        baseline.point_shadow_draws_skipped, 0,
        "默认档不许排除点光 caster：{baseline:?}"
    );

    renderer.set_shadow_caster_skip(Some("blocker".to_owned()));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback excluded");
    let excluded = renderer.last_frame_metrics();
    assert!(
        excluded.point_shadow_draws_skipped >= 1,
        "点光链必须记到排除：{excluded:?}"
    );
    // 排除量不一定是 6 的倍数：面剔除在排除之前，只有"本来会提交"的那几个面才记。
    assert!(
        excluded.point_shadow_draws_skipped <= 6,
        "单盏灯最多 6 个面，排除量不可能超过它：{excluded:?}"
    );
    assert!(
        excluded.shadow_draws_skipped >= 1,
        "方向光链的排除必须照旧生效：{excluded:?}"
    );

    renderer.set_shadow_caster_skip(None);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback restored");
    let restored = renderer.last_frame_metrics();
    assert_eq!(
        restored.point_shadow_draws_skipped, 0,
        "还原后点光链不许再排除：{restored:?}"
    );
    assert_eq!(
        restored.shadow_draws_skipped, 0,
        "还原后方向光链不许再排除：{restored:?}"
    );
    // 收尾回读 = 显式完成栅栏（swiftshader 的 device drop 等待）。
    drop(texture);
}

/// P3-R265：点光专用排除档只掐点光 6 面链——方向光 CSM 的提交量**必须**与默认档相同
/// （`shadow_draws_skipped` 恒 0），点光链的排除量涨上来且与默认提交量守恒，主视图照画；
/// 还原后点光链回到默认。
///
/// 抓的假绿：把点光专用档写成与 `shadow_caster_skip` 同一位 ⇒ 方向光链也会记到排除，
/// 本门会红在「CSM 提交量必须守恒」。
#[test]
fn point_shadow_caster_skip_only_drops_the_point_chain() {
    let Some((device, queue)) = test_device("rustlua-point-only-caster-skip-device") else {
        return gpu_skipped("request_adapter");
    };
    // 关实例合批：两个同网格路径的 drawable 才能各自按**名字**分类。
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(2.0, 2.0, 2.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 30.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let settings = RenderSettings::default()
        .with_pbr(true)
        .with_point_shadows(true);
    renderer.set_render_settings(settings);

    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback baseline");
    let baseline = renderer.last_frame_metrics();
    assert!(
        baseline.point_shadows_active,
        "点光应触发立方体阴影 pass：{baseline:?}"
    );
    assert!(
        baseline.point_shadow_draws_drawn > 0,
        "默认档点光链应真提交：{baseline:?}"
    );
    assert!(
        baseline.shadow_draws_drawn > 0,
        "默认档方向光链应真提交（否则下面的守恒断言是空等式）：{baseline:?}"
    );
    assert_eq!(baseline.point_shadow_draws_skipped, 0);
    assert_eq!(baseline.shadow_draws_skipped, 0);

    renderer.set_point_shadow_caster_skip(Some("blocker".to_owned()));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback excluded");
    let excluded = renderer.last_frame_metrics();
    assert!(
        excluded.point_shadow_draws_skipped >= 1,
        "点光链必须记到排除：{excluded:?}"
    );
    assert_eq!(
        excluded.point_shadow_draws_drawn + excluded.point_shadow_draws_skipped,
        baseline.point_shadow_draws_drawn,
        "点光链守恒：带档提交 + 排除 = 默认提交"
    );
    assert_eq!(
        excluded.shadow_draws_skipped, 0,
        "方向光 CSM 不许被点光专用档波及：{excluded:?}"
    );
    assert_eq!(
        excluded.shadow_draws_drawn, baseline.shadow_draws_drawn,
        "方向光 CSM 的提交量必须与默认档相同"
    );
    assert_eq!(
        excluded.draw_calls, baseline.draw_calls,
        "主视图仍须画遮挡块（只掐点光链）"
    );

    renderer.set_point_shadow_caster_skip(None);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback restored");
    let restored = renderer.last_frame_metrics();
    assert_eq!(
        restored.point_shadow_draws_skipped, 0,
        "还原后点光链不许再排除：{restored:?}"
    );
    assert_eq!(
        restored.point_shadow_draws_drawn, baseline.point_shadow_draws_drawn,
        "还原后点光提交量必须回到默认"
    );
    // 收尾回读 = 显式完成栅栏（swiftshader 的 device drop 等待）。
    drop(texture);
}

/// P3-R256：距离预算只掐阴影链——相机到遮挡块超过预算时地面阴影点变亮、主视图照画
/// （`draw_calls` 不变），跳过量单独计数且与提交量守恒；预算很大或还原成 0 都退回默认。
///
/// 抓的假绿：只 skip 不计数 ⇒ 守恒式对不上；预算误伤主视图 ⇒ `draw_calls` 变少。
#[test]
fn shadow_caster_distance_drops_far_casters_and_keeps_the_main_pass() {
    let Some((device, queue)) = test_device("rustlua-shadow-caster-distance-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    let sample = |world: Vec3| {
        let ndc = camera.world_to_ndc(world, viewport).expect("ndc");
        (
            ((ndc.x * 0.5 + 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
            ((0.5 - ndc.y * 0.5) * SIZE as f32).clamp(0.0, (SIZE - 1) as f32) as u32,
        )
    };
    let shadow_pixel = sample(Vec3::new(0.0, 0.0, -2.0));
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));

    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let baseline = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback baseline");
    let baseline_metrics = renderer.last_frame_metrics();
    assert_eq!(
        baseline_metrics.shadow_distance_skipped, 0,
        "默认档不许按距离掐 caster"
    );
    assert!(
        baseline_metrics.shadow_draws_drawn > 0,
        "默认档应真提交阴影 draw：{baseline_metrics:?}"
    );
    let shadowed = pixel(&baseline, shadow_pixel);

    // 预算 2 m：遮挡块与地面都在 6 m 开外 ⇒ 整条 CSM 链只剩空 pass。
    renderer.set_shadow_caster_max_distance(2.0);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let excluded = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback excluded");
    let excluded_metrics = renderer.last_frame_metrics();
    let unshadowed = pixel(&excluded, shadow_pixel);
    assert!(
        excluded_metrics.shadow_distance_skipped >= 1,
        "距离预算必须计数：{excluded_metrics:?}"
    );
    assert_eq!(
        excluded_metrics.shadow_draws_drawn + excluded_metrics.shadow_distance_skipped,
        baseline_metrics.shadow_draws_drawn,
        "距离跳过量必须与提交量守恒：带档提交 + 跳过 = 默认提交"
    );
    assert_eq!(
        excluded_metrics.draw_calls, baseline_metrics.draw_calls,
        "主视图仍须画遮挡块（只掐阴影链）"
    );
    assert!(
        shadowed[0].saturating_add(10) < unshadowed[0],
        "掐掉远处 caster 后地面阴影点应变亮：shadowed={shadowed:?} unshadowed={unshadowed:?}"
    );

    // 预算 100 m：全部 caster 都在预算内 ⇒ 与默认档逐像素相同、计数为 0。
    renderer.set_shadow_caster_max_distance(100.0);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let near = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback near");
    let near_metrics = renderer.last_frame_metrics();
    assert_eq!(
        near_metrics.shadow_distance_skipped, 0,
        "预算内不许跳：{near_metrics:?}"
    );
    assert_eq!(near, baseline, "预算内必须与默认档逐字节相同");

    // 还原成 0（关闭）：计数清零、像素回到默认。
    renderer.set_shadow_caster_max_distance(0.0);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let restored = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback restored");
    let restored_metrics = renderer.last_frame_metrics();
    assert_eq!(
        restored_metrics.shadow_distance_skipped, 0,
        "还原后不许再跳"
    );
    assert_eq!(
        restored_metrics.shadow_draws_drawn, baseline_metrics.shadow_draws_drawn,
        "还原后提交量必须回到默认"
    );
    assert_eq!(restored, baseline, "还原后像素必须逐字节回到默认");
    drop(texture);
}

/// P3-R256：距离预算同时掐点光 6 面链（与分类档同一条"只掐阴影"的形状）。
/// 抓的假绿：只改方向光链时点光计数恒 0——本门会红在「点光链必须记到距离跳过」。
#[test]
fn shadow_caster_distance_also_drops_point_shadow_casters() {
    let Some((device, queue)) = test_device("rustlua-point-caster-distance-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(2.0, 1.0, 6.0),
        Vec3::new(0.0, 0.0, -1.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light {
            direction: Vec3::new(0.0, 0.0, -1.0),
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            ..Light::default()
        },
        point_lights: vec![PointLight {
            position: Vec3::new(2.0, 2.0, 2.0),
            color: [1.0, 1.0, 1.0],
            intensity: 20.0,
            range: 30.0,
        }],
        drawables: vec![
            plane_drawable("ground", -2.0, 4.0, 4.0),
            plane_drawable("blocker", 0.0, 0.5, 0.1),
        ],
    };
    let mesh = empty_mesh();
    let (texture, view) = target(&device);
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_point_shadows(true),
    );

    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback baseline");
    let baseline = renderer.last_frame_metrics();
    assert!(
        baseline.point_shadows_active,
        "点光应触发立方体阴影：{baseline:?}"
    );
    assert_eq!(
        baseline.point_shadow_distance_skipped, 0,
        "默认档不许按距离掐点光 caster：{baseline:?}"
    );

    renderer.set_shadow_caster_max_distance(2.0);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback excluded");
    let excluded = renderer.last_frame_metrics();
    assert!(
        excluded.point_shadow_distance_skipped >= 1,
        "点光链必须记到距离跳过：{excluded:?}"
    );
    assert!(
        excluded.point_shadow_distance_skipped <= 6,
        "单盏灯最多 6 个面，跳过量不可能超过它：{excluded:?}"
    );
    assert!(
        excluded.shadow_distance_skipped >= 1,
        "方向光链的距离跳过必须同时生效：{excluded:?}"
    );

    renderer.set_shadow_caster_max_distance(0.0);
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    let _ = renderer
        .copy_texture_to_cpu(&texture, SIZE, SIZE)
        .expect("readback restored");
    let restored = renderer.last_frame_metrics();
    assert_eq!(
        restored.point_shadow_distance_skipped, 0,
        "还原后点光链不许再跳：{restored:?}"
    );
    assert_eq!(
        restored.shadow_distance_skipped, 0,
        "还原后方向光链不许再跳：{restored:?}"
    );
    drop(texture);
}
