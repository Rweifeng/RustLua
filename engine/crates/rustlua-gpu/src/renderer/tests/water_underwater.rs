//! P3-R173：相机入水时的整帧体积吸收门。
//!
//! 与 P3-R171 的区别：吸收那条只在**水面像素**上按「水位 − 河床 y」算光程；相机潜到水下时，
//! 整帧都隔着水，改成「相机 → 该像素世界点」的距离。宿主按「相机在水面矩形内且低于水位」
//! 自动开关（`params[108]`），没有新增场景字段。
//!
//! 离线一条：着色器契约（函数存在、距离取相机到世界点、有 40 m 光程上限、整段落在剖面封口
//! 之后闪白之前）+ 宿主按相机高度判定。
//!
//! GPU 一条：同一块地面（32×32、y = −3），两张帧只改相机高度（水面下 1.5 m / 水面上 3 m）。
//! * **距离相关**：水下远景的红通道必须掉到近景 0.6 以下 —— 常数染色过不了这条
//!   （亮度被散射的 `body` 托底，深水收敛到水体色而不是黑，所以判红通道）；
//! * **逐通道**：水下远景收向水体（蓝/红 > 3）—— 只整体调暗过不了这条；
//! * **只在水下**：水面上那帧是「隔 3 m 水柱看河床」、光程恒定 ⇒ 远近比≈1；若那里也掉，
//!   说明落差来自几何/光照而不是相机入水。
//!
//! 反面控制（手工执行、记在文档里）：把 `passes.rs` 的 `params[108]` 固定成 0 ⇒ 水下远景
//! 恢复亮度，第一条断言必须红。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const LEVEL: f32 = 0.0;
const BED_Y: f32 = -3.0;
const SIZE: u32 = 64;
const COLS: std::ops::Range<u32> = 24..40;
/// 近景地面（相机脚下 2 m 量级）。
const NEAR_ROWS: std::ops::Range<u32> = 56..62;
/// 远景地面（接近地平线，20 m 量级；地面缩放到 32×32 才覆盖得到）。
const FAR_ROWS: std::ops::Range<u32> = 34..40;

#[test]
fn tonemap_applies_underwater_only_between_the_cut_and_the_flash() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(
        source.contains("fn underwater_transmittance(uv: vec2<f32>) -> vec3<f32>"),
        "水下体积吸收要抽成函数，光程口径与水面吸收分开"
    );
    assert!(
        source.contains(
            "clamp(length(point - params.camera_position.xyz), 0.0, params.water_under.y)"
        ),
        "光程必须是「相机 → 该像素世界点」的距离，并被上限截断"
    );
    assert!(
        source.contains("params.water_under.x > 0.5"),
        "整段必须由相机入水开关控制"
    );
    assert!(
        source.contains("linear * transmittance + body * (1.0 - transmittance)"),
        "合成式必须与水面吸收同一形状（透射 + 散射补集）"
    );
    let cut = source.find("let cut = section_cut(uv);").expect("剖面封口");
    let under = source
        .find("if params.water_under.x > 0.5 {")
        .expect("水下吸收分支");
    let flash = source.find("params.cloud.y * 0.55").expect("闪白叠加");
    assert!(
        cut < under && under < flash,
        "水下吸收要在剖面封口之后、闪白之前，否则土壤剖面/闪光不参与水体光程"
    );

    let host = include_str!("../passes.rs");
    assert!(
        host.contains("params[41] < plane[0]")
            && host.contains("params[108] = if underwater { 1.0 } else { 0.0 };"),
        "宿主必须按「相机低于水位 + 在水面矩形内」写开关"
    );
    assert!(
        host.contains("params[109] = 40.0;"),
        "光程上限要由宿主给（0 = 关，不能靠默认零值凑）"
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

/// 默认接收面是 8×8，放大 4 倍才有 20 m 量级的远景地面可同帧比较。
fn bed_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(3),
        name: "Bed".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, BED_Y, 0.0))
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
}

impl Fixture {
    fn render(&mut self, eye: Vec3) -> Option<Vec<u8>> {
        let camera = Camera3D::perspective(
            eye,
            Vec3::new(0.0, -3.0, -1.0),
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
            drawables: vec![bed_drawable(), water_drawable()],
        };
        let mesh = empty_mesh();
        let outcome =
            self.renderer
                .render_into_texture(&mesh, Some(&scene), &self.view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "水下帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("underwater readback");
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
        label: Some("rustlua-test-underwater-device"),
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

#[test]
fn underwater_absorption_falls_off_with_camera_distance() {
    let Some(mut fixture) = fixture("rustlua-test-underwater-target") else {
        return gpu_skipped("request_adapter");
    };

    // 水面上（相机 y = +3）：开关为 0，远近比只反映材质/光照。
    let Some(above) = fixture.render(Vec3::new(0.0, 3.0, 6.0)) else {
        return gpu_skipped("hdr_active");
    };
    // 水面下（相机 y = −1.5，在水面矩形内）：整帧隔着水。
    let Some(under) = fixture.render(Vec3::new(0.0, -1.5, 6.0)) else {
        return gpu_skipped("hdr_active");
    };

    for row in (30..64).step_by(2) {
        let band = patch_mean(&above, &(row..row + 1), &COLS);
        let under_band = patch_mean(&under, &(row..row + 1), &COLS);
        println!(
            "row {row}: above={:.4} under={:.4}",
            luma(band),
            luma(under_band)
        );
    }

    let above_near = patch_mean(&above, &NEAR_ROWS, &COLS);
    let above_far = patch_mean(&above, &FAR_ROWS, &COLS);
    let under_near = patch_mean(&under, &NEAR_ROWS, &COLS);
    let under_far = patch_mean(&under, &FAR_ROWS, &COLS);
    println!(
        "underwater above_near={above_near:?} above_far={above_far:?} under_near={under_near:?} under_far={under_far:?}"
    );
    println!(
        "ratios above={:.4} under={:.4}",
        luma(above_far) / luma(above_near),
        luma(under_far) / luma(under_near)
    );

    // ① 距离相关：红通道（消光最快）在水下远景必须掉到近景的一半以下。
    //    亮度会被散射补的 `body` 托底（深水最终收敛到水体色，不是黑），所以判据落在红通道。
    assert!(
        under_far[0] < under_near[0] * 0.6,
        "水下远景红通道没有掉下去（near {:.4} / far {:.4}）：光程没接上或常数染色",
        under_near[0],
        under_far[0]
    );
    // ② 逐通道：远景色相收向水体（青蓝），只做整体调暗过不了这条。
    assert!(
        under_far[2] > under_far[0] * 3.0,
        "水下远景没有收向水体色（rgb {:?}）：蓝/红只有 {:.2}",
        under_far,
        under_far[2] / under_far[0]
    );
    // ③ 只在水下：水面上那帧同样是「隔着 3 m 水柱看河床」，但光程恒定 ⇒ 远近比≈1。
    //    若远景在两张帧里都掉下去，说明落差来自几何/光照而不是相机入水。
    assert!(
        above_far[0] > above_near[0] * 0.85,
        "水面上（3 m 水柱）远近红通道比已经 {:.4}，与水下接近：落差不是相机入水造成的",
        above_far[0] / above_near[0]
    );
}
