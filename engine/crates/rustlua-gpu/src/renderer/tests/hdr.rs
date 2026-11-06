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
use rustlua_render::frame::{SceneDraw, SceneMesh, WHITE_TEXTURE_PATH, WorldSpriteVertex};
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
    assert!(
        corner[2] > corner[0] + 10,
        "天空角像素应偏蓝，实际 {corner:?}"
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
