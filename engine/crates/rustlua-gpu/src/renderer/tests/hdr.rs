//! P3-R01：HDR 场景颜色 → Tonemap → 2D/UI 的像素级契约。
//!
//! 三条断言各自对应计划里的一条验收：
//!
//! 1. **目标只在需要时存在**：`策略 On + 能力可用 + 本帧有世界内容` 三者同时成立才创建
//!    `Rgba16Float` 目标；纯 2D/空帧即使策略打开也不建。
//! 2. **Tonemap 前 > 1、之后 ≤ 1**：世界 Sprite 的顶点色 8.0 在 HDR 目标里必须原样保留
//!    （这正是 `Rgba16Float` 与 8 位目标的分界），Tonemap 后的 8 位目标不可能越过上限。
//! 3. **UI 不属于场景色**：UI 画在 Tonemap **之后**，因此 HDR 开/关时同一个 UI 像素一致；
//!    且 UI 颜色不会出现在 HDR 目标里。
//!
//! 能力不可用的机器上像素用例显式 `gpu_skipped`（`RUSTLUA_REQUIRE_GPU=1` 时为硬失败），
//! 而不是把"没跑"记成通过。

use super::*;
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{
    BoundsSource, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh, WHITE_TEXTURE_PATH,
    WorldSpriteVertex,
};
use rustlua_render::layer::RenderLayer;
use rustlua_render::post_process::{PostProcess, Tonemap};
use rustlua_render::render_settings::{RenderSettings, SceneColorFormat, ToggleMode};
use rustlua_render::sprite_batch::SpriteDrawBatch;

const SIZE: u32 = 64;

fn renderer() -> Option<(Renderer, wgpu::Device)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-hdr-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    Some((renderer, device))
}

fn target(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-hdr-target"),
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
    (texture, view)
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

/// 世界坐标 ±0.5 的四边形。相机中心 0、缩放 = 目标边长时正好铺满整个目标。
fn world_quad(color: [f32; 4]) -> Vec<WorldSpriteVertex> {
    [
        (-0.5_f32, -0.5_f32),
        (0.5, -0.5),
        (-0.5, 0.5),
        (0.5, -0.5),
        (0.5, 0.5),
        (-0.5, 0.5),
    ]
    .into_iter()
    .map(|(x, y)| WorldSpriteVertex {
        position: [x, y, 0.0],
        uv: [0.5, 0.5],
        color,
    })
    .collect()
}

fn bright_sprite_mesh(color: [f32; 4]) -> SceneMesh {
    let mut mesh = empty_mesh();
    mesh.world_sprites = world_quad(color);
    // 不写深度 = 回滚档（未声明视图）的半透明分段。
    mesh.world_sprite_draws = vec![SpriteDrawBatch::translucent(
        0,
        0,
        RenderLayer::World2D,
        0,
        6,
    )];
    mesh.paths = vec![WHITE_TEXTURE_PATH.to_owned()];
    mesh
}

/// 屏幕右半边的 NDC 四边形（x ∈ [0, 1]），走 `ScreenUI` 层。
fn right_half_ui(color: [f32; 4]) -> Vec<OverlayVertex> {
    [
        (0.0_f32, -1.0_f32),
        (1.0, -1.0),
        (0.0, 1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (0.0, 1.0),
    ]
    .into_iter()
    .map(|(x, y)| OverlayVertex {
        position: [x, y],
        uv: [0.5, 0.5],
        color,
        sdf: 0,
    })
    .collect()
}

fn set_camera(renderer: &mut Renderer, size: (u32, u32)) {
    let viewport = Viewport::new(size.0 as f32, size.1 as f32).expect("viewport");
    let camera = Camera2D::new(Vec2::new(0.0, 0.0), size.0.max(size.1) as f32).expect("camera2d");
    renderer.set_world_sprite_camera(
        camera
            .view_projection(viewport, -1.0, 1.0)
            .expect("view projection"),
    );
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

/// SSAO 只在 HDR Tonemap 路径实际执行；关闭时指标必须为 false，且不产生新的通道。
#[test]
fn ssao_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        !renderer.last_frame_metrics().ssao_active,
        "默认关闭时不得报告 SSAO 生效"
    );

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_ssao(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().ssao_active,
        "HDR + SSAO 打开后必须报告生效"
    );
}

/// SSR 必须真的从深度重建世界坐标、用相机矩阵投回屏幕；且循环要定长，
/// 否则 FXC 会以 `unable to unroll loop` 拒绝整条 Tonemap 管线。
#[test]
fn tonemap_shader_reconstructs_world_space_ssr() {
    let source = include_str!("../../../shaders/ssr.wgsl");
    assert!(source.contains("inverse_view_projection"));
    assert!(source.contains("world_from_depth"));
    assert!(source.contains("params.view_projection * vec4<f32>(sample"));
    assert!(source.contains("textureSampleLevel(source, source_sampler, sample_uv, 0.0)"));
    assert!(source.contains("for (var step: i32 = 1; step <= 10; step = step + 1)"));
    assert!(!source.contains("break;"), "SSR 循环必须定长，不能 break");
    assert!(!source.contains("continue;"), "SSR 循环不能 continue");
}

/// SSR 双边模糊必须按深度差加权，且 8 个邻域都读反射图。
#[test]
fn ssr_blur_shader_weights_reflections_by_depth() {
    let source = include_str!("../../../shaders/ssr_blur.wgsl");
    assert!(source.contains("exp(-abs(sample_depth - center_depth) * 240.0)"));
    assert!(source.contains("array<vec2<f32>, 8>"));
    assert!(source.contains("textureLoad(ssr_source"));
    assert!(source.contains("prev_view_projection"));
    assert!(source.contains("textureSampleLevel(ssr_history"));
    assert!(source.contains("abs(previous_scene_depth - center_depth) > 0.01"));
}

/// P3-R176：未命中/出屏的反射必须回退到程序化天空的方向渐变，而不是黑。回退用的渐变
/// 常量必须与 Tonemap 的 `sky_color` 逐字一致——两份硬编码各写一套就是两个天空，
/// 反射里的地平线会和天上对不上。
#[test]
fn ssr_misses_fall_back_to_the_procedural_sky() {
    let ssr = include_str!("../../../shaders/ssr.wgsl");
    let tonemap = include_str!("../../../shaders/tonemap.wgsl");
    let host = include_str!("../passes.rs");
    assert!(
        ssr.contains("    sky: vec4<f32>,"),
        "参数末尾要加 vec4：xyz = 指向太阳，w = 太阳强度（兼天空开关）"
    );
    assert!(
        ssr.contains("return sky_reflection(normal, view, reflected, world);"),
        "10 步走完还没命中时必须回退天空：这里原来返回 vec4(0.0)，反射只能是黑的"
    );
    assert!(
        ssr.contains("if params.sky.w <= 0.0 || direction.y <= 0.0 {"),
        "天空关闭（太阳强度 0）或反射线朝下时必须保持零，不能凭空加一层天空色"
    );
    assert_eq!(
        ssr.matches("return vec4<f32>(0.0);").count(),
        2,
        "只剩两条「必须黑」的路径：中心像素是天空（depth=1）、天空回退被关闭或射线朝下"
    );
    for constant in [
        "let zenith = vec3<f32>(0.055, 0.14, 0.34);",
        "let horizon = vec3<f32>(0.55, 0.68, 0.80);",
        "mix(vec3<f32>(0.34, 0.38, 0.41), color, smoothstep(-0.18, 0.01,",
    ] {
        assert!(
            ssr.contains(constant) && tonemap.contains(constant),
            "天空回退与 `sky_color` 必须共用同一组渐变常量：{constant}"
        );
    }
    assert!(
        ssr.contains("pow(height, 0.55)") && host.contains("params[58] = 0.55;"),
        "渐变指数也要与宿主写给 Tonemap 的 0.55 一致"
    );
    assert!(
        ssr.contains("reflected_color * reflection_weight(normal, view)")
            && ssr.contains("color * reflection_weight(normal, view)"),
        "命中与天空回退必须共用同一个菲涅耳权重，否则接缝上反射强度会跳变"
    );
    assert!(
        ssr.contains("const SUN_REFLECTION_CAP: f32 = 0.25;")
            && ssr.contains("min(sun_term, SUN_REFLECTION_CAP)"),
        "太阳项必须有上限：1400 次幂的盘心在半分辨率 + 3×3 双边上会摊成白斑"
    );

    let host = include_str!("../passes.rs");
    assert!(
        host.contains("const SKY_SUN_INTENSITY: f32 = 2.4;")
            && host.contains("params[57] = SKY_SUN_INTENSITY;")
            && host.contains("params[39] = SKY_SUN_INTENSITY;"),
        "太阳强度只能有一个真值：Tonemap 与 SSR 回退各写一份就会对不上"
    );
    assert!(
        host.contains("let mut params = [0.0_f32; 44];")
            && host.contains("params[36] = -direction.x / length;")
            && host.contains("params[42] = self.clock_secs();"),
        "SSR uniform 44 槽：36..40 天空方向/强度，40..44 云量/云速/时间/云底；\
         时间必须取 `clock_secs()` 这个单一取值点（P3-R207），否则测试钉不住时钟"
    );
    assert!(
        ssr.contains("let density = ssr_cloud_density(")
            && ssr
                .contains("let t = (clamp(params.clouds.w, 40.0, 200.0) - world.y) / direction.y;"),
        "回退的云遮挡要按反射线打到云底平面，采样点只能是 world + direction * t"
    );
    assert!(
        ssr.contains("let cloud_lit = mix(") && ssr.contains("+ cloud_lit * density * 0.14;"),
        "云要回射自己的受光色（P3-R179）：只做透射衰减会让深云区变成比周围更蓝的洞；\
         回射权重 0.14 必须小于遮挡的 0.85，净能量才会随云量下降"
    );
    let construct = include_str!("../construct.rs");
    assert!(
        construct.contains("min_binding_size: wgpu::BufferSize::new(176),")
            && construct.contains("label: Some(\"rustlua-ssr-params\"),\n            size: 176,"),
        "uniform 缓冲与布局下限都要跟着 44 个 f32（176 B）走"
    );
}

/// 景深只在 HDR Tonemap 路径实际执行。
#[test]
fn dof_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(!renderer.last_frame_metrics().dof_active);

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_dof(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().dof_active);
}

/// 景深必须按深度 CoC 做 8 方向采样。
#[test]
fn tonemap_shader_reconstructs_depth_of_field() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn depth_of_field"));
    assert!(source.contains("params.dof.y"));
    assert!(source.contains("dirs[i] * texel * radius"));
}

/// 屏幕空间神光只在 HDR Tonemap 路径实际执行。
#[test]
fn god_rays_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(!renderer.last_frame_metrics().god_rays_active);

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_god_rays(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().god_rays_active);
}

/// 神光必须从太阳屏幕位置沿像素方向累积，且屏外时直接跳过。
#[test]
fn tonemap_shader_projects_sun_for_god_rays() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn god_rays_at"));
    assert!(source.contains("sun_uv"));
    assert!(source.contains("params.god_rays"));
    assert!(source.contains("for (var i: i32 = 0; i < 12; i = i + 1)"));
    assert!(source.contains("sun_distance"));
    assert!(source.contains("smoothstep(0.045, 0.012, sun_distance)"));
}

/// 距离雾只在 HDR Tonemap 路径实际执行。
#[test]
fn fog_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(!renderer.last_frame_metrics().fog_active);

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_fog(true)
            .with_fog_density(0.02),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().fog_active);
}

/// 雾必须按相机距离与密度执行，并在天空像素跳过。
#[test]
fn tonemap_shader_reconstructs_distance_fog() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn fog_at"));
    assert!(source.contains("params.fog.x * distance"));
    assert!(source.contains("mix(linear, params.fog.yzw"));
}

/// 低空湿雾在距离雾之上按世界高度衰减，并且只在远景渐入（P3-R47）。
#[test]
fn tonemap_shader_adds_height_weighted_mist() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("let ground = exp(-max(world.y - 0.5, 0.0) * 0.35)"));
    assert!(source.contains("let far = smoothstep(40.0, 220.0, distance)"));
    assert!(source.contains("* ground * 0.18 * far"));
    assert!(source.contains("clamp(base + mist, 0.0, 0.94)"));
}

/// 天空只在 HDR + 雾 + 有场景相机的帧替换 depth=1 背景，且背景像素真的变蓝。
#[test]
fn sky_fills_hdr_background_when_fog_is_on() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (target, view) = target(&device, SIZE, SIZE);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let scene_view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(scene_view),
        overlay: None,
    });
    let scene = Scene3d {
        camera,
        view: scene_view,
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: Vec::new(),
    };
    let mesh = bright_sprite_mesh([2.0, 2.0, 2.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(!renderer.last_frame_metrics().sky_active);

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_fog(true)
            .with_fog_density(0.01),
    );
    renderer.render_into_texture(&mesh, Some(&scene), &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().sky_active);
    let pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("sky readback");
    let corner = pixel(&pixels, SIZE, 0, 0);
    let brightness = i32::from(corner[0]) + i32::from(corner[1]) + i32::from(corner[2]);
    assert!(
        brightness > 30,
        "天空角像素应被程序化天空/云层填充，实际 {corner:?}"
    );
}

/// 天空必须由逆视图投影重建相机射线，并按 depth=1 替换背景。
#[test]
fn tonemap_shader_reconstructs_a_procedural_sky() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn sky_color"));
    assert!(source.contains("params.sky.x"));
    assert!(source.contains("depth_at(uv) >= 0.9999"));
    assert!(source.contains("world_from_depth(uv, 1.0)"));
    assert!(source.contains("fn tonemap_volumetric_clouds"));
    assert!(source.contains("let step_count = 8"));
    assert!(source.contains("tonemap_cloud_density(p + to_sun"));
    assert!(source.contains("tonemap_volumetric_clouds(ray, to_sun, time, uv)"));
    assert!(source.contains("let t0_jittered = t0 + jitter * dt"));
    assert!(source.contains("fn output_dither"));
    assert!(source.contains("output_dither(uv)"));
    assert!(source.contains("P3-R140"), "缺少暗角/颗粒");
    assert!(source.contains("color * (1.0 - clouds.a * 0.85) + clouds.rgb"));
}

/// 水面物体反射：解析水面求交 + 屏幕空间步进，且只在水面像素叠加（P3-R52）。
#[test]
fn tonemap_shader_adds_screen_space_water_reflection() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn water_reflection"));
    assert!(source.contains("params.water_params.x"));
    assert!(source.contains("abs(hit.x - center.x) > half"));
    assert!(source.contains("for (var step: i32 = 0; step < 18; step = step + 1)"));
    assert!(source.contains("distorted + texel * vec2<f32>(2.0, 0.0)"));
    assert!(source.contains("fn water_refraction"));
    assert!(source.contains("let refracted = water_refraction(uv, params.camera_position.w)"));
    assert!(source.contains("offset * 1.08"));
    assert!(source.contains("offset * 0.92"));
    assert!(source.contains("linear = mix(linear, refracted, 0.30)"));
    // P3-R171 起水像素判定在 Tonemap 里只做一次，折射/吸收/反射共用同一个 `surface`。
    assert!(source.contains("let screen = water_reflection(uv, surface);"));
    assert!(
        source.contains("linear = linear + select(screen, planar, length(planar) > 0.001) * 0.65")
    );
}

/// 接触阴影只在 HDR Tonemap 路径实际执行。
#[test]
fn contact_shadows_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        !renderer.last_frame_metrics().contact_shadows_active,
        "默认关闭时不得报告接触阴影生效"
    );

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_contact_shadows(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().contact_shadows_active,
        "HDR + 接触阴影打开后必须报告生效"
    );
}

/// 接触阴影必须沿“指向光”的方向推进，并且循环定长（FXC 约束与 SSR 相同）。
#[test]
fn tonemap_shader_marches_contact_shadows_toward_the_light() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("light_direction: vec4<f32>"));
    assert!(source.contains("fn contact_shadow_at"));
    assert!(source.contains("normalize(to_light)"));
    assert!(source.contains("for (var step: i32 = 1; step <= 8; step = step + 1)"));
}

/// SSR-lite 同样只在 HDR Tonemap 路径实际执行。
#[test]
fn ssr_flag_is_reported_only_on_the_hdr_path() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        !renderer.last_frame_metrics().ssr_active,
        "默认关闭时不得报告 SSR 生效"
    );

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_ssr(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().ssr_active,
        "HDR + SSR 打开后必须报告生效"
    );
}

/// 启用条件的三方对账：`metrics.hdr_active`、计划里的 `Tonemap`、实际附件格式与
/// `hdr_scene_size` 必须指向同一个事实；纯 2D/空帧不创建目标。
#[test]
fn hdr_target_exists_only_when_policy_capability_and_world_content_agree() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    // 回滚档：策略 Off（默认），世界内容再多也不建 HDR 目标。
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    assert!(!metrics.hdr_active, "默认 RenderSettings 必须保持 LDR");
    assert_eq!(metrics.scene_format, SceneColorFormat::Rgba8UnormSrgb);
    assert!(!renderer.last_frame_plan().contains(RenderPass::Tonemap));
    assert!(
        renderer.read_scene_color_to_cpu().is_err(),
        "默认档不得创建 HDR 目标"
    );

    // 策略 On + 空帧：没有世界内容就不该付一次 16F 附件与全屏 Tonemap 的代价。
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&empty_mesh(), None, &view, SIZE, SIZE, true);
    assert!(
        !renderer.last_frame_metrics().hdr_active,
        "纯 2D/空帧即使策略打开也不走 HDR"
    );
    assert!(
        renderer.read_scene_color_to_cpu().is_err(),
        "空帧不得创建 HDR 目标"
    );

    // 策略 On + 世界内容：能力是最后的闸门，三个读数必须一致。
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    let usable = renderer.capabilities().hdr_usable();
    assert_eq!(
        metrics.hdr_active, usable,
        "HDR 是否启用必须严格等于能力位（能力={usable}）"
    );
    if !usable {
        assert_eq!(metrics.scene_format, SceneColorFormat::Rgba8UnormSrgb);
        assert!(!renderer.last_frame_plan().contains(RenderPass::Tonemap));
        assert!(renderer.read_scene_color_to_cpu().is_err());
        return;
    }
    assert_eq!(metrics.scene_format, SceneColorFormat::Rgba16Float);
    assert_eq!(
        metrics.tonemap,
        Some(Tonemap::Aces),
        "没有后处理设置时按默认 ACES 报告"
    );
    assert!(
        renderer.last_frame_plan().contains(RenderPass::Tonemap),
        "HDR 帧的计划里必须有 Tonemap"
    );
    assert_eq!(renderer.hdr_scene_size(), (SIZE, SIZE));
    let scene = renderer
        .read_scene_color_to_cpu()
        .expect("HDR scene readback");
    assert_eq!((scene.width, scene.height), (SIZE, SIZE));
    assert!(
        scene.max_rgb() > 7.9,
        "8.0 的线性高光在 Tonemap 前必须仍 > 1（说明没有被夹成 LDR）：max={}",
        scene.max_rgb()
    );
}

/// Tonemap 前有 > 1 的线性值；Tonemap 后 8 位目标不越界；UI 在 Tonemap 之后画，
/// 因此 HDR 开/关的同一 UI 像素一致，且 UI 颜色不会写进 HDR 目标。
#[test]
fn tonemap_consumes_hdr_and_leaves_ui_untouched() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_capability");
    }
    let (target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));

    // 世界：8.0 的白色精灵铺满全屏；UI：右半边纯红。
    let mut mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);
    mesh.vertices = right_half_ui([1.0, 0.0, 0.0, 1.0]);
    mesh.draws = vec![SceneDraw::screen_ui(0, 0, 6)];

    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(renderer.last_frame_metrics().hdr_active);

    let scene = renderer
        .read_scene_color_to_cpu()
        .expect("HDR scene readback");
    assert!(
        scene.max_rgb() > 7.9,
        "Tonemap 前的高光必须 > 1：max={}",
        scene.max_rgb()
    );
    // UI 不在 HDR 目标里：右半边在 Tonemap 前仍是 8.0 的世界色，不是 UI 的纯红。
    let scene_right = scene.pixel(SIZE - 8, SIZE / 2).expect("scene pixel");
    assert!(
        scene_right[0] > 7.9 && scene_right[1] > 7.9,
        "ScreenUI 必须在 Tonemap 之后画，不得进入 HDR 场景目标：{scene_right:?}"
    );

    let hdr_pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("HDR frame readback");
    let world_centre = pixel(&hdr_pixels, SIZE, SIZE / 4, SIZE / 2);
    let ui_centre = pixel(&hdr_pixels, SIZE, SIZE * 3 / 4, SIZE / 2);
    assert!(
        world_centre[0] >= 250 && world_centre[1] >= 250 && world_centre[2] >= 250,
        "8.0 经 ACES 应压到接近白：{world_centre:?}"
    );
    assert!(
        ui_centre[0] >= 254 && ui_centre[1] <= 2 && ui_centre[2] <= 2,
        "UI 纯红必须原样保留（不被 ACES 改色）：{ui_centre:?}"
    );

    // 同一帧改回 LDR：UI 像素必须与 HDR 路径一致（差距只允许量化误差）。
    renderer.set_render_settings(RenderSettings::default());
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(!renderer.last_frame_metrics().hdr_active);
    let ldr_pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("LDR frame readback");
    let ui_ldr = pixel(&ldr_pixels, SIZE, SIZE * 3 / 4, SIZE / 2);
    for channel in 0..4 {
        let delta = ui_centre[channel].abs_diff(ui_ldr[channel]);
        assert!(
            delta <= 1,
            "UI 像素必须绕过 Tonemap（通道 {channel}: HDR={} LDR={}，差 {delta}）",
            ui_centre[channel],
            ui_ldr[channel]
        );
    }
}

/// HDR 与后处理共存：`exposure == 1.0` 时 Tonemap 在前、PostProcess 最后；
/// `exposure != 1.0` 时 HDR 主动让位（曝光只走一条路），计划里不得有 Tonemap。
#[test]
fn hdr_composes_with_post_process_and_rejects_double_exposure() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_capability");
    }
    let (_target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    let mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);

    // 双重曝光：HDR 让位，但后处理照常执行。
    renderer.set_post_process(Some(
        PostProcess::new(2.0, [1.0; 4], 0.0).expect("valid post process"),
    ));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        !renderer.last_frame_metrics().hdr_active,
        "PostProcess.exposure != 1.0 时必须回退 LDR，避免曝光乘两次"
    );
    assert!(!renderer.last_frame_plan().contains(RenderPass::Tonemap));
    assert!(renderer.last_frame_plan().contains(RenderPass::PostProcess));

    // 曝光归属正确：EV 交给 Tonemap，谱通后处理仍在最后。
    renderer.set_post_process(Some(
        PostProcess::new_hdr(Tonemap::Reinhard, 1.0, [1.0; 4], 0.0).expect("valid HDR settings"),
    ));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let metrics = renderer.last_frame_metrics();
    assert!(metrics.hdr_active);
    assert_eq!(metrics.tonemap, Some(Tonemap::Reinhard));
    let passes = renderer.last_frame_plan().passes();
    let tonemap_at = passes
        .iter()
        .position(|pass| *pass == RenderPass::Tonemap)
        .expect("HDR + PostProcess 帧必须有 Tonemap");
    assert!(
        tonemap_at < passes.len() - 1,
        "Tonemap 必须在 PostProcess 之前：{passes:?}"
    );
    assert_eq!(passes.last(), Some(&RenderPass::PostProcess));
}

/// HDR 泛光：同一张亮精灵，开 bloom 后精灵外的像素必须被提亮；关 bloom 时保持基线。
#[test]
fn hdr_bloom_bleeds_bright_energy_outside_the_sprite() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_capability");
    }
    let (target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    let mut mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);
    // 缩小成中心亮斑，周围留出暗背景，才能看见溢光。
    for v in &mut mesh.world_sprites {
        v.position[0] *= 0.2;
        v.position[1] *= 0.2;
    }

    renderer.set_post_process(Some(
        PostProcess::default().with_bloom(0.0).expect("bloom off"),
    ));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let base = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("baseline frame readback");

    renderer.set_post_process(Some(
        PostProcess::default().with_bloom(0.8).expect("bloom on"),
    ));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let bloom = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("bloom frame readback");

    // 不假设亮斑具体覆盖到哪一列：逐像素找最大增量，只要暗背景被提亮就证明
    // 8 方向亮部采样真的跑在 Tonemap 之前。
    let mut max_delta = 0_u8;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let b = pixel(&base, SIZE, x, y);
            let l = pixel(&bloom, SIZE, x, y);
            for channel in 0..3 {
                max_delta = max_delta.max(l[channel].saturating_sub(b[channel]));
            }
        }
    }
    assert!(
        max_delta >= 2,
        "bloom 必须在亮斑外产生可见溢光：max_delta={max_delta}"
    );

    // 场景级开关也必须真的进 Tonemap pass：不设 PostProcess，只把
    // RenderSettings.bloom 打开，与同一帧的 bloom=false 逐像素比较。
    renderer.set_post_process(None);
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let scene_base = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("scene bloom baseline");
    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_bloom(true),
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().bloom_active,
        "泛光帧必须把 bloom_active 报成 true"
    );
    let scene_bloom = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("scene bloom frame");
    let mut scene_delta = 0_u8;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let b = pixel(&scene_base, SIZE, x, y);
            let l = pixel(&scene_bloom, SIZE, x, y);
            for channel in 0..3 {
                scene_delta = scene_delta.max(l[channel].saturating_sub(b[channel]));
            }
        }
    }
    assert!(
        scene_delta >= 2,
        "RenderSettings.bloom 必须打开 Tonemap 泛光：max_delta={scene_delta}"
    );
}

/// FXAA-lite 必须只在开关打开时改变高对比边缘，关闭时保持基线。
#[test]
fn hdr_fxaa_smooths_high_contrast_edges() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_capability");
    }
    let (target, view) = target(&device, SIZE, SIZE);
    set_camera(&mut renderer, (SIZE, SIZE));
    let mut mesh = bright_sprite_mesh([8.0, 8.0, 8.0, 1.0]);
    for v in &mut mesh.world_sprites {
        v.position[0] *= 0.2;
        v.position[1] *= 0.2;
    }

    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    let base = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("fxaa baseline");

    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_fxaa(true),
    );
    assert!(
        renderer.render_settings().fxaa,
        "FXAA 开关必须已写入渲染设置"
    );
    renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(
        renderer.last_frame_metrics().fxaa_active,
        "FXAA 帧必须把 fxaa_active 报成 true"
    );
    let smoothed = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("fxaa frame");

    let mut max_delta = 0_u8;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let b = pixel(&base, SIZE, x, y);
            let s = pixel(&smoothed, SIZE, x, y);
            for channel in 0..3 {
                max_delta = max_delta.max(s[channel].abs_diff(b[channel]));
            }
        }
    }
    assert!(
        max_delta >= 2,
        "FXAA 必须在高对比边缘产生可见平滑：max_delta={max_delta}"
    );
}

/// 尺寸变化：HDR 目标随目标附件重建，旧尺寸不再被沿用。
#[test]
fn hdr_scene_target_rebuilds_on_resize() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    if !renderer.capabilities().hdr_usable() {
        return gpu_skipped("hdr_capability");
    }
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    let mesh = bright_sprite_mesh([2.0, 2.0, 2.0, 1.0]);

    let (small, small_view) = target(&device, 800, 600);
    set_camera(&mut renderer, (800, 600));
    renderer.render_into_texture(&mesh, None, &small_view, 800, 600, true);
    assert!(renderer.last_frame_metrics().hdr_active);
    assert_eq!(renderer.hdr_scene_size(), (800, 600));
    let scene = renderer.read_scene_color_to_cpu().expect("small readback");
    assert_eq!((scene.width, scene.height), (800, 600));

    let (large, large_view) = target(&device, 1280, 720);
    set_camera(&mut renderer, (1280, 720));
    renderer.render_into_texture(&mesh, None, &large_view, 1280, 720, true);
    assert!(renderer.last_frame_metrics().hdr_active);
    assert_eq!(
        renderer.hdr_scene_size(),
        (1280, 720),
        "目标尺寸变化后 HDR 附件必须重建"
    );
    assert_eq!(
        renderer.hdr_scene_texture.as_ref().map(|texture| {
            let size = texture.size();
            (size.width, size.height)
        }),
        Some((1280, 720)),
        "HDR 纹理的实际尺寸必须跟上目标"
    );
    let scene = renderer.read_scene_color_to_cpu().expect("large readback");
    assert_eq!((scene.width, scene.height), (1280, 720));
    drop((small, large));
}

/// 画面下方中点的像素：相机在 `(0,10,20)` 俯视，这一行射线穿过 `z=0` 时都在地下。
const SECTION_PROBE_ROW: u32 = SIZE * 7 / 8;

/// 渲染一帧单块地形，返回画面下方的探针像素。
fn render_terrain_probe(
    renderer: &mut Renderer,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
    camera: Camera3D,
    scene_view: RenderView,
    path: &str,
) -> [u8; 4] {
    let scene = Scene3d {
        camera,
        view: scene_view,
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Terrain".to_owned(),
            mesh_path: path.to_owned(),
            material: None,
            mesh_handle: None,
            model: Mat4::identity(),
            palette: Vec::new(),
            instances: Vec::new(),
            bounds: Scene3dBounds {
                local_aabb: Aabb3 {
                    min: Vec3::new(-40.0, -2.0, -40.0),
                    max: Vec3::new(40.0, 2.0, 40.0),
                },
                source: BoundsSource::Fallback,
            },
        }],
    };
    let overlay = empty_mesh();
    renderer.render_into_texture(&overlay, Some(&scene), view, SIZE, SIZE, true);
    let pixels = renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("section readback");
    pixel(&pixels, SIZE, SIZE / 2, SECTION_PROBE_ROW)
}

fn max_channel_delta(a: [u8; 4], b: [u8; 4]) -> u8 {
    (0..3)
        .map(|channel| a[channel].abs_diff(b[channel]))
        .max()
        .unwrap_or(0)
}

/// P3-R166：`sect=1` 地形 + 相机裁剪平面 ⇒ Tonemap 把切口封成实体土壤。
///
/// 反面对照有两条，都在**像素**上判：
///
/// 1. 同一场景、同一裁剪平面，只是网格路径去掉 `sect=1`：缺口回到清屏深蓝，
///    证明暖色土壤不是清屏色或相机的副产物。
/// 2. 同一张 `sect=1` 地形，把裁剪平面清零：封口必须消失，证明这段填充由平面触发，
///    而不是地形着色器自己画出来的。
#[test]
fn section_cut_fills_the_clipped_terrain_hole() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (target_texture, target_view) = target(&device, SIZE, SIZE);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let mut camera = Camera3D::perspective(
        Vec3::new(0.0, 10.0, 20.0),
        Vec3::new(0.0, 0.0, -4.0),
        std::f32::consts::FRAC_PI_3,
        0.1,
        120.0,
    )
    .expect("camera");
    // `n·p + d >= 0` 保留 z <= 0 的一侧：相机与平面之间的近半张地形整块被裁掉。
    camera.clip_plane = [0.0, 0.0, -1.0, 0.0];
    let scene_view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(scene_view),
        overlay: None,
    });
    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_pbr(true),
    );
    let with_section = render_terrain_probe(
        &mut renderer,
        &target_texture,
        &target_view,
        camera,
        scene_view,
        "__terrain__?size=80&res=48&amp=2&seed=3&sect=1",
    );
    let without_section = render_terrain_probe(
        &mut renderer,
        &target_texture,
        &target_view,
        camera,
        scene_view,
        "__terrain__?size=80&res=48&amp=2&seed=3",
    );

    let mut unclipped_camera = camera;
    unclipped_camera.clip_plane = [0.0; 4];
    let unclipped_view = RenderView::main_3d(unclipped_camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(unclipped_view),
        overlay: None,
    });
    let without_plane = render_terrain_probe(
        &mut renderer,
        &target_texture,
        &target_view,
        unclipped_camera,
        unclipped_view,
        "__terrain__?size=80&res=48&amp=2&seed=3&sect=1",
    );

    assert!(
        with_section[0] > with_section[2] + 15,
        "切口应被暖色土壤填满：探针 {with_section:?}（无 sect 对照 {without_section:?}）"
    );
    assert!(
        without_section[2] > without_section[0],
        "去掉 sect=1 后同一像素应回到深蓝清屏色：{without_section:?}"
    );
    let delta = max_channel_delta(with_section, without_plane);
    assert!(
        delta > 20,
        "裁剪平面清零后封口必须消失：{with_section:?} vs {without_plane:?}，max_delta={delta}"
    );
}

/// P3-R203：地形 `cheap=1` 必须真的跳过近细节——像素级两条：
/// 1. `cheap=0` 与缺省路径逐字节一致（默认不变）；
/// 2. `cheap=1` 必须改变输出（视差/裂缝 AO/太阳微阴影里至少一项生效）。
#[test]
fn terrain_cheap_skips_near_detail_pixels() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (target_texture, target_view) = target(&device, SIZE, SIZE);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 6.0, 14.0),
        Vec3::new(0.0, 0.0, -2.0),
        std::f32::consts::FRAC_PI_3,
        0.1,
        120.0,
    )
    .expect("camera");
    let scene_view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(scene_view),
        overlay: None,
    });
    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_pbr(true),
    );
    let render = |renderer: &mut Renderer, path: &str| {
        let scene = Scene3d {
            camera,
            view: scene_view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables: vec![Scene3dDrawable {
                entity: rustlua_scene::scene::SceneEntityId(1),
                name: "Terrain".to_owned(),
                mesh_path: path.to_owned(),
                material: None,
                mesh_handle: None,
                model: Mat4::identity(),
                palette: Vec::new(),
                instances: Vec::new(),
                bounds: Scene3dBounds {
                    local_aabb: Aabb3 {
                        min: Vec3::new(-40.0, -2.0, -40.0),
                        max: Vec3::new(40.0, 2.0, 40.0),
                    },
                    source: BoundsSource::Fallback,
                },
            }],
        };
        let overlay = empty_mesh();
        renderer.render_into_texture(&overlay, Some(&scene), &target_view, SIZE, SIZE, true);
        renderer
            .copy_texture_to_cpu(&target_texture, SIZE, SIZE)
            .expect("terrain readback")
    };
    // detail=64：瓦片大 ⇒ 纹素足迹小 ⇒ lod≈0，近细节分支一定参与；否则 cheap 无从生效。
    let base = "__terrain__?size=80&res=96&amp=2&seed=3&detail=64";
    let default_pixels = render(&mut renderer, base);
    let explicit_off = render(&mut renderer, &format!("{base}&cheap=0"));
    let cheap_pixels = render(&mut renderer, &format!("{base}&cheap=1"));
    assert_eq!(
        default_pixels, explicit_off,
        "cheap=0 必须与缺省路径逐字节一致"
    );
    let changed = default_pixels
        .iter()
        .zip(&cheap_pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        changed > 0,
        "cheap=1 必须改变近细节像素（changed={changed}）"
    );
}

/// P3-R277：普通网格廉价着色档（`--crowd-cheap`）的像素级门：
/// 1. 关档必须逐字节回到缺省路径（默认不变）；
/// 2. 开档必须改变输出——砍掉的是点光循环/级联阴影/SH9/镜面 IBL，
///    用一盏点光把地面照亮，关档后该处必须变暗。
#[test]
fn crowd_cheap_override_changes_generic_mesh_pixels() {
    let Some((mut renderer, device)) = renderer() else {
        return gpu_skipped("request_adapter");
    };
    let (target_texture, target_view) = target(&device, SIZE, SIZE);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 6.0, 12.0),
        Vec3::new(0.0, 0.0, 0.0),
        std::f32::consts::FRAC_PI_3,
        0.1,
        120.0,
    )
    .expect("camera");
    let scene_view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(scene_view),
        overlay: None,
    });
    renderer.set_render_settings(
        RenderSettings::default()
            .with_hdr(ToggleMode::On)
            .with_pbr(true),
    );
    let scene = Scene3d {
        camera,
        view: scene_view,
        light: Light::default(),
        point_lights: vec![PointLight {
            position: Vec3::new(0.0, 2.0, 0.0),
            color: [1.0, 1.0, 1.0],
            intensity: 40.0,
            range: 20.0,
        }],
        drawables: vec![Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: rustlua_render::mesh3d::GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            mesh_handle: None,
            model: rustlua_math::math3d::Mat4::identity(),
            palette: Vec::new(),
            instances: Vec::new(),
            bounds: Scene3dBounds {
                local_aabb: Aabb3 {
                    min: Vec3::new(-4.0, -0.1, -4.0),
                    max: Vec3::new(4.0, 0.1, 4.0),
                },
                source: BoundsSource::Fallback,
            },
        }],
    };
    let overlay = empty_mesh();
    let render = |renderer: &mut Renderer| {
        renderer.render_into_texture(&overlay, Some(&scene), &target_view, SIZE, SIZE, true);
        renderer
            .copy_texture_to_cpu(&target_texture, SIZE, SIZE)
            .expect("crowd cheap readback")
    };
    let default_pixels = render(&mut renderer);
    renderer.set_crowd_cheap_override(true);
    let cheap_pixels = render(&mut renderer);
    renderer.set_crowd_cheap_override(false);
    let restored = render(&mut renderer);
    assert_eq!(default_pixels, restored, "关档必须与缺省路径逐字节一致");
    let changed = default_pixels
        .iter()
        .zip(&cheap_pixels)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        changed > 0,
        "开档必须改变像素（点光/IBL 被砍，changed={changed}）"
    );
}
