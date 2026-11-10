//! P3-R207：深度加权水面折射（`set_water_refraction_gain`）的门。
//!
//! 背景：`tonemap.wgsl` 早就有一版折射，但偏移量是**写死的常数** `0.0035`，
//! 与「这一像素的水有多深」「视角有多平」都无关 —— 文档里因此仍记着「真实折射未做」。
//! 本档把偏移量改成 `基准 + 增益 × 光程 × 掠射角`：光程就是 P3-R171 那份
//! 「水位 − 河床 y」，掠射角来自视线方向。默认关闭 ⇒ 走回那枚常数，像素逐字节不变。
//!
//! 门三条（全部把渲染钟钉在 `CLOCK_SECS`：着色器时间统一走 `clock_secs()`，
//! 两帧之间只要真实时间走过几百毫秒，波相位差就会淹没档位的像素差）：
//! * **默认 = 显式关闭**：同参数两帧逐字节相同（档没被偷偷打开）。
//! * **不越界 + 确实改像素**：开档后地平线以上逐字节不变（折射仍只在掩码内），
//!   水面区块必须有像素变化。
//! * **深度加权是真的**：把「开档 − 关档」的差值除以基线的空间梯度（消掉深水被吸收
//!   压暗带来的对比度差异），得到的相对位移在 5 m 光程必须明显大于 1 m 光程；
//!   同一河床下把增益再翻 4 倍，位移也必须明显变大（钉住「uniform 真的到了着色器」）。
//!
//! 反面控制（手工执行、记在文档里）：把 `passes.rs` 的 `params[111]`（深度增益）临时
//! 改成 0 ⇒ 深度那一条必须红；把 `params[110]` 的基准改成 0 之外的值 ⇒ 「默认 = 关闭」
//! 那条必须红。
//!
//! P3-R241 追加：同一夹具上的 **Snell 屏幕空间步进档**（`set_water_refract_steps`）。
//! 门钉的是「默认关 + 步数真的驱动循环 + 命中点被采用」：
//! * 默认 / 显式 0 / **1 步**三帧逐字节相同 —— 1 步只能走 0.35 m（最深 0.34 m），
//!   而河床在 5 m 下 ⇒ 必然一次都不命中、回退到 R207 的偏移落点；若 1 步就改了像素，
//!   说明走的是偏移近似而不是真步进。
//! * 16 步必须改水面像素（天光不变）：命中点由折射射线投影得到，与偏移落点是两个点。
//! * 步数钳位（宿主 32 / 着色器再钳一次）与「档位 > 0 才写 uniform」的源文本契约。
//!
//! 反面控制：把 `passes.rs` 的 `params[136]` 改成 1.0（无条件开）⇒ 第一条红；
//! 删掉 `select(..., hit_uv, hit)` 里的 `hit` ⇒ 第二条红；把 `params[136]` 接到
//! 别的空槽位 ⇒ 第二条红。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const LEVEL: f32 = 0.0;
const SHALLOW_BED: f32 = -1.0;
const DEEP_BED: f32 = -5.0;
const SIZE: u32 = 64;
/// 地平线以上：折射不许漏到天空（与 P3-R171 同一份水掩码口径）。
const SKY_ROWS: std::ops::Range<u32> = 0..12;
/// 两个河床高度都覆盖的水面区块（相机 y=3 俯视原点、90° 视场）。
const BED_ROWS: std::ops::Range<u32> = 44..56;
const BED_COLS: std::ops::Range<u32> = 24..40;
/// 探针增益（uv / 米光程 / 掠射角）。**不是定档值**：夹具只有 64² ⇒ 一个纹素
/// = 1/64 ≈ 0.0156 uv，而波斜率最大约 0.65，按真实量级（约 8e−4，1280×720 下
/// 5 m 水深的偏移约 3 个纹素）在这张图上连一个纹素都跨不过去，逐字节差值恒为 0。
/// 折射强度对增益是线性的，门要钉的是「uniform 进了着色器 + 按光程放大」，
/// 所以这里把探针放大到能跨过纹素；视觉定档另说。
const BASE_GAIN: f32 = 0.030;
const STRONG_GAIN: f32 = 0.120;
/// 钉住的渲染钟读数（秒）。取非零值：`t = 0` 时波法线恰好是平的，
/// 折射偏移整体归零，「开档必须改像素」那条会空转。
const CLOCK_SECS: f32 = 4.2;

/// 着色器契约：折射强度必须同时吃到光程与掠射角，且默认走回旧常数。
#[test]
fn tonemap_refraction_strength_is_gated_by_depth_and_grazing() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains(
            "fn water_refraction(uv: vec2<f32>, time: f32, optical_depth: f32, grazing: f32)"
        ),
        "折射必须收光程与掠射角两个参数，否则深度加权无从谈起"
    );
    assert!(
        source.contains("params.water_under.z + params.water_under.w * optical_depth * grazing"),
        "强度必须是「基准 + 增益 × 光程 × 掠射角」"
    );
    assert!(
        source.contains("params.water_under.z > 0.0,"),
        "关档必须由基准为 0 判定，回退到旧常数 0.0035"
    );
    assert!(
        source.contains("let refracted = water_refraction(uv, params.camera_position.w, optical_depth, grazing);"),
        "调用点必须把同一份光程传给折射（自己再算一遍就会和吸收/散射口径分家）"
    );
    // 折射段必须整段位于水掩码内部：天空像素的深度不是河床。
    let mask = source
        .find("        if surface.valid {")
        .expect("水合成分支必须以水掩码开头");
    let refracted = source
        .find("let refracted = water_refraction(")
        .expect("折射调用点");
    assert!(
        refracted > mask,
        "折射在水掩码之外 ⇒ 天空/岸上像素会被按远景深度偏移"
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

fn water_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Water".to_owned(),
        mesh_path: format!("__water__?size=40&res=8&y={LEVEL}"),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-20.0, -0.1, -20.0),
                max: Vec3::new(20.0, 0.1, 20.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

fn bed_drawable(height: f32) -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(3),
        name: "Bed".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, height, 0.0)),
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

struct Fixture {
    renderer: Renderer,
    scene: Scene3d,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    fn with_bed(&mut self, height: f32) {
        self.scene.drawables = vec![bed_drawable(height), water_drawable()];
    }

    /// 渲染一帧并回读紧凑 RGBA8（时钟已在建夹具时钉住）。
    fn render(&mut self) -> Option<Vec<u8>> {
        let outcome = self.renderer.render_into_texture(
            &empty_mesh(),
            Some(&self.scene),
            &self.view,
            SIZE,
            SIZE,
            true,
        );
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "折射帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("refraction readback");
        if !self.renderer.last_frame_metrics().hdr_active {
            return None;
        }
        Some(pixels)
    }
}

fn fixture(label: &str) -> Option<Fixture> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-test-refraction-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    // 后期颗粒是逐帧抖动：不关掉的话「默认 vs 显式关闭」的逐字节对照没有意义。
    renderer.set_post_fx(0.0, 0.0);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 3.0, 6.0),
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
    // 取一个非零秒数：t=0 时波法线恰好是平的，折射偏移会整体归零，
    // 「开档必须改像素」那条就变成空转。
    renderer.freeze_clock_for_tests(CLOCK_SECS);
    let scene = Scene3d {
        camera,
        view: scene_view,
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![bed_drawable(SHALLOW_BED), water_drawable()],
    };
    Some(Fixture {
        renderer,
        scene,
        target,
        view,
    })
}

/// 区块内逐像素绝对差之和（RGB 三通道）。
fn diff_sum(a: &[u8], b: &[u8]) -> f64 {
    let mut sum = 0.0f64;
    for row in BED_ROWS {
        for col in BED_COLS.clone() {
            let base = ((row * SIZE + col) * 4) as usize;
            for channel in 0..3 {
                sum += (a[base + channel] as f64 - b[base + channel] as f64).abs();
            }
        }
    }
    sum
}

/// 基线自身的空间梯度：位移量除以它，得到的才是「挪了几个自己」的相对量，
/// 深水被吸收压暗（对比度整体变小）就不会伪装成「位移变小」。
fn gradient_sum(pixels: &[u8]) -> f64 {
    let mut sum = 0.0f64;
    for row in BED_ROWS.clone().take(BED_ROWS.len() - 1) {
        for col in BED_COLS.clone().take(BED_COLS.len() - 1) {
            let base = ((row * SIZE + col) * 4) as usize;
            let right = base + 4;
            let down = base + (SIZE as usize) * 4;
            for channel in 0..3 {
                sum += (pixels[base + channel] as f64 - pixels[right + channel] as f64).abs();
                sum += (pixels[base + channel] as f64 - pixels[down + channel] as f64).abs();
            }
        }
    }
    sum.max(1.0)
}

/// 顶部若干行是否逐字节相同（折射没漏到天空）。
fn sky_identical(a: &[u8], b: &[u8]) -> bool {
    let sky_len = (SKY_ROWS.end * SIZE * 4) as usize;
    a[..sky_len] == b[..sky_len]
}

/// 两帧之间取值不同的字节个数（全帧，不分区块）。
fn byte_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

#[test]
fn refraction_dial_is_off_by_default_and_scales_with_depth() {
    let Some(mut fixture) = fixture("rustlua-test-refraction-target") else {
        return gpu_skipped("request_adapter");
    };

    // ① 默认 == 显式关闭：两帧逐字节相同。
    let Some(default_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_water_refraction_gain(0.0);
    let Some(off_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let changed = byte_diff(&default_frame, &off_frame);
    assert_eq!(
        changed, 0,
        "显式关闭的档与默认帧不一致（{changed} 字节不同）：默认路径被改了"
    );
    assert_eq!(
        fixture.renderer.water_refraction_gain(),
        0.0,
        "默认增益必须是 0（关）"
    );
    // 非法值归一为关闭。
    fixture.renderer.set_water_refraction_gain(f32::NAN);
    assert_eq!(fixture.renderer.water_refraction_gain(), 0.0);
    fixture.renderer.set_water_refraction_gain(-1.0);
    assert_eq!(fixture.renderer.water_refraction_gain(), 0.0);

    // ② 开档：天空逐字节不变，水面区块必须有变化。
    fixture.renderer.set_water_refraction_gain(BASE_GAIN);
    let Some(on_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    assert!(
        sky_identical(&off_frame, &on_frame),
        "地平线以上的像素被折射改动：越出了水掩码"
    );
    let shallow_off = diff_sum(&off_frame, &on_frame);
    assert!(
        shallow_off > 0.0,
        "开档后水面区块一个像素都没变：折射档是空转的"
    );

    // ③ 深度加权：5 m 光程的相对位移必须明显大于 1 m 光程。
    fixture.with_bed(DEEP_BED);
    fixture.renderer.set_water_refraction_gain(0.0);
    let Some(deep_off) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_water_refraction_gain(BASE_GAIN);
    let Some(deep_on) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    assert!(sky_identical(&deep_off, &deep_on), "深水帧的天空被改动");
    let shallow_shift = shallow_off / gradient_sum(&off_frame);
    let deep_diff = diff_sum(&deep_off, &deep_on);
    let deep_shift = deep_diff / gradient_sum(&deep_off);
    println!(
        "refraction shallow={shallow_shift:.5} deep={deep_shift:.5} \
         (diffs {shallow_off:.0} / {deep_diff:.0})"
    );
    assert!(
        deep_shift > shallow_shift * 1.5,
        "5 m 光程的相对位移（{deep_shift:.5}）没有明显大于 1 m（{shallow_shift:.5}）：\
         折射偏移没按水深加权，仍是常数档"
    );

    // ④ 增益真的进了着色器：同一河床把增益翻 4 倍，位移必须跟着变大。
    fixture.with_bed(SHALLOW_BED);
    fixture.renderer.set_water_refraction_gain(0.0);
    let Some(replay) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    // 同一份参数在序列末尾再渲一次必须与首帧逐字节相同：这一步把「时间之外还有
    // 每帧状态」整类失败钉住（否则前面所有差值读数都可能只是相位漂移）。
    let replay_diff = byte_diff(&off_frame, &replay);
    assert_eq!(
        replay_diff, 0,
        "关档重放帧与首帧不一致（{replay_diff} 字节不同）：夹具里还有没被钉住的逐帧状态"
    );
    fixture.renderer.set_water_refraction_gain(STRONG_GAIN);
    let Some(strong_on) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let strong_shift = diff_sum(&off_frame, &strong_on) / gradient_sum(&off_frame);
    println!("refraction gain x4 shift={strong_shift:.5} vs base={shallow_shift:.5}");
    assert!(
        strong_shift > shallow_shift * 1.5,
        "增益翻 4 倍后位移没跟着变大（{strong_shift:.5} vs {shallow_shift:.5}）：\
         uniform 没接到着色器"
    );
}

/// P3-R241：Snell 屏幕空间步进档的门（见文件头注释）。
#[test]
fn snell_stepping_is_off_by_default_and_steps_drive_the_march() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains("fn refraction_step_uv(bed: vec3<f32>, slope: vec2<f32>, fallback: vec2<f32>) -> vec3<f32>"),
        "Snell 步进函数缺失：水面折射仍只有 R207 的偏移近似"
    );
    assert!(
        source.contains("if params.water_refract.x >= 0.5 && surface_depth < 0.9999 {"),
        "步进必须在「档位 ≥ 1 且像素确有河床深度」时才进入，否则天空/岸上像素会被步进"
    );
    assert!(
        source.contains("i32(clamp(params.water_refract.x, 1.0, 32.0))"),
        "着色器必须自己再钳一次步数（uniform 可以带进任何值）"
    );
    assert!(
        source.contains("let refracted = refract(direction, normal, params.water_refract.y);"),
        "水下方向必须来自 Snell 折射（eta = 空气/水），不是任意偏移"
    );
    let passes = include_str!("../passes.rs");
    assert!(
        passes.contains("if self.water_refract_steps > 0 {")
            && passes.contains("params[136] = self.water_refract_steps as f32;"),
        "宿主必须在档位 > 0 时才写 water_refract 槽（默认四槽恒零 ⇒ 逐字节不变）"
    );

    let Some(mut fixture) = fixture("rustlua-test-refract-steps-target") else {
        return gpu_skipped("request_adapter");
    };
    assert_eq!(fixture.renderer.water_refract_steps(), 0, "默认档必须是关");
    fixture.renderer.set_water_refract_steps(1000);
    assert_eq!(
        fixture.renderer.water_refract_steps(),
        32,
        "步数必须钳在 32"
    );
    fixture.renderer.set_water_refract_steps(0);

    // 深河床：5 m 水柱下折射横向位移约 1 m（64² 上数个纹素），1 m 河床只有约 0.4 m，
    // 容易落在同一个纹素里 —— 门用深河床不给「改像素」那条留运气。
    fixture.with_bed(DEEP_BED);
    let Some(off_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_water_refract_steps(1);
    let Some(one_step) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let one_diff = byte_diff(&off_frame, &one_step);
    assert_eq!(
        one_diff, 0,
        "1 步就改了 {one_diff} 字节：第一步只走 0.35 m，5 m 河床一次都不该命中，\
         说明开档后走的不是步进而是偏移近似"
    );

    fixture.renderer.set_water_refract_steps(16);
    let Some(stepped) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    assert!(
        sky_identical(&off_frame, &stepped),
        "地平线以上的像素被步进改动：越出了水掩码"
    );
    let water_diff = diff_sum(&off_frame, &stepped);
    let changed = byte_diff(&off_frame, &stepped);
    println!("refract steps=16 diff_sum={water_diff:.0} changed_bytes={changed}");
    assert!(
        water_diff > 0.0,
        "16 步一个水面像素都没改：步进空转（命中点没被采用）或 uniform 没进着色器"
    );
    // 命中点与偏移落点差出数米横向位移，64² 的水面区块里必然是一整片像素在动；
    // 只动几十字节说明只是撞线噪声，不能证明命中点真的被采了。
    assert!(
        changed >= 64,
        "16 步只改了 {changed} 字节（预期数百）：命中点没被采用"
    );
}
