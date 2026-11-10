//! P3-R237：3D 实例批次（crowd）的渲染侧门。
//!
//! 批次把 N 份实例变换收进**一个** drawable（`Scene3dDrawable::instances`），渲染侧仍走
//! 同一条已存在的实例化路径（`instance_models` 存储缓冲 + `draw_indexed(..., 0..instances)`）。
//! 本门钉四件事：
//!
//! * **像素等价**：一个批次 drawable（N 份实例）与 N 个普通 drawable（同一批位置）必须
//!   **逐字节相同**——批次只是"多份变换"的另一种装法，不许改出任何画面差别。
//! * **反面控制**：把批次里的实例全挪到同一位置 ⇒ 画面必须变。否则上面那条是**空洞通过**
//!   （两帧都没画东西也会"逐字节相同"）。
//! * **一次实例化绘制**：N 份实例合进**一条** draw（`models.len() == N`），而不是 N 条 draw。
//! * **逐实例剔除**：视锥外的那一只被剔掉，同批的其它实例照画；候选数按**实例**计。
//!
//! 网格用地面哨兵（渲染器自合成，不依赖工程资源）；朝向靠绕 X 转 90°，让平面正对相机。

use super::*;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::{Mat4, Quat, Vec3};
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 相机在 z=+4 朝 -z 看；视锥在 z=0 平面上半宽 4 个单位 ⇒ 1 单位 = 8 像素。
const EYE: Vec3 = Vec3::new(0.0, 0.0, 4.0);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// 四只"蚂蚁"的横向摆位（单位间距 1.8，避开 ±4 的画面边缘）。
const SLOTS: [f32; 4] = [-2.7, -0.9, 0.9, 2.7];

/// 地面哨兵正对相机的朝向（绕 X 转 90° ⇒ 法线 +Y 变 +Z）。
fn facing() -> Mat4 {
    Mat4::from_quat(Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2).unwrap())
}

/// 单只蚂蚁的世界矩阵（摆位 + 朝向 + 缩到 1.2×1.2）。
fn ant_model(x: f32) -> Mat4 {
    Mat4::translation(Vec3::new(x, 0.0, 0.0)) * facing() * Mat4::scale(Vec3::new(0.15, 0.15, 0.15))
}

/// 一个批次 drawable：网格是地面哨兵，实例是 N 份世界矩阵。
fn batch_drawable(models: &[Mat4]) -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(1),
        name: "Crowd".to_owned(),
        mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        // 批次实体自身的变换不参与（实例坐标是世界坐标）。
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: models.to_vec(),
        bounds: Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-4.0, -4.0, -0.1),
                max: Vec3::new(4.0, 4.0, 0.1),
            },
            source: BoundsSource::Fallback,
        },
    }
}

/// N 个普通 drawable：同一批位置，逐个实体摆（对照组）。
fn separate_drawables(models: &[Mat4]) -> Vec<Scene3dDrawable> {
    models
        .iter()
        .enumerate()
        .map(|(index, model)| {
            let mut drawable = batch_drawable(&[]);
            drawable.entity = rustlua_scene::scene::SceneEntityId(index as u64 + 1);
            drawable.name = format!("Ant-{index}");
            drawable.model = *model;
            drawable
        })
        .collect()
}

fn scene_with(drawables: Vec<Scene3dDrawable>, viewport: Viewport) -> Scene3d {
    let camera = Camera3D::perspective(EYE, CENTER, std::f32::consts::FRAC_PI_2, 0.1, 100.0)
        .expect("perspective camera");
    Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables,
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
    /// 按 drawable 列表渲染一帧，回读紧凑 RGBA8 与本帧计数（时钟已在建夹具时钉住）。
    fn render(&mut self, drawables: Vec<Scene3dDrawable>) -> Frame {
        let scene = scene_with(drawables, self.viewport);
        self.renderer.set_frame_views(FrameViews {
            main: Some(scene.view),
            overlay: None,
        });
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
            "批次帧必须出图"
        );
        let metrics = self.renderer.last_frame_metrics();
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("batch readback");
        Frame {
            pixels,
            draw_calls: metrics.draw_calls,
            triangles: metrics.triangles,
            dropped: metrics.dropped_draws,
            hdr: metrics.hdr_active,
        }
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

fn fixture(label: &str) -> Option<Fixture> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-instance-batch-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, true, true)
        .expect("offscreen renderer");
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    // 后期颗粒与折射都会自己改像素，留着只会给像素差再添两个来源。
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

/// 两帧之间取值不同的字节个数。
fn byte_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// 像素等价 + 反面控制：批次与逐个实体画同一批位置必须逐字节相同；把实例全挪到
/// 同一位置后画面必须变（否则"逐字节相同"是空洞通过）。
#[test]
fn batch_drawable_renders_identically_to_separate_drawables() {
    let Some(mut fixture) = fixture("rustlua-test-instance-batch-pixels") else {
        return gpu_skipped("request_adapter");
    };
    let models: Vec<Mat4> = SLOTS.iter().map(|x| ant_model(*x)).collect();

    let separate = fixture.render(separate_drawables(&models));
    let batch = fixture.render(vec![batch_drawable(&models)]);
    println!(
        "batch_pixels separate={:?} batch={:?} hdr={}{}",
        (separate.draw_calls, separate.triangles),
        (batch.draw_calls, batch.triangles),
        separate.hdr,
        batch.hdr
    );
    if !separate.hdr || !batch.hdr {
        return gpu_skipped("hdr_active");
    }
    assert!(separate.draw_calls >= 1, "对照组必须真的画了东西");
    assert_eq!(batch.dropped, 0, "批次 draw 不许被静默丢弃");

    let same = byte_diff(&separate.pixels, &batch.pixels);
    println!("batch_pixels diff_bytes={same}");
    assert_eq!(
        same, 0,
        "一个批次 drawable 与 N 个普通 drawable 必须逐字节相同（实测 {same} 字节不同）"
    );

    // 反面控制：四只蚂蚁全挤到同一个位置 ⇒ 画面必须与四只分开摆不同。
    let stacked: Vec<Mat4> = vec![ant_model(SLOTS[0]); SLOTS.len()];
    let stacked_frame = fixture.render(vec![batch_drawable(&stacked)]);
    let stacked_diff = byte_diff(&batch.pixels, &stacked_frame.pixels);
    println!("batch_pixels stacked_diff_bytes={stacked_diff}");
    assert!(
        stacked_diff > 0,
        "反面控制：实例全挤到一处时画面必须变（实测 {stacked_diff} 字节不同）"
    );
}

/// 一次实例化绘制 + 逐实例剔除：N 份实例合进一条 draw；视锥外的那一只被剔掉，
/// 同批的其它实例照画；候选数按**实例**计（不是按 drawable 计）。
#[test]
fn batch_instances_share_one_draw_and_cull_per_instance() {
    let Some((device, queue)) = (|| {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-test-instance-batch-counts"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device");
        Some((device, queue))
    })() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, true, true).expect("offscreen renderer");

    let models: Vec<Mat4> = SLOTS.iter().map(|x| ant_model(*x)).collect();
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = scene_with(vec![batch_drawable(&models)], viewport);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(
        draws.len(),
        1,
        "四份实例必须合进**一条**实例化绘制，而不是四条 draw"
    );
    assert_eq!(
        draws[0].models.len(),
        SLOTS.len(),
        "这一条 draw 必须带着四份模型矩阵"
    );
    assert_eq!(
        renderer.frame_cull_candidates,
        SLOTS.len() as u64,
        "候选数按**实例**计（批次是 1 个 drawable、4 份实例）"
    );
    assert_eq!(renderer.frame_cull_frustum, 0, "四只在视锥内");

    // 逐实例剔除：把第三只挪到 far=100 之外（x=500），同批的其它三只照画。
    let mut far_models = models.clone();
    far_models[2] = ant_model(500.0);
    let scene = scene_with(vec![batch_drawable(&far_models)], viewport);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(
        renderer.frame_cull_candidates,
        SLOTS.len() as u64,
        "剔掉的那只仍进候选分母（剔除率口径不被剔除本身污染）"
    );
    assert_eq!(
        renderer.frame_cull_frustum, 1,
        "只有视锥外那一只被剔掉，同批其它实例不受牵连"
    );
    assert_eq!(
        draws.iter().map(|draw| draw.models.len()).sum::<usize>(),
        SLOTS.len() - 1,
        "留下的三只必须照画"
    );
}
