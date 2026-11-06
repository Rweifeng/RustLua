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
