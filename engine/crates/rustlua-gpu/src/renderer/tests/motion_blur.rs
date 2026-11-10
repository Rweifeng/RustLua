//! P3-R174：相机运动模糊门。
//!
//! 做法：把当前像素的世界点用**上一帧**的视图投影投回屏幕，与当前 uv 的差就是这一像素
//! 一帧内扫过的屏幕距离；沿这个方向在 Tonemap 里取 5 个抽头（中心 + 各 ±0.25/0.5/0.75/1
//! 倍，权重线性衰减），上限 0.02 uv。宿主每帧把本帧视图投影留成下一帧的「上一帧」，
//! 与 SSR 的那份分开（SSR 关掉时它不更新，而运动模糊要跟着每一帧走）。
//!
//! 离线一条：着色器契约（重投影用 `previous_view_projection`、零位移提前返回、速度有上限、
//! 只在曝光之后泛光之前）+ 宿主写槽位 / 每帧轮换上一帧矩阵。
//!
//! GPU 一条：160²、`__ground_plane__` 放大 4 倍、相机固定眼位只改俯仰，地平线是一条强梯度。
//! * **静止不变**：相机不动时强度 1 与强度 0 逐字节相同（零位移 ⇒ 全部抽头落在同一像素）；
//! * **有位移才糊**：两帧间俯仰步进 0.10 rad（≈0.064 uv，被 0.02 上限截断）时，强度 1 的
//!   第二帧必须与强度 0 的那帧不同；
//! * **跟着重投影走**：同一场景把步进缩到 0.004 rad（≈0.0025 uv，亚像素位移）后，变化的像素
//!   数必须显著减少 —— 常数量级的滤镜/整屏染色过不了这条，只有按速度积分才成立；
//! * **地平线变软而不是变脏**：抽头落在天空时必须走 `sky_color`（P3-R175），所以拖影是
//!   「天空色混进地面」，最陡行间梯度必须下降；直接采源图清屏色的写法梯度反而会升到 1.15×
//!   （P3-R174 首次跑出的 108.18 vs 93.98 就是这条的反面对照）。
//!
//! 反面控制（手工执行、记在文档里）：把 `passes.rs` 的 `params[131]` 固定成 0.0 ⇒ 强度 1
//! 的第二帧退回成清晰帧，位移断言必须红。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 160;
/// 大位移：俯仰步进（弧度）。垂直 fov = π/2 ⇒ 屏幕位移 ≈ 0.10 / 1.57 ≈ 0.064 uv，
/// 超过 0.02 的抽头上限，量到的是「被截断后的最大拖影」。
const PITCH_STEP: f32 = 0.10;
/// 小位移：同一场景的 1/25，屏幕位移 ≈ 0.0025 uv（亚像素）—— 只有按速度积分才会少改像素。
const PITCH_STEP_SMALL: f32 = 0.004;
/// 判「变了」的像素阈值（0..255）。
const DIFF_THRESHOLD: u8 = 8;
/// 量地平线梯度只取中间列，避开地面左右两侧的边缘。
const CENTER_COLS: std::ops::Range<u32> = 56..104;

#[test]
fn tonemap_reprojects_with_the_previous_frame_and_skips_zero_motion() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains(
            "fn motion_blur_at(uv: vec2<f32>, center: vec3<f32>, gain: f32) -> vec3<f32>"
        ),
        "运动模糊要抽成函数，重投影口径独立可读"
    );
    assert!(
        source.contains("params.previous_view_projection * vec4<f32>(point, 1.0)"),
        "必须用上一帧视图投影把当前世界点投回屏幕，而不是拿速度场猜"
    );
    assert!(
        source.contains("dot(velocity, velocity) < 1e-12"),
        "零位移必须提前返回：相机静止时帧要逐字节不变"
    );
    assert!(
        source.contains("clamp(velocity, vec2<f32>(-limit), vec2<f32>(limit))"),
        "屏幕速度要有上限，遮挡边界上的巨大重投影不能把整屏抽到另一侧"
    );
    assert!(
        source.contains("return accumulation / weight;"),
        "抽头必须归一化求和"
    );
    assert!(
        source.contains("fn motion_tap(uv: vec2<f32>, gain: f32) -> vec3<f32>")
            && source.contains("if depth_at(uv) >= 0.9999 && params.sky.x > 0.5 {")
            && source.contains("return sky_color(uv);"),
        "抽头落在天空时必须走 `sky_color`（P3-R175）：直接采源图会把清屏色混进地面，压出地平线暗边"
    );
    assert!(
        source.contains("accumulation += motion_tap(forward, gain) * tap_weight;")
            && source.contains("accumulation += motion_tap(backward, gain) * tap_weight;"),
        "抽头必须走 `motion_tap`，不能绕过天空替换"
    );
    let gain = source
        .find("var linear = max(sampled.rgb * gain, vec3<f32>(0.0));")
        .expect("曝光与钳位");
    let blur = source
        .find("linear = motion_blur_at(uv, linear, gain);")
        .expect("运动模糊调用");
    let bloom = source.find("if params.bloom > 0.0 {").expect("泛光分支");
    assert!(
        gain < blur && blur < bloom,
        "运动模糊要在曝光之后、泛光之前：拖影不参与 HDR 泛光扩散"
    );

    let host = include_str!("../passes.rs");
    assert!(
        host.contains("params[112..128].copy_from_slice(&previous.columns.concat());"),
        "上一帧视图投影占 112..128 这 16 个槽"
    );
    assert!(
        host.contains("params[128] = motion_blur.clamp(0.0, 1.0);")
            && host.contains("params[131] = 1.0;"),
        "强度与开关要由宿主写，0 强度必须整段跳过"
    );
    assert!(
        host.contains("self.tonemap_prev_view_projection = current_view_projection;"),
        "每帧都要轮换「上一帧」，否则第二帧起就再也算不出速度"
    );
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

fn ground_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(7),
        name: "Ground".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::scale(Vec3::new(4.0, 1.0, 4.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-16.0, -0.1, -16.0),
                max: Vec3::new(16.0, 0.1, 16.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

struct Fixture {
    renderer: Renderer,
    viewport: Viewport,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    /// `pitch` 是视线向下与水平面的夹角（弧度）：眼位固定，只低头/抬头。
    fn render(&mut self, pitch: f32) -> Option<Vec<u8>> {
        let eye = Vec3::new(0.0, 2.0, 6.0);
        let camera = Camera3D::perspective(
            eye,
            Vec3::new(0.0, 2.0 - pitch, 5.0),
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .expect("camera");
        let view = RenderView::main_3d(camera, self.viewport, DEFAULT_3D_CLEAR);
        self.renderer.set_frame_views(FrameViews {
            main: Some(view),
            overlay: None,
        });
        let scene = Scene3d {
            camera,
            view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables: vec![ground_drawable()],
        };
        let mesh = empty_mesh();
        let outcome =
            self.renderer
                .render_into_texture(&mesh, Some(&scene), &self.view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "运动模糊帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("motion-blur readback");
        if !self.renderer.last_frame_metrics().hdr_active {
            return None;
        }
        Some(pixels)
    }
}

fn fixture(label: &str, blur: f32) -> Option<Fixture> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-test-motion-blur-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let mut settings = RenderSettings::default()
        .with_pbr(true)
        .with_hdr(ToggleMode::On);
    // 只打开天空（雾密度 0 ⇒ 只画天空、不上雾色），地平线因此是一条干净的高对比边。
    settings.fog = true;
    settings.fog_density = 0.0;
    settings.motion_blur = blur;
    renderer.set_render_settings(settings);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
    Some(Fixture {
        renderer,
        viewport,
        target,
        view,
    })
}

/// 同一台设备上按顺序渲染一组相机姿态，返回每一帧的像素。
fn run(label: &str, blur: f32, poses: &[f32]) -> Vec<Vec<u8>> {
    let Some(mut fixture) = fixture(label, blur) else {
        gpu_skipped("request_adapter");
        return Vec::new();
    };
    let mut frames = Vec::with_capacity(poses.len());
    for &pitch in poses {
        let Some(frame) = fixture.render(pitch) else {
            gpu_skipped("hdr_active");
            return Vec::new();
        };
        frames.push(frame);
    }
    frames
}

fn changed_pixels(lhs: &[u8], rhs: &[u8]) -> usize {
    lhs.chunks_exact(4)
        .zip(rhs.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|channel| a[channel].abs_diff(b[channel]) > DIFF_THRESHOLD))
        .count()
}

fn luma_at(pixels: &[u8], row: u32, column: u32) -> f32 {
    let index = ((row * SIZE + column) * 4) as usize;
    0.2126 * f32::from(pixels[index])
        + 0.7152 * f32::from(pixels[index + 1])
        + 0.0722 * f32::from(pixels[index + 2])
}

/// 地平线附近最陡的行间梯度：最亮的一行往下一行掉了多少（中间列取均值）。
fn max_row_step(pixels: &[u8]) -> f32 {
    let mut steepest = 0.0f32;
    for row in 0..SIZE - 1 {
        let mut upper = 0.0;
        let mut lower = 0.0;
        for column in CENTER_COLS.clone() {
            upper += luma_at(pixels, row, column);
            lower += luma_at(pixels, row + 1, column);
        }
        let step = (upper - lower).abs() / CENTER_COLS.len() as f32;
        steepest = steepest.max(step);
    }
    steepest
}

#[test]
fn motion_blur_only_trails_when_the_camera_moves() {
    let sharp = run("rustlua-test-motion-blur-sharp", 0.0, &[0.0, PITCH_STEP]);
    if sharp.is_empty() {
        return;
    }
    let blurred = run("rustlua-test-motion-blur-on", 1.0, &[0.0, PITCH_STEP]);
    let small = run(
        "rustlua-test-motion-blur-small",
        1.0,
        &[0.0, PITCH_STEP_SMALL],
    );
    // 小位移也要在**同一姿态**下取清晰参考，否则比较的是两张不同相机的帧。
    let sharp_small = run(
        "rustlua-test-motion-blur-sharp-small",
        0.0,
        &[0.0, PITCH_STEP_SMALL],
    );
    let still = run(
        "rustlua-test-motion-blur-still",
        1.0,
        &[PITCH_STEP, PITCH_STEP],
    );
    assert_eq!(blurred.len(), 2, "快门序列必须出两帧");
    assert_eq!(small.len(), 2, "小位移序列必须出两帧");
    assert_eq!(still.len(), 2, "静止序列必须出两帧");

    // ① 相机静止：强度 1 的第 2 帧必须与第 1 帧逐字节相同，并与强度 0 的同姿态帧相同。
    assert_eq!(
        still[0], still[1],
        "相机静止时运动模糊改变了帧：零位移没有提前返回"
    );
    assert_eq!(
        still[1], sharp[1],
        "相机静止时强度 1 与强度 0 的输出不一致：模糊在没有位移的帧上生效了"
    );

    // ② 有位移：第二帧必须与清晰帧不同。
    let changed = changed_pixels(&sharp[1], &blurred[1]);
    // ③ 同一场景把位移缩到 1/25：变化的像素数必须显著减少（不是常数量级的滤镜）。
    let changed_small = changed_pixels(&sharp_small[1], &small[1]);
    // ④ 地平线：拖影是「天空色混进地面」⇒ 最陡行间梯度下降；直接采源图的写法会反升到 1.15×。
    let sharp_step = max_row_step(&sharp[1]);
    let blur_step = max_row_step(&blurred[1]);
    println!(
        "motion blur: changed={changed}（步进 {PITCH_STEP}）changed_small={changed_small}（步进 {PITCH_STEP_SMALL}）sharp_step={sharp_step:.2} blur_step={blur_step:.2}"
    );
    assert!(
        changed > 120,
        "相机俯仰 {PITCH_STEP} rad 后强度 1 的帧与清晰帧只差 {changed} 个像素：重投影/写槽位没接上"
    );
    assert!(
        changed > changed_small * 4,
        "位移缩到 1/25 后变化像素只从 {changed} 降到 {changed_small}：拖影没有跟着重投影速度走"
    );
    assert!(
        blur_step < sharp_step * 0.9,
        "地平线最大行间梯度没有下降（sharp {sharp_step:.2} / blur {blur_step:.2}）：抽头没走 sky_color，拖影在地平线压出了暗边"
    );
}
