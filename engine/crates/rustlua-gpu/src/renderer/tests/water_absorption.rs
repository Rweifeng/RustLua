//! P3-R171/R172：水面逐像素光学吸收与散射（Beer–Lambert + in-scattering）的门。
//!
//! 背景：水面是 `Blend`、不写深度，所以水像素上 `depth_at` 拿到的是**河床**。
//! 光程 = 水位 − 河床 y，逐通道衰减 `T = exp(-k·d)`；散射项 `body·(1 - T)` 把深水
//! 拉回水体自身的颜色 —— 只有吸收时，河床的辐射会被乘到接近 0，深水在画面上收成死黑。
//!
//! 离线一条：着色器契约。吸收与散射必须挂在 `water_surface(uv).valid` 里面 —— 天空与
//! 岸上像素的深度不是河床，若整屏生效，天空（depth = 1 反投影到视锥远点）会被当成
//! 32 m 深水压黑。
//!
//! GPU 门三条：
//!
//! * **不越界**：画面顶部（地平线以上）两帧逐字节相同 —— 吸收没漏到天空；
//! * **变暗 + 逐通道**：河床从 −1 换到 −5，平均亮度显著下降，且红/绿比大幅下降
//!   （只做整体调暗也能过"变暗"这一条，红/绿比才能把逐通道钉住）；
//! * **散射**：河床再换到 −8（光程 8 m，吸收后本该几乎全黑），亮度必须仍在一个
//!   中间值之上、且蓝 > 红 —— 这是 `body·(1 - T)` 的签名。
//!
//! 反面控制（手工执行、记在文档里）：把 `passes.rs` 的 `params[103]`（吸收强度）或
//! `params[107]`（散射强度）临时改成 0，对应断言必须红；改回 1 必须绿。

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
const VERY_DEEP_BED: f32 = -8.0;
const SIZE: u32 = 64;
/// 地平线（相机 y=3 俯视原点、90° 视场）落在第 16 行附近；顶部 12 行一定是天空。
const SKY_ROWS: std::ops::Range<u32> = 0..12;
/// 两个河床高度都覆盖的屏幕区域：河床 8×8、摄像头俯视 ⇒ 第 44–56 行、第 24–40 列。
const BED_ROWS: std::ops::Range<u32> = 44..56;
const BED_COLS: std::ops::Range<u32> = 24..40;
/// −8 m 的河床离相机更远，投影更靠上：第 46–54 行仍在它上面。
const DEEP_ROWS: std::ops::Range<u32> = 46..54;

#[test]
fn tonemap_only_absorbs_inside_the_water_mask() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains("fn water_surface(uv: vec2<f32>) -> WaterSurface"),
        "水像素判定必须抽成一个函数，折射/吸收/散射/反射共用同一份口径"
    );
    assert!(
        source.contains("let optical_depth = clamp(params.water.x - surface.bed.y, 0.0, 32.0);"),
        "光程必须是「水位 − 河床 y」，不是相机距离"
    );
    assert!(
        source.contains(
            "let transmittance = exp(-params.water_abs.rgb * optical_depth * params.water_abs.w);"
        ),
        "Beer–Lambert 必须是逐通道（vec3 消光系数）而不是标量"
    );
    assert!(
        source.contains("let body = params.water_scatter.rgb * params.water_scatter.w;"),
        "散射色要按强度缩放后参与合成"
    );
    assert!(
        source.contains("transmittance + body * (1.0 - transmittance)"),
        "散射权重必须是 1 − T：浅滩几乎不受影响，深水收敛到水体色"
    );
    // 吸收段必须整段位于 `if surface.valid {` 里：找水掩码之后的第一个花括号块，
    // 透射公式必须在它内部。
    let mask = source
        .find("        if surface.valid {")
        .expect("水合成分支必须以水掩码开头");
    let absorb = source
        .find("let transmittance = exp(-params.water_abs.rgb")
        .expect("吸收公式");
    assert!(
        absorb > mask,
        "吸收公式在水掩码之外 ⇒ 天空/岸上像素也会被按远景深度压黑"
    );
    assert!(
        source.contains("linear = mix(linear, refracted, 0.30) * transmittance + body"),
        "透射只能在合成后乘一次（`mix(linear, refracted * T, 0.3) * T` 会把同一段光程算两遍）"
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
    camera: Camera3D,
    scene_view: RenderView,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    fn scene(&self, drawables: Vec<Scene3dDrawable>) -> Scene3d {
        Scene3d {
            camera: self.camera,
            view: self.scene_view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables,
        }
    }

    /// 渲染一帧并回读紧凑 RGBA8（sRGB 编码后的最终画面）。
    fn render(&mut self, scene: &Scene3d) -> Option<Vec<u8>> {
        let mesh = empty_mesh();
        let outcome =
            self.renderer
                .render_into_texture(&mesh, Some(scene), &self.view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "吸收帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("absorption readback");
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
        label: Some("rustlua-test-absorption-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
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
    Some(Fixture {
        renderer,
        camera,
        scene_view,
        target,
        view,
    })
}

/// 区域平均 RGB（0..1），用 sRGB 字节直接平均；只做帧间比较，不需要线性化。
fn patch_mean(pixels: &[u8], rows: &std::ops::Range<u32>, cols: &std::ops::Range<u32>) -> [f32; 3] {
    let mut sum = [0.0f64; 3];
    let mut count = 0.0f64;
    for row in rows.clone() {
        for column in cols.clone() {
            let index = ((row * SIZE + column) * 4) as usize;
            for channel in 0..3 {
                sum[channel] += f64::from(pixels[index + channel]);
            }
            count += 1.0;
        }
    }
    [
        (sum[0] / count / 255.0) as f32,
        (sum[1] / count / 255.0) as f32,
        (sum[2] / count / 255.0) as f32,
    ]
}

fn luma(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn render_bed(fixture: &mut Fixture, height: f32) -> Option<Vec<u8>> {
    let scene = fixture.scene(vec![bed_drawable(height), water_drawable()]);
    fixture.render(&scene)
}

#[test]
fn deep_bed_is_absorbed_more_than_shallow_bed() {
    let Some(mut fixture) = fixture("rustlua-test-absorption-target") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );

    let Some(shallow) = render_bed(&mut fixture, SHALLOW_BED) else {
        return gpu_skipped("hdr_active");
    };
    let Some(deep) = render_bed(&mut fixture, DEEP_BED) else {
        return gpu_skipped("hdr_active");
    };

    // ① 吸收不能漏到天空：顶部若干行必须逐字节相同（水掩码 + 深度守卫）。
    let sky_len = (SKY_ROWS.end * SIZE * 4) as usize;
    assert_eq!(
        shallow[..sky_len],
        deep[..sky_len],
        "地平线以上的像素被吸收改动了：水掩码失效会把天空按远景深度压黑"
    );

    let shallow_rgb = patch_mean(&shallow, &BED_ROWS, &BED_COLS);
    let deep_rgb = patch_mean(&deep, &BED_ROWS, &BED_COLS);
    let shallow_luma = luma(shallow_rgb);
    let deep_luma = luma(deep_rgb);
    println!(
        "absorption shallow={shallow_rgb:?} luma={shallow_luma:.4} deep={deep_rgb:?} luma={deep_luma:.4}"
    );
    assert!(
        shallow_luma > 0.02,
        "浅水河床区域几乎全黑（{shallow_luma}），镜头构图没落在水面上，门是空转"
    );

    // ② 变暗：光程 1 m → 5 m。
    assert!(
        deep_luma < shallow_luma * 0.85,
        "深水河床没有明显变暗（{deep_luma:.4} vs {shallow_luma:.4}）：吸收没生效"
    );

    // ③ 逐通道：红被拿掉得比绿多 ⇒ 红/绿比必须明显下降。
    let shallow_ratio = shallow_rgb[0] / shallow_rgb[1].max(1e-6);
    let deep_ratio = deep_rgb[0] / deep_rgb[1].max(1e-6);
    println!("absorption red/green shallow={shallow_ratio:.4} deep={deep_ratio:.4}");
    assert!(
        deep_ratio < shallow_ratio * 0.9,
        "红/绿比几乎没变（{deep_ratio:.4} vs {shallow_ratio:.4}）：只是整体调暗，不是逐通道吸收"
    );
}

#[test]
fn water_in_scattering_lifts_the_deepest_bed() {
    let Some(mut fixture) = fixture("rustlua-test-scattering-target") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    let Some(pixels) = render_bed(&mut fixture, VERY_DEEP_BED) else {
        return gpu_skipped("hdr_active");
    };
    let rgb = patch_mean(&pixels, &DEEP_ROWS, &BED_COLS);
    let value = luma(rgb);
    println!("scattering deep8={rgb:?} luma={value:.4}");
    // 8 m 光程下吸收只剩 (0.012, 0.202, 0.326)：没有散射时整块区域会被压到 ~0.10，
    // 有散射时收敛到水体色 ~0.20。门槛落在两者之间。
    assert!(
        value > 0.14,
        "8 m 深水被压到 {value:.4}：散射没生效（只有吸收时深水会收成死黑）"
    );
    assert!(
        rgb[2] > rgb[0] * 1.2,
        "深水不是水体色（{rgb:?}）：蓝必须明显高于红"
    );
}

/// 反例：没有水面时整段合成不执行，画面不该被吸收影响（与浅水帧同构，只少水面）。
#[test]
fn absorption_stays_off_without_a_water_plane() {
    let Some(mut fixture) = fixture("rustlua-test-absorption-negative") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    // 同一块河床、两种高度，但完全没有水面：两帧的河床区域应只差在几何投影上，
    // 不存在"深处更暗"的逐通道吸收。这里只钉一条：水掩码关掉后帧仍然出图（不会因
    // 缺少水面而读未初始化的 uniform），且河床区域不为全黑。
    let dry_shallow = fixture.scene(vec![bed_drawable(SHALLOW_BED)]);
    let Some(pixels) = fixture.render(&dry_shallow) else {
        return gpu_skipped("hdr_active");
    };
    let rgb = patch_mean(&pixels, &BED_ROWS, &BED_COLS);
    assert!(
        luma(rgb) > 0.02,
        "没有水面时河床区域全黑（{rgb:?}），反例构图无效"
    );
}
