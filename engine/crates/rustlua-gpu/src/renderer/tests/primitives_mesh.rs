//! M82-A：参数化基本体哨兵（`__primitive__`）的渲染侧门（绘制计数 + 像素 + 确定性）。
//!
//! 三件事：
//! * **六种基本体都真的进 GPU**：`__primitive__` 路径由 `primitives::PrimitiveSpec` 在 CPU 侧
//!   合成几何，渲染器按路径缓存。每种都断言 `draw_calls>=1`、`dropped_draws==0`、三角形数
//!   不少于几何账，且像素与背景不同——「屏幕上少一块、日志里一个字都没有」是本仓最不能
//!   接受的失败形状，故计数与像素双盯。
//! * **确定性**：同一规格渲染两帧逐字节相同（时钟已钉住）。这是「规格串就是全部输入」的
//!   渲染侧凭证：跨平台/打包后只要规格相同，画面就相同。
//! * **反面控制（防 gpu_skipped 假绿）**：空场景必须 `draw_calls==0`。零 draw 帧
//!   `hdr_active=false` 却照样 pass，所以「有几何」的判据落在 `draw_calls>0` 上，而不是
//!   只看像素门有没有跑——清空 drawable 时上面那条 `>=1` 会红，本条证明它真的会红。
//!
//! 反面控制（跑完把读数记进 M82-00 验收证据）：
//! * 把哨兵派发里 `__primitive__` 分支删掉 ⇒ 六种基本体全落进文件加载、`draw_calls==0`，
//!   「每种都进 GPU」那条红。
//! * 把 `build_mesh` 的索引清空 ⇒ 三角形数那条红。
//! * 把本文件 `draw_calls>=1` 改成 `>=0` ⇒ 空场景也绿，反面控制失去意义（刻意不这么写）。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::primitives::PrimitiveSpec;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 斜上方看原点处的单位基本体：plane 的 +Y 面也朝相机。
const EYE: Vec3 = Vec3::new(2.5, 2.0, 2.5);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);

/// 六种基本体 + 各自的几何三角形账（不含后期全屏 quad，故门用 `>=`）。
fn kinds() -> Vec<(&'static str, PrimitiveSpec, u64)> {
    vec![
        (
            "box",
            PrimitiveSpec::Box {
                w: 1.0,
                h: 1.0,
                d: 1.0,
            },
            12,
        ),
        ("sphere", PrimitiveSpec::Sphere { r: 0.6, seg: 16 }, 256),
        (
            "cylinder",
            PrimitiveSpec::Cylinder {
                r: 0.5,
                h: 1.0,
                seg: 16,
            },
            64,
        ),
        (
            "cone",
            PrimitiveSpec::Cone {
                r: 0.5,
                h: 1.0,
                seg: 16,
            },
            32,
        ),
        (
            "torus",
            PrimitiveSpec::Torus {
                big_r: 0.5,
                r: 0.2,
                seg: 24,
                ring_seg: 12,
            },
            576,
        ),
        ("plane", PrimitiveSpec::Plane { w: 1.5, h: 1.5 }, 2),
    ]
}

fn primitive_drawable(spec: &PrimitiveSpec) -> Scene3dDrawable {
    let (min, max) = spec.bounds();
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Primitive".to_owned(),
        mesh_path: spec.mesh_path(),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(min[0], min[1], min[2]),
                max: Vec3::new(max[0], max[1], max[2]),
            },
            source: BoundsSource::Fallback,
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

struct Frame {
    pixels: Vec<u8>,
    draw_calls: u64,
    triangles: u64,
    dropped: u64,
    hdr: bool,
}

struct Fixture {
    renderer: Renderer,
    viewport: Viewport,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    /// 按一组 drawable 渲染一帧，回读紧凑 RGBA8 与本帧计数。
    fn render(&mut self, drawables: Vec<Scene3dDrawable>) -> Frame {
        let camera = Camera3D::perspective(EYE, CENTER, std::f32::consts::FRAC_PI_2, 0.1, 200.0)
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
            drawables,
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
            "基本体帧必须出图"
        );
        let metrics = self.renderer.last_frame_metrics();
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("primitive readback");
        Frame {
            pixels,
            draw_calls: metrics.draw_calls,
            triangles: metrics.triangles,
            dropped: metrics.dropped_draws,
            hdr: metrics.hdr_active,
        }
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
        label: Some("rustlua-test-primitives-device"),
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
    renderer.set_post_fx(0.0, 0.0);
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
    })
}

fn byte_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// 六种基本体都进 GPU（计数 + 像素），且同规格两帧逐字节相同。
#[test]
fn every_primitive_kind_reaches_gpu_and_is_deterministic() {
    let Some(mut fixture) = fixture("rustlua-test-primitives-kinds") else {
        return gpu_skipped("request_adapter");
    };
    // 背景参考帧：空场景。基本体帧必须与它不同，否则「画了」只是计数不是像素。
    let background = fixture.render(Vec::new());
    for (label, spec, min_triangles) in kinds() {
        let frame = fixture.render(vec![primitive_drawable(&spec)]);
        println!(
            "primitive_{label} draws={} triangles={} dropped={} hdr={}",
            frame.draw_calls, frame.triangles, frame.dropped, frame.hdr
        );
        // 计数判据必须落在 gpu_skipped 之前：清空 drawable ⇒ draw_calls==0 要红，
        // 不能被 hdr_active=false 的跳过分支冒充成绿（M82 反面对照实测过的假绿形状）。
        assert!(frame.draw_calls >= 1, "{label} 必须产生 draw");
        assert_eq!(frame.dropped, 0, "{label} 的 draw 不许被静默丢弃");
        assert!(
            frame.triangles >= min_triangles,
            "{label} 几何三角形 {} 必须全进这一帧，实测 {}",
            min_triangles,
            frame.triangles
        );
        if !frame.hdr {
            return gpu_skipped("hdr_active");
        }
        let diff = byte_diff(&background.pixels, &frame.pixels);
        assert!(diff > 0, "{label} 必须改像素（与背景逐字节相同 = 没画上）");
        // 确定性：同一规格再渲一帧，逐字节相同。
        let again = fixture.render(vec![primitive_drawable(&spec)]);
        assert_eq!(
            byte_diff(&frame.pixels, &again.pixels),
            0,
            "{label} 同规格两帧必须逐字节相同"
        );
    }
}

/// 反面控制：清空 drawable ⇒ 零 draw。证明上面 `draw_calls>=1` 真的会因「没几何」而红，
/// 而不是被 gpu_skipped 假绿绕过去。
#[test]
fn empty_scene_has_zero_draws() {
    let Some(mut fixture) = fixture("rustlua-test-primitives-empty") else {
        return gpu_skipped("request_adapter");
    };
    let frame = fixture.render(Vec::new());
    println!(
        "primitive_empty draws={} triangles={} dropped={}",
        frame.draw_calls, frame.triangles, frame.dropped
    );
    assert_eq!(frame.draw_calls, 0, "空场景必须零 draw");
    assert_eq!(frame.triangles, 0, "空场景必须零三角形");
    assert_eq!(frame.dropped, 0, "空场景没有可丢弃的 draw");
}

/// 坏规格串走「跳过并留痕」：解析失败 ⇒ 零 draw、零丢弃，不静默丢一块。
#[test]
fn invalid_primitive_spec_is_skipped_not_dropped() {
    let Some(mut fixture) = fixture("rustlua-test-primitives-bogus") else {
        return gpu_skipped("request_adapter");
    };
    let bogus = "__primitive__?kind=pyramid".to_owned();
    let frame = fixture.render(vec![primitive_drawable(&PrimitiveSpec::Plane {
        w: 1.0,
        h: 1.0,
    })]);
    // 先用一张合法 plane 确认夹具能画，再换坏串。
    assert!(frame.draw_calls >= 1, "夹具本身必须能画");
    let mut drawable = primitive_drawable(&PrimitiveSpec::Plane { w: 1.0, h: 1.0 });
    drawable.mesh_path = bogus;
    let bad = fixture.render(vec![drawable]);
    println!(
        "primitive_bogus draws={} triangles={} dropped={}",
        bad.draw_calls, bad.triangles, bad.dropped
    );
    assert_eq!(bad.draw_calls, 0, "坏规格必须跳过");
    assert_eq!(bad.dropped, 0, "坏规格不是「丢 draw」，是根本没提交");
}
