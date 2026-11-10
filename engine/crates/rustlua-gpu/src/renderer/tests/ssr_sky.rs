//! P3-R176：SSR 未命中/出屏的天空回退门。
//!
//! 做法：在 `ssr.wgsl` 里，10 步推进没命中或射线出了屏幕时不再返回零，而是按**反射方向**
//! 取程序化天空的同一组渐变常量（与 Tonemap 的 `sky_color` 逐字一致），权重是命中路径
//! 同一个菲涅耳函数；`params.sky.w`（太阳强度）同时是开关，天空关闭时整段回退失效。
//!
//! 场景：160²、`__ground_plane__` 放大 4 倍（32 m 见方）、眼位 (0,2,6) 平视，地平线正好在
//! 中间行、地面的最远边在 ~row 89。地面朝向观察者，反射线全部朝上且**屏幕内没有可以命中
//! 的几何**——这一帧的反射全部来自天空回退。太阳放在相机背后（方向 y 向下、z 朝相机），
//! 反射线一律朝 −z，`dot(reflected, to_sun) < 0`，太阳盘与光晕都不进画面：量的是渐变本身。
//!
//! * **未命中不再是黑**：SSR 开与关的帧在地面区（row 96..160）必须有大批像素不同；
//! * **加的是天空**：被改动的像素里蓝通道增量必须大于红通道（天空地平线是 0.55/0.68/0.80）；
//! * **天空自己不动**：row 0..84 一个像素都不许变（中心深度 1.0 在片元开头就返回零）；
//! * 反面对照：把宿主 `params[39]` 固定成 0.0（天空回退关闭）⇒ 地面区变回与 SSR 关一致，
//!   第一条断言必须红。这条对照证明的是**开关真的接在写入的槽位上**，而不是靠常量巧合。
//!
//! 这条门还钉住了半分辨率 UV：SSR 目标只有深度图的 1/2 边长，若用 `@builtin(position)`
//! 除以深度图尺寸（全分辨率），整屏反射只会采样左上四分之一——在本场景里那一块全是天空，
//! 回退一个像素都不会触发，第一条断言直接红。
//!
//! P3-R177 追加：把太阳放到相机前方、仰角 30°（`direction = (0, −0.5, 0.866)`），反射线
//! 正好扫过太阳盘。1400 次幂的盘心只有几像素宽，半分辨率 + 3×3 双边会把它摊成一条亮带 ——
//! `SUN_REFLECTION_CAP` 把整条太阳项夹在 0.25，地面最亮通道 180；去掉上限（反面对照）读
//! 233，断言 `brightest < 210` 落在两者之间。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 160;
/// 判「变了」的像素阈值（0..255）。SSR 关/开两帧的地面着色完全相同，实测差分是 0，
/// 阈值取 4 只用来滤掉 8 位舍入。
const DIFF_THRESHOLD: u8 = 4;
/// 地平线以上完全无地面的行：SSR 在这些行一个像素都不许改。
const SKY_ROWS: std::ops::Range<u32> = 0..84;
/// 整行都是地面（32 m 见方的平面在 row 96 处横向覆盖 ±12.8 m，够满宽）的行。
const FLOOR_ROWS: std::ops::Range<u32> = 96..160;

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

fn ground_drawable() -> Scene3dDrawable {
    // 哨兵网格本身在 y=0 的水平面上（−4..4 的 xz 四边形），放大 4 倍 ⇒ 32 m 见方；
    // 不需要像点光门那样绕 X 轴转 90°（那里是要把地面转成墙）。
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(9),
        name: "Ground".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::scale(Vec3::new(4.0, 1.0, 4.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-4.0, -0.1, -4.0),
                max: Vec3::new(4.0, 0.1, 4.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

/// 一帧：平视地面，返回 LDR 像素。`ssr` 决定这条门要对比的那一维；`sun_direction`
/// 是方向光（也决定天空回退里的太阳方向 `to_sun = -direction`）。
fn render_frame(
    renderer: &mut Renderer,
    view: &wgpu::TextureView,
    target: &wgpu::Texture,
    ssr: bool,
    sun_direction: Vec3,
    cloud_coverage: f32,
) -> Vec<u8> {
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 2.0, 6.0),
        Vec3::new(0.0, 2.0, 5.0),
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(
            camera,
            Viewport::new(SIZE as f32, SIZE as f32).expect("viewport"),
            DEFAULT_3D_CLEAR,
        ),
        light: Light {
            direction: sun_direction,
            ..Light::default()
        },
        point_lights: Vec::new(),
        drawables: vec![ground_drawable()],
    };
    let mut settings = RenderSettings::default()
        .with_pbr(true)
        .with_hdr(ToggleMode::On)
        .with_ssr(ssr);
    // 天空与雾同一个开关；密度 0 ⇒ 只画天空、不上雾色。
    settings.fog = true;
    settings.fog_density = 0.0;
    renderer.set_render_settings(settings);
    renderer.set_cloud_coverage(cloud_coverage);
    let outcome = renderer.render_into_texture(&empty_mesh(), Some(&scene), view, SIZE, SIZE, true);
    assert!(
        matches!(outcome, RenderOutcome::Presented),
        "天空回退帧必须正常出图"
    );
    assert!(
        renderer.last_frame_metrics().hdr_active,
        "天空回退只在 HDR 路径上，这里必须是 HDR 帧"
    );
    renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("ssr sky readback")
}

fn changed_pixels_in(lhs: &[u8], rhs: &[u8], rows: std::ops::Range<u32>) -> usize {
    let mut changed = 0;
    for row in rows.clone() {
        for column in 0..SIZE {
            let index = ((row * SIZE + column) * 4) as usize;
            if (0..3)
                .any(|channel| lhs[index + channel].abs_diff(rhs[index + channel]) > DIFF_THRESHOLD)
            {
                changed += 1;
            }
        }
    }
    changed
}

fn setup() -> Option<(Renderer, wgpu::Device, wgpu::Texture, wgpu::TextureView)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        gpu_skipped("request_adapter");
        return None;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-test-ssr-sky-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-test-ssr-sky-target"),
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
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    Some((renderer, device, target, view))
}

#[test]
fn ssr_sky_fallback_lifts_missed_reflections() {
    let Some((mut renderer, _device, target, view)) = setup() else {
        return;
    };
    // 太阳在相机背后上方：地面反射线一律朝 −z，太阳盘/光晕都进不了反射。
    let sun = Vec3::new(0.0, -0.8, -0.6);
    // 云量 0：这一条量的是渐变本身（云的遮挡见 P3-R178 用例）。
    let off = render_frame(&mut renderer, &view, &target, false, sun, 0.0);
    let on = render_frame(&mut renderer, &view, &target, true, sun, 0.0);
    assert!(
        renderer.last_frame_metrics().ssr_active,
        "SSR 打开的帧必须报告生效，否则下面量的是别的通道"
    );

    let floor_changed = changed_pixels_in(&off, &on, FLOOR_ROWS.clone());
    let sky_changed = changed_pixels_in(&off, &on, SKY_ROWS.clone());
    let mut red_delta = 0i64;
    let mut blue_delta = 0i64;
    let mut darkened = 0usize;
    for row in FLOOR_ROWS.clone() {
        for column in 0..SIZE {
            let index = ((row * SIZE + column) * 4) as usize;
            let red = i64::from(on[index]) - i64::from(off[index]);
            let blue = i64::from(on[index + 2]) - i64::from(off[index + 2]);
            let luma_on = 0.2126 * f32::from(on[index])
                + 0.7152 * f32::from(on[index + 1])
                + 0.0722 * f32::from(on[index + 2]);
            let luma_off = 0.2126 * f32::from(off[index])
                + 0.7152 * f32::from(off[index + 1])
                + 0.0722 * f32::from(off[index + 2]);
            if luma_on + 1.0 < luma_off {
                darkened += 1;
            }
            red_delta += red;
            blue_delta += blue;
        }
    }
    let floor_area = FLOOR_ROWS.len() as u32 * SIZE;
    let mean_red = red_delta as f64 / f64::from(floor_area);
    let mean_blue = blue_delta as f64 / f64::from(floor_area);
    println!(
        "ssr sky: floor_changed={floor_changed}/{floor_area} sky_changed={sky_changed} mean_red={mean_red:.2} mean_blue={mean_blue:.2} darkened={darkened}"
    );

    assert!(
        floor_changed > (floor_area as usize) / 4,
        "地面区只有 {floor_changed}/{floor_area} 个像素与 SSR 关的帧不同：未命中回退没接上（还是黑）"
    );
    assert_eq!(
        sky_changed, 0,
        "地平线以上（depth=1）有 {sky_changed} 个像素被 SSR 改了：天空像素必须在片元开头就返回零"
    );
    assert!(
        mean_blue > mean_red,
        "回退加的不是天空色：红增量 {mean_red:.2} ≥ 蓝增量 {mean_blue:.2}"
    );
    assert_eq!(
        darkened, 0,
        "天空回退只允许加能量，却有 {darkened} 个地面像素变暗了"
    );
}

/// P3-R177：反射扫过太阳盘时不许把 LDR 打饱和。
///
/// 方向光 `(0, −0.5, 0.866)`（太阳在相机前方）⇒ `to_sun ≈ (0, 0.5, −0.866)`，仰角 30°：
/// 地面反射线一律朝 −z，可见仰角 0°..45°，30° 正好落在中间带（第一条用例的太阳在相机
/// 背后、`to_sun` 朝 +z，反射线与之点积恒为 0，盘永远打不着）。
/// `pow(sun, 1400)` 的半宽约 1.8°，投在 160 px 的 90° 画面上是几行宽；上限生效时它只是
/// 一条偏暖的亮带，去掉 `min(sun_term, CAP)`（反面对照）⇒ 盘心 14.4 线性，直接饱和成
/// (255,255,255)，本用例红。
#[test]
fn ssr_sky_sun_disk_stays_bounded() {
    let Some((mut renderer, _device, target, view)) = setup() else {
        return;
    };
    let sun = Vec3::new(0.0, -0.5, 0.866);
    let off = render_frame(&mut renderer, &view, &target, false, sun, 0.0);
    // `ssr_blur` 的 0.75 时序累积从零历史起步：首帧盘心只值 25%，8 帧后收敛到稳态。
    let mut on = Vec::new();
    for _ in 0..8 {
        on = render_frame(&mut renderer, &view, &target, true, sun, 0.0);
    }
    assert!(
        renderer.last_frame_metrics().ssr_active,
        "SSR 打开的帧必须报告生效，否则下面量的是别的通道"
    );

    let floor_changed = changed_pixels_in(&off, &on, FLOOR_ROWS.clone());
    let mut saturated = 0usize;
    let mut brightest = 0u8;
    for row in FLOOR_ROWS.clone() {
        for column in 0..SIZE {
            let index = ((row * SIZE + column) * 4) as usize;
            let peak = on[index].max(on[index + 1]).max(on[index + 2]);
            brightest = brightest.max(peak);
            if on[index] >= 250 && on[index + 1] >= 250 && on[index + 2] >= 250 {
                saturated += 1;
            }
        }
    }
    let floor_area = FLOOR_ROWS.len() as u32 * SIZE;
    println!(
        "ssr sky sun: floor_changed={floor_changed}/{floor_area} saturated={saturated} brightest={brightest}"
    );

    assert!(
        floor_changed > (floor_area as usize) / 4,
        "太阳在相机前方时地面区只有 {floor_changed}/{floor_area} 个像素被回退抬亮：回退没接上"
    );
    assert!(
        brightest < 210,
        "地面最亮通道 {brightest} ≥ 210：太阳项上限 0.25 没接上（无上限的读数是 233）"
    );
}

/// P3-R178：反射的天空必须带云。
///
/// 四帧差分把「云影打在几何上」和「云挡在反射里」分开：同一个云量下 SSR 开/关的差
/// 是反射贡献，云量 0 → 0.9 时这个贡献必须整体变小。每条腿都跑 12 帧让 `ssr_blur`
/// 的历史从上一档收敛过来（0.75^12 ≈ 3% 残留）。
#[test]
fn ssr_sky_reflections_respect_cloud_coverage() {
    let Some((mut renderer, _device, target, view)) = setup() else {
        return;
    };
    let sun = Vec3::new(0.0, -0.5, 0.866);
    let clear_off = render_frame(&mut renderer, &view, &target, false, sun, 0.0);
    let mut clear_on = Vec::new();
    for _ in 0..12 {
        clear_on = render_frame(&mut renderer, &view, &target, true, sun, 0.0);
    }
    let cloudy_off = render_frame(&mut renderer, &view, &target, false, sun, 0.9);
    let mut cloudy_on = Vec::new();
    for _ in 0..12 {
        cloudy_on = render_frame(&mut renderer, &view, &target, true, sun, 0.9);
    }
    assert!(
        renderer.last_frame_metrics().ssr_active,
        "SSR 打开的帧必须报告生效，否则下面量的是别的通道"
    );

    let floor_area = FLOOR_ROWS.len() as u32 * SIZE;
    let mut clear_sum = 0.0_f64;
    let mut cloudy_sum = 0.0_f64;
    let mut clear_hue = 0.0_f64;
    let mut cloudy_hue = 0.0_f64;
    for row in FLOOR_ROWS.clone() {
        for column in 0..SIZE {
            let index = ((row * SIZE + column) * 4) as usize;
            let luma = |frame: &[u8]| {
                0.2126 * f32::from(frame[index])
                    + 0.7152 * f32::from(frame[index + 1])
                    + 0.0722 * f32::from(frame[index + 2])
            };
            // 反射贡献的色相：天空是蓝的（蓝−红 > 0），云的受光色偏暖。
            let hue = |on: &[u8], off: &[u8]| {
                f32::from(on[index + 2])
                    - f32::from(off[index + 2])
                    - (f32::from(on[index]) - f32::from(off[index]))
            };
            clear_sum += f64::from(luma(&clear_on) - luma(&clear_off));
            cloudy_sum += f64::from(luma(&cloudy_on) - luma(&cloudy_off));
            clear_hue += f64::from(hue(&clear_on, &clear_off));
            cloudy_hue += f64::from(hue(&cloudy_on, &cloudy_off));
        }
    }
    let floor_area = f64::from(floor_area);
    let clear_mean = clear_sum / floor_area;
    let cloudy_mean = cloudy_sum / floor_area;
    let clear_hue = clear_hue / floor_area;
    let cloudy_hue = cloudy_hue / floor_area;
    println!(
        "ssr sky cloud: clear_mean={clear_mean:.2} cloudy_mean={cloudy_mean:.2} clear_hue={clear_hue:.2} cloudy_hue={cloudy_hue:.2}"
    );

    assert!(
        clear_mean > 2.0,
        "无云时反射没加能量（{clear_mean:.2}），后面云有没有遮挡都量不出来"
    );
    assert!(
        cloudy_mean < clear_mean * 0.95,
        "云没有压暗反射里的天空：无云 {clear_mean:.2} vs 多云 {cloudy_mean:.2}（门槛 95%）"
    );
    // P3-R179：只做透射衰减会把反射里的「天蓝」等比压暗（蓝−红的比值基本不变，实测
    // 6.27/6.93 = 0.905）；云的受光色回射会把这条差值进一步拉低，这是「云不只是洞」的
    // 唯一可量特征——净亮度那一维被压缩后已经分不出来。
    assert!(
        cloudy_hue < clear_hue * 0.85,
        "云没有把反射染暖：无云蓝−红 {clear_hue:.2} vs 多云 {cloudy_hue:.2}（门槛 85%）"
    );
}
