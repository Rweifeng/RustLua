//! P3-R208：水下视角的水-气界面（`set_submerged_view_gain`）门。
//!
//! 背景：相机潜到水位以下时，此前只有 P3-R173 的整帧体积吸收 —— 画面只会越看越黑，
//! 向上看没有 Snell 窗口、也没有临界角外的全反射银面。文档因此一直记着「三维淹水未做」。
//! 本档在 Tonemap 里按解析水面平面求交：朝上且打到界面、且界面之前没有几何体的像素，
//! 换成「折射出射方向上的天空（窗口内）」或「水体镜像 + 临界环（窗口外）」，光程改用
//! 「相机 → 界面」。默认增益 0 ⇒ 四个槽恒零，着色器走回 R173 原式，像素逐字节不变。
//!
//! 门四条（两帧对照都依赖 P3-R207 的 `clock_secs()` 时间锁）：
//! * **默认 = 显式关闭**：同参数两帧逐字节相同。
//! * **掩码不越界**：水下朝下看河床 ⇒ 开档与关档逐字节相同（射线打不到界面就不许动）。
//! * **几何守卫**：河床夹在相机与界面之间 ⇒ 开档与关档逐字节相同（隔着道具/蚁看界面是空转）。
//!   内建地面是单面的，夹具把它绕 X 轴翻 180°，从水下朝上看才进得了深度图。
//! * **窗口真的在上方**：朝上看时，靠天顶的条带必须明显亮于靠地平线的条带
//!   （前者入射角在临界角内看到天空，后者全反射）；同时开档相对关档必须有像素差。
//!
//! 反面控制（手工执行、记在文档里）：
//! * 把 `tonemap.wgsl` 的 `view.window` 固定成 `1.0`（取消临界角判定）⇒ 第四条必须红；
//! * 删掉 `if length(geometry - origin) <= t` 那段几何守卫 ⇒ 第三条必须红；
//! * 把 `construct.rs` 的 `submerged_view_gain` 默认改成 1.0 ⇒ 第一条必须红。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const LEVEL: f32 = 0.0;
/// 界面之下的河床（朝上看时它挡不住界面，朝下看时它就是被看的东西）。
const DEEP_BED: f32 = -6.0;
/// 夹在相机（y = −2）与界面之间的河床：朝上的射线先打到它。
const BLOCKING_BED: f32 = -0.6;
const EYE: Vec3 = Vec3::new(0.0, -2.0, 6.0);
const SIZE: u32 = 64;
const COLS: std::ops::Range<u32> = 20..44;
/// 朝上看时靠天顶的条带（入射角小 ⇒ Snell 窗口内）。
const WINDOW_ROWS: std::ops::Range<u32> = 6..20;
/// 同帧里靠地平线的条带（入射角掠过临界角 ⇒ 全反射）。
const MIRROR_ROWS: std::ops::Range<u32> = 46..60;
/// 探针强度：界面像素完全换成界面辐射（1 = 全效果）。
const GAIN: f32 = 1.0;

/// 着色器与宿主的契约：临界角、几何守卫、开关口径都必须落在源码里。
#[test]
fn submerged_interface_is_gated_by_eta_and_geometry() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains("fn submerged_interface(uv: vec2<f32>) -> SubmergedInterface"),
        "界面求交要抽成函数，否则水下分支与 R173 的吸收口径会分家"
    );
    assert!(
        source.contains("let critical = sqrt(max(1.0 - eta * eta, 0.0));"),
        "临界角余弦必须由折射率比算出（cos θc = √(1 − η²)），不能写死一个亮度分界"
    );
    assert!(
        source.contains("view.window = smoothstep(critical - band, critical + band, cosine);"),
        "窗口权重必须按入射余弦与临界角比较"
    );
    assert!(
        source.contains("view.direction = refract(incident, -normal, eta);"),
        "出射方向必须是 Snell 折射；全反射时 refract 自带退化"
    );
    assert!(
        source.contains("if length(geometry - origin) <= t {"),
        "界面之前有几何体（道具/蚁/河床）时必须让位，否则界面会把它们整块盖掉"
    );
    assert!(
        source.contains("if params.submersion.x < 0.5 {"),
        "整个界面分支必须由开关控制（关档 ⇒ 恒走 R173 原式）"
    );
    // 界面分支必须落在 R173 的入水判定内部：相机在水上时不许画窗口。
    let under = source
        .find("if params.water_under.x > 0.5 {")
        .expect("水下吸收分支");
    let radiance = source
        .find("linear = underwater_radiance(uv, linear);")
        .expect("水下辐射调用点");
    assert!(
        under < radiance && radiance < under + 400,
        "界面必须与 R173 的吸收同一开关，不能各开一处"
    );

    let host = include_str!("../passes.rs");
    assert!(
        host.contains("if underwater && self.submerged_view_gain > 0.0 {")
            && host.contains("params[132] = 1.0;"),
        "宿主必须只在「相机入水 + 档位非零」时写开关，其余情况四个槽留零"
    );
    assert!(
        host.contains("params[133] = 1.0 / 1.333;"),
        "折射率比由宿主给：水 → 空气 ≈ 1/1.333，着色器不该自己写死介质"
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
        mesh_path: format!("__water__?size=64&res=8&y={LEVEL}"),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-32.0, -0.1, -32.0),
                max: Vec3::new(32.0, 0.1, 32.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

/// 32×32 的地面：朝上看时它能整体挡在界面之前（几何守卫那条要用），朝下看时它就是河床。
/// `facing_down` 绕 X 轴转 180°：内建地面是单面的，水下朝上看只有翻过来的这一面才进深度图。
fn bed_drawable(height: f32, facing_down: bool) -> Scene3dDrawable {
    let flip = if facing_down {
        Mat4::from_quat(Quat::from_axis_angle(Vec3::X, std::f32::consts::PI).expect("quat"))
    } else {
        Mat4::identity()
    };
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(3),
        name: "Bed".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, height, 0.0))
            * flip
            * Mat4::scale(Vec3::new(4.0, 1.0, 4.0)),
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
    bed: f32,
    bed_faces_down: bool,
}

impl Fixture {
    fn with_bed(&mut self, height: f32, facing_down: bool) {
        self.bed = height;
        self.bed_faces_down = facing_down;
    }

    /// 渲染一帧并回读紧凑 RGBA8（时钟已在建夹具时钉住）。
    fn render(&mut self, target: Vec3) -> Option<Vec<u8>> {
        let camera = Camera3D::perspective(EYE, target, std::f32::consts::FRAC_PI_2, 0.1, 200.0)
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
            drawables: vec![
                bed_drawable(self.bed, self.bed_faces_down),
                water_drawable(),
            ],
        };
        let outcome = self.renderer.render_into_texture(
            &empty_mesh(),
            Some(&scene),
            &self.view,
            SIZE,
            SIZE,
            true,
        );
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "水下帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("submerged readback");
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
        label: Some("rustlua-test-submerged-device"),
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
    // 后期颗粒是逐帧抖动：不关掉的话逐字节对照没有意义。
    renderer.set_post_fx(0.0, 0.0);
    // 折射档会改水面像素的取样位置，留着只会给像素差再添一个来源。
    renderer.set_water_refraction_gain(0.0);
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
    renderer.freeze_clock_for_tests(4.2);
    Some(Fixture {
        renderer,
        viewport,
        target,
        view,
        bed: DEEP_BED,
        bed_faces_down: false,
    })
}

fn patch_luma(pixels: &[u8], rows: &std::ops::Range<u32>, cols: &std::ops::Range<u32>) -> f64 {
    let mut sum = 0.0f64;
    let mut count = 0.0f64;
    for row in rows.clone() {
        for column in cols.clone() {
            let index = ((row * SIZE + column) * 4) as usize;
            sum += 0.2126 * f64::from(pixels[index])
                + 0.7152 * f64::from(pixels[index + 1])
                + 0.0722 * f64::from(pixels[index + 2]);
            count += 1.0;
        }
    }
    sum / count.max(1.0)
}

/// 两帧之间取值不同的字节个数。
fn byte_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

#[test]
fn submerged_window_is_off_by_default_and_brighter_than_the_mirrored_ring() {
    let Some(mut fixture) = fixture("rustlua-test-submerged-target") else {
        return gpu_skipped("request_adapter");
    };

    // ① 默认 == 显式关闭：两帧逐字节相同，非法值一律归零。
    let Some(default_frame) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_submerged_view_gain(0.0);
    let Some(off_frame) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    let changed = byte_diff(&default_frame, &off_frame);
    assert_eq!(
        changed, 0,
        "显式关闭的档与默认帧不一致（{changed} 字节不同）：默认路径被改了"
    );
    assert_eq!(
        fixture.renderer.submerged_view_gain(),
        0.0,
        "默认强度必须是 0（关）"
    );
    fixture.renderer.set_submerged_view_gain(f32::NAN);
    assert_eq!(fixture.renderer.submerged_view_gain(), 0.0);
    fixture.renderer.set_submerged_view_gain(-1.0);
    assert_eq!(fixture.renderer.submerged_view_gain(), 0.0);
    fixture.renderer.set_submerged_view_gain(4.0);
    assert_eq!(
        fixture.renderer.submerged_view_gain(),
        1.0,
        "超过 1 的混合权重会把颜色反着混，必须截断"
    );
    fixture.renderer.set_submerged_view_gain(GAIN);

    // ② 掩码不越界：水下朝下看河床，射线打不到界面 ⇒ 开档必须逐字节不变。
    fixture.renderer.set_submerged_view_gain(0.0);
    let Some(down_off) = fixture.render(Vec3::new(0.0, -6.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_submerged_view_gain(GAIN);
    let Some(down_on) = fixture.render(Vec3::new(0.0, -6.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    let down_changed = byte_diff(&down_off, &down_on);
    assert_eq!(
        down_changed, 0,
        "朝下看（射线打不到界面）时开档改了 {down_changed} 个字节：界面掩码越界了"
    );

    // ③ 几何守卫：河床夹在相机与界面之间 ⇒ 看到的是河床，界面必须让位。
    fixture.with_bed(BLOCKING_BED, true);
    fixture.renderer.set_submerged_view_gain(0.0);
    let Some(blocked_off) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_submerged_view_gain(GAIN);
    let Some(blocked_on) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    let blocked_changed = byte_diff(&blocked_off, &blocked_on);
    assert_eq!(
        blocked_changed, 0,
        "河床挡在界面之前时开档仍改了 {blocked_changed} 个字节：没做「几何比界面更近」的守卫"
    );
    fixture.with_bed(DEEP_BED, false);

    // ④ 窗口在上、全反射在下：朝上看时靠天顶的条带必须明显亮于靠地平线的条带。
    let Some(on_frame) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_submerged_view_gain(0.0);
    let Some(base_frame) = fixture.render(Vec3::new(0.0, 10.0, 2.0)) else {
        return gpu_skipped("hdr_active");
    };
    let changed = byte_diff(&base_frame, &on_frame);
    assert!(
        changed > 0,
        "开档后一像素都没变：submersion 的四个槽没接到着色器"
    );
    let window = patch_luma(&on_frame, &WINDOW_ROWS, &COLS);
    let mirror = patch_luma(&on_frame, &MIRROR_ROWS, &COLS);
    println!("submerged window={window:.3} mirror={mirror:.3} (bytes {changed})");
    assert!(
        window > mirror * 1.35,
        "Snell 窗口（{window:.3}）没有明显亮过窗口外的全反射带（{mirror:.3}）：\
         临界角判定没起作用，界面只是一层常数染色"
    );
    // 关档时同一帧不存在窗口分层：朝上看的整帧只由吸收决定，上下条带的亮度差
    // 必须明显小于开档（否则上面那条可以靠 R173 的吸收凑出来，是假绿）。
    let base_window = patch_luma(&base_frame, &WINDOW_ROWS, &COLS);
    let base_mirror = patch_luma(&base_frame, &MIRROR_ROWS, &COLS);
    println!(
        "off baseline window={base_window:.3} mirror={base_mirror:.3} \
         ratio={:.3} vs on {:.3}",
        base_window / base_mirror.max(1.0),
        window / mirror.max(1.0),
    );
    assert!(
        window / mirror.max(1.0) > base_window / base_mirror.max(1.0) * 1.2,
        "开档的上下条带比（{:.3}）没有明显大于关档（{:.3}）：分层来自吸收而不是界面",
        window / mirror.max(1.0),
        base_window / base_mirror.max(1.0),
    );
}
