//! P3-R245：帧路径 GPU 位姿接线的门（暂存批次 → compute → 单独一条 draw）。
//!
//! 钉四件事：
//!
//! * **默认关档逐字节不变**：只 `stage_motion_batch`、不开 `set_gpu_motion_batches` 时，帧与
//!   CPU 参考帧逐字节相同（暂存只是登记，一个字节都不取走）。
//! * **GPU 档与 CPU 档同画**：同一批次、同一冻结时刻，开档后 draw 数与三角形数一致，像素差
//!   只允许着色器 `sin/cos` 末位差（少量字节 + 每通道小值），且画面**真的出了图**（与空场景差分 > 0）。
//! * **反面对照**：改 staged 时刻（整体平移一个世界单位）⇒ 画面必须变；把实例全挤到一处 ⇒
//!   画面也必须变。否则"同画"可能只是烘焙失败后静默回退 CPU 矩阵的假绿。
//! * **暂存不跨帧残留**：开档帧把登记取走；关档帧不取（输入原样留着，默认路径不碰）。

use super::*;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::instance_batch::{
    BatchMotion3D, Instance3D, InstanceBatch3D, InstanceMotion3D, MotionKind,
};
use rustlua_render::light::Light;
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 相机在 z=+4 朝 -z 看；视锥在 z=0 平面上半宽 4 个单位 ⇒ 1 单位 = 8 像素。
const EYE: Vec3 = Vec3::new(0.0, 0.0, 4.0);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// 四只"蚂蚁"的横向摆位（单位间距 1.8，避开 ±4 的画面边缘）。
const SLOTS: [f32; 4] = [-2.7, -0.9, 0.9, 2.7];
/// 冻结时钟：CPU 参考帧与 staged 时刻用同一个数（同一时刻比位姿，不掺时钟漂移）。
const FROZEN: f32 = 4.2;
/// 路径覆盖 x ∈ [-8, 8]；实例偏移写成"槽位 + 8 − 冻结点"⇒ 冻结时刻正好落在槽位上。
const PATH_ENDS: [f32; 2] = [-8.0, 8.0];
const ANT_SCALE: f32 = 0.15;

/// 地面哨兵正对相机的朝向：绕 X 转 90°（法线 +Y 变 +Z），写成四元数 (x, y, z, w)。
fn facing_quat() -> [f32; 4] {
    let half = std::f32::consts::FRAC_PI_4;
    [half.sin(), 0.0, 0.0, half.cos()]
}

/// 一条从 x=-8 到 x=+8 的开口路径：`slots` 里的每只在冻结时刻恰好落在自己的槽位上。
///
/// 逐实例速度给 1.0（不是 0）：改 staged 时刻时姿态必须整体平移，反面对照才有东西可变。
fn motion_batch(slots: &[f32]) -> InstanceBatch3D {
    motion_batch_at(slots, 0.0)
}

/// 同上，但整条路径平移到 `z`（P3-R252 用：把一批挪到剔除球外，又不离开视锥）。
fn motion_batch_at(slots: &[f32], z: f32) -> InstanceBatch3D {
    motion_batch_scaled(slots, z, ANT_SCALE)
}

/// P3-R255 用：整批缩放可换（让网格局部半径 × 缩放真的伸进/伸出视锥）。
fn motion_batch_scaled(slots: &[f32], z: f32, scale: f32) -> InstanceBatch3D {
    let path = vec![[PATH_ENDS[0], 0.0, z], [PATH_ENDS[1], 0.0, z]];
    let instances = slots
        .iter()
        .map(|slot| Instance3D {
            position: [*slot, 0.0, 0.0],
            rotation: facing_quat(),
            scale: [scale; 3],
            motion: Some(InstanceMotion3D {
                offset: *slot - PATH_ENDS[0] - FROZEN,
                speed: 1.0,
                align: false,
                ..InstanceMotion3D::default()
            }),
        })
        .collect();
    InstanceBatch3D {
        mesh: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        instances,
        motion: Some(BatchMotion3D {
            kind: MotionKind::Path,
            path,
            closed: false,
            speed: 1.0,
        }),
    }
}

/// 一个批次 drawable：网格是地面哨兵，实例是 N 份世界矩阵（GPU 档下只作包围盒与回退）。
fn batch_drawable(models: &[Mat4]) -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(1),
        name: "Crowd".to_owned(),
        mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
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
            "GPU 位姿帧必须出图"
        );
        let metrics = self.renderer.last_frame_metrics();
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("motion frame readback");
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
        label: Some(label),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, true, true)
        .expect("offscreen renderer");
    // 没有 compute 能力的适配器上这条门只能跳过（如实报 gpu_skipped，不当通过）。
    if !renderer.compute_enabled() {
        return None;
    }
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
    renderer.freeze_clock_for_tests(FROZEN);
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

/// 两帧之间逐通道差值的最大绝对值。
fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// P3-R246：宿主**不再给 CPU 矩阵**（`instances` 为空）时，GPU 档仍与 CPU 参考帧同画。
///
/// 这是"跳过 `matrices_at`"真正兑现的那一步：实例数取自批次，包围盒换成路径 AABB 外扩
/// 最大实例半径（保守）。若空 `instances` 被当成"没有可画的实例"，画面会整块消失；
/// 若包围盒算小了，阴影/平面反射会在剔除阶段丢掉这批几何——两种错法都会让像素差爆表。
#[test]
fn gpu_motion_frame_without_cpu_matrices_draws_the_same() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-no-cpu") else {
        return gpu_skipped("request_adapter/compute");
    };
    let batch = motion_batch(&SLOTS);
    let cpu_models = batch.matrices_at(FROZEN);
    let cpu = fixture.render(vec![batch_drawable(&cpu_models)]);
    if !cpu.hdr {
        return gpu_skipped("hdr_active");
    }
    fixture.renderer.set_gpu_motion_batches(true);
    fixture.renderer.stage_motion_batch(1, batch, FROZEN);
    // 关键：drawable 一个实例矩阵都不带（宿主侧把 `matrices_at` 省了）。
    let gpu = fixture.render(vec![batch_drawable(&[])]);
    println!(
        "gpu_motion_no_cpu cpu=({}, {}) gpu=({}, {}) dropped={}",
        cpu.draw_calls, cpu.triangles, gpu.draw_calls, gpu.triangles, gpu.dropped
    );
    assert_eq!(
        gpu.draw_calls, cpu.draw_calls,
        "空矩阵档的 draw 数必须与参考帧一致"
    );
    assert_eq!(gpu.triangles, cpu.triangles, "三角形数必须与参考帧一致");
    assert_eq!(gpu.dropped, 0, "空矩阵档不许静默丢 draw");
    let empty = fixture.render(Vec::new());
    let drawn = byte_diff(&gpu.pixels, &empty.pixels);
    println!("gpu_motion_no_cpu drawn_bytes={drawn}");
    assert!(
        drawn > 0,
        "空矩阵档必须真的出了图（与空场景差分为 0 = 实例被当成空表跳过）"
    );
    let diff = byte_diff(&cpu.pixels, &gpu.pixels);
    let max_delta = max_channel_delta(&cpu.pixels, &gpu.pixels);
    println!("gpu_motion_no_cpu diff_bytes={diff} max_delta={max_delta}");
    assert!(
        diff * 100 <= gpu.pixels.len(),
        "空矩阵档与 CPU 参考帧的像素差应只落在边沿抖动（实测 {diff}/{} 字节）",
        gpu.pixels.len()
    );
    assert!(
        max_delta <= 16,
        "单通道最大差 {max_delta} 超出末位差范围（包围盒或实例数口径分叉）"
    );
}

/// GPU 档与 CPU 档同画；两层反面对照证明门在判位姿、不是"跑完了"。
#[test]
fn gpu_motion_frame_draws_the_same_poses_as_cpu() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-frame") else {
        return gpu_skipped("request_adapter/compute");
    };
    let batch = motion_batch(&SLOTS);
    let cpu_models = batch.matrices_at(FROZEN);
    assert_eq!(cpu_models.len(), SLOTS.len(), "CPU 侧四只都该合法");
    // `Scene3dDrawable` 不实现 `Clone`：每帧现造一份（同一批模板矩阵）。
    let drawables = || vec![batch_drawable(&cpu_models)];

    // 参考帧：CPU 档（不开档、不 stage），模板矩阵就是 `matrices_at(FROZEN)`。
    let cpu = fixture.render(drawables());
    if !cpu.hdr {
        return gpu_skipped("hdr_active");
    }
    assert!(cpu.draw_calls >= 1, "参考帧必须真的画了东西");
    assert_eq!(cpu.dropped, 0, "参考帧不许静默丢 draw");

    // GPU 档：同一场景 + 同一批次 + 同一冻结时刻。
    fixture.renderer.set_gpu_motion_batches(true);
    fixture
        .renderer
        .stage_motion_batch(1, batch.clone(), FROZEN);
    let gpu = fixture.render(drawables());
    assert!(
        fixture.renderer.staged_motion.is_empty(),
        "开档帧必须把暂存取走（不跨帧残留）"
    );
    println!(
        "gpu_motion_frame cpu=({}, {}) gpu=({}, {}) dropped={} hdr={}",
        cpu.draw_calls, cpu.triangles, gpu.draw_calls, gpu.triangles, gpu.dropped, gpu.hdr
    );
    assert_eq!(
        gpu.draw_calls, cpu.draw_calls,
        "GPU 批是单独一条 draw：draw 数必须与 CPU 档一致"
    );
    assert_eq!(gpu.triangles, cpu.triangles, "三角形数必须与 CPU 档一致");
    assert_eq!(gpu.dropped, 0, "GPU 档不许静默丢 draw");

    // 非空洞：画面必须与空场景不同（两帧都没画东西也能"差值为零"）。
    let empty = fixture.render(Vec::new());
    let drawn = byte_diff(&gpu.pixels, &empty.pixels);
    println!("gpu_motion_frame drawn_bytes={drawn}");
    assert!(
        drawn > 0,
        "GPU 档必须真的出了图（空场景差分为 0 就是空洞通过）"
    );

    // 同位姿对账：只允许着色器 sin/cos 末位差造成的像素抖动。
    let diff = byte_diff(&cpu.pixels, &gpu.pixels);
    let max_delta = max_channel_delta(&cpu.pixels, &gpu.pixels);
    println!("gpu_motion_frame diff_bytes={diff} max_delta={max_delta}");
    assert!(
        diff * 100 <= gpu.pixels.len(),
        "GPU 档与 CPU 档的像素差应只落在边沿抖动（实测 {diff}/{} 字节），不是另一种位姿",
        gpu.pixels.len()
    );
    assert!(
        max_delta <= 16,
        "单通道最大差 {max_delta} 超出末位差范围（位姿或法线口径分叉）"
    );

    // 反面对照 ①：staged 时刻整体 +1 秒 ⇒ 四只各挪一个世界单位（8 像素）⇒ 画面必须变。
    // 如果 staged 时刻没被真正用上（比如静默回退 CPU 矩阵），这一帧会和上一帧一模一样。
    // 门限取帧字节的 2%：这四只各只有 ~1.2 像素宽，实测差 1260 字节（7.7%），余量足够；
    // 而"位姿没被用上"的值恒为 0，任何非零门限都能抓住它。
    fixture
        .renderer
        .stage_motion_batch(1, batch.clone(), FROZEN + 1.0);
    let shifted = fixture.render(drawables());
    let shifted_diff = byte_diff(&gpu.pixels, &shifted.pixels);
    println!("gpu_motion_frame shifted_diff_bytes={shifted_diff}");
    assert!(
        shifted_diff > gpu.pixels.len() / 50,
        "反面对照①：staged 时刻平移一个单位后画面必须变化，实测 {shifted_diff} 字节（出图 {drawn} 字节）"
    );

    // 反面对照 ②：四只全挤到同一槽位 ⇒ 画面必须变（证明 GPU 档读的是批次实例，不是回退矩阵）。
    let stacked = motion_batch(&[SLOTS[0]; 4]);
    fixture.renderer.stage_motion_batch(1, stacked, FROZEN);
    let stacked_frame = fixture.render(drawables());
    let stacked_diff = byte_diff(&gpu.pixels, &stacked_frame.pixels);
    println!("gpu_motion_frame stacked_diff_bytes={stacked_diff}");
    assert!(
        stacked_diff > gpu.pixels.len() / 50,
        "反面对照②：实例全挤到一处时画面必须变化，实测 {stacked_diff} 字节（出图 {drawn} 字节）"
    );

    // 默认关档：stage 只登记，帧与 CPU 参考逐字节相同，且暂存没有被取走。
    fixture.renderer.set_gpu_motion_batches(false);
    fixture.renderer.stage_motion_batch(1, batch, FROZEN);
    let off = fixture.render(drawables());
    let off_diff = byte_diff(&off.pixels, &cpu.pixels);
    println!("gpu_motion_frame off_diff_bytes={off_diff}");
    assert_eq!(
        off_diff, 0,
        "关档时暂存不被取走：帧必须与 CPU 参考帧逐字节相同（实测 {off_diff} 字节不同）"
    );
    assert!(
        !fixture.renderer.staged_motion.is_empty(),
        "关档时 stage 只是登记，不许取走"
    );
}

/// P3-R252：整批都在距离剔除球外 ⇒ 这条 draw 干脆不登记（像素与"烘焙后零矩阵"一致）。
///
/// 判据用保守包围盒：远批在球外必须跳、近批一只都不许跳。两层都带非空洞对照：
/// 远批在剔除档关时必须真的出现在画面里，否则"跳过前后同像素"可能只是它本来就没画出来。
#[test]
fn gpu_motion_batch_outside_cull_sphere_skips_the_draw() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-batch-cull") else {
        return gpu_skipped("request_adapter/compute");
    };
    // 远批：路径在 z = -30（相机在 z=+4，距离 34 m > 20 m），仍在视锥内（远平面 100）。
    let far = motion_batch_at(&SLOTS, -30.0);
    fixture.renderer.set_gpu_motion_batches(true);

    // 非空洞对照：剔除档关时这批必须真的画出来。
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let far_visible = fixture.render(vec![batch_drawable(&[])]);
    let empty = fixture.render(Vec::new());
    let drawn = byte_diff(&far_visible.pixels, &empty.pixels);
    println!(
        "batch_cull far_visible draw_calls={} drawn_bytes={drawn}",
        far_visible.draw_calls
    );
    assert!(
        drawn > 0,
        "远批在剔除档关时必须真的在画面里（否则跳过后的同像素是空洞通过）"
    );

    // 开剔除档：整批在球外 ⇒ 不登记 draw。
    fixture.renderer.set_gpu_motion_cull_distance(20.0);
    fixture.renderer.stage_motion_batch(1, far, FROZEN);
    let skipped = fixture.render(vec![batch_drawable(&[])]);
    let culled = fixture.renderer.frame_gpu_motion_culled();
    let culled_instances = fixture.renderer.frame_gpu_motion_culled_instances();
    let skipped_diff = byte_diff(&skipped.pixels, &empty.pixels);
    println!(
        "batch_cull skipped culled={culled} culled_instances={culled_instances} draw_calls={} empty_draw_calls={} diff_bytes={skipped_diff}",
        skipped.draw_calls, empty.draw_calls
    );
    assert_eq!(culled, 1, "整批在球外必须跳过这条 draw（实测 {culled}）");
    assert_eq!(
        culled_instances,
        SLOTS.len() as u64,
        "跳过的实例数必须等于批次大小（实测 {culled_instances}）"
    );
    assert_eq!(
        skipped.draw_calls, empty.draw_calls,
        "跳过后的 draw 数应与空场景一致"
    );
    assert_eq!(
        skipped_diff, 0,
        "跳过的帧必须与空场景逐字节相同（画零矩阵也是这个像素）"
    );

    // 近批对照：距离 < 20 m ⇒ 一只都不许被整批跳过，且与剔除档关的帧同画。
    let near = motion_batch(&SLOTS);
    fixture.renderer.stage_motion_batch(1, near.clone(), FROZEN);
    let near_culled = fixture.render(vec![batch_drawable(&[])]);
    let near_culled_count = fixture.renderer.frame_gpu_motion_culled();
    let near_culled_instances = fixture.renderer.frame_gpu_motion_culled_instances();
    fixture.renderer.set_gpu_motion_cull_distance(0.0);
    fixture.renderer.stage_motion_batch(1, near, FROZEN);
    let near_off = fixture.render(vec![batch_drawable(&[])]);
    let near_diff = byte_diff(&near_culled.pixels, &near_off.pixels);
    let near_delta = max_channel_delta(&near_culled.pixels, &near_off.pixels);
    println!(
        "batch_cull near_culled={near_culled_count} near_culled_instances={near_culled_instances} diff_bytes={near_diff} max_delta={near_delta}"
    );
    assert_eq!(
        near_culled_count, 0,
        "近批不许被整批跳过（实测 {near_culled_count}）"
    );
    assert_eq!(
        near_culled_instances, 0,
        "近批不存在被跳过的实例（实测 {near_culled_instances}）"
    );
    assert!(
        near_diff * 100 <= near_off.pixels.len(),
        "近批开/关剔除档的像素差应只落在末位（实测 {near_diff}/{} 字节）",
        near_off.pixels.len()
    );
    assert!(
        near_delta <= 16,
        "近批单通道最大差 {near_delta} 超出末位差范围"
    );
}

/// P3-R255：帧路径的逐实例视锥剔除——画内的实例一个都不许被误剔，画外的两只不改变像素。
///
/// 视锥由 `render_passes` 从 `main_matrix` 打包：相机在 z=+4、90° 视角 ⇒ z=0 平面上半宽
/// 4 个单位。四只里 x=±6 的两只整颗球在视锥外（本来就出 0 像素），x=±0.9 的两只在画内。
/// 剔除档开/关两帧必须**逐字节相同**：画外两只剔没剔不影响像素，画内两只被剔就会立刻变。
///
/// 第二屏是这一门真正能变红的地方：一只中心在画外的实例（x=5.5 > 半宽 4），但网格局部
/// 半径 5.66 × 缩放 0.6 ≈ 3.4，球伸到 x≈2.1 进画面（四边形左缘 x=3.1 会露出约 7 像素宽）。
/// 半径口径写错（只看球心 / 半径没接线 / 漏乘 model_scale）就会把它剔掉，两帧像素差非零。
#[test]
fn gpu_motion_frustum_cull_keeps_visible_instances() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-frustum-cull") else {
        return gpu_skipped("request_adapter/compute");
    };
    fixture.renderer.set_gpu_motion_batches(true);
    let empty = fixture.render(Vec::new());

    // 一屏：两只画外 + 两只画内。
    let batch = motion_batch(&[-6.0, -0.9, 0.9, 6.0]);
    fixture
        .renderer
        .stage_motion_batch(1, batch.clone(), FROZEN);
    let off = fixture.render(vec![batch_drawable(&[])]);
    if !off.hdr {
        return gpu_skipped("hdr_active");
    }
    let drawn = byte_diff(&off.pixels, &empty.pixels);
    assert!(drawn > 0, "关档参考帧必须真的出了图");

    fixture.renderer.set_gpu_motion_frustum_cull(true);
    fixture.renderer.stage_motion_batch(1, batch, FROZEN);
    let on = fixture.render(vec![batch_drawable(&[])]);
    let diff = byte_diff(&on.pixels, &off.pixels);
    println!(
        "frustum_cull mixed draw_calls={}/{} triangles={}/{} diff_bytes={diff} drawn={drawn}",
        on.draw_calls, off.draw_calls, on.triangles, off.triangles
    );
    assert_eq!(
        on.draw_calls, off.draw_calls,
        "剔除档不许改 draw 数（批仍是一条 draw）"
    );
    assert_eq!(
        on.triangles, off.triangles,
        "剔除档不许改三角形数（indirect 只缩实例数）"
    );
    assert_eq!(
        diff, 0,
        "视锥剔除不许改动画内实例的像素（实测 {diff} 字节不同）"
    );

    // 二屏：中心在画外、保守球伸进画面的实例必须保留。
    let straddling = motion_batch_scaled(&[5.5], 0.0, 0.6);
    fixture.renderer.set_gpu_motion_frustum_cull(false);
    fixture
        .renderer
        .stage_motion_batch(1, straddling.clone(), FROZEN);
    let edge_off = fixture.render(vec![batch_drawable(&[])]);
    let edge_drawn = byte_diff(&edge_off.pixels, &empty.pixels);
    assert!(
        edge_drawn > 0,
        "半露实例在关档时必须真的露出像素（否则这一屏没有判别力）"
    );
    fixture.renderer.set_gpu_motion_frustum_cull(true);
    fixture.renderer.stage_motion_batch(1, straddling, FROZEN);
    let edge_on = fixture.render(vec![batch_drawable(&[])]);
    let edge_diff = byte_diff(&edge_on.pixels, &edge_off.pixels);
    println!("frustum_cull straddling diff_bytes={edge_diff} drawn={edge_drawn}");
    assert_eq!(
        edge_diff, 0,
        "保守半径伸进画面的实例被剔掉（实测 {edge_diff} 字节不同，露出 {edge_drawn} 字节）"
    );
}

/// P3-R266：远程密度档——远批只留批次内 `index % k == 0` 的那些，近批一只不动。
///
/// 判据落在三件上：
/// * 远批开档后的帧与"只 stage 保留下来的那两只"的帧**逐字节相同**（保留集合精确：
///   多留一只或少留一只都会变）；
/// * 关档四只 vs 开档两只必须有真实像素差（uniform 真进了 shader，不是只写状态）；
/// * 近批（距离 < 档位半径）开/关两帧**逐字节相同**；步长 1 归一为关档。
///
/// 口径：**空场景不能当像素基线**——没有 drawable 时整帧走另一条渲染路径，一只
/// 4.8 px 的实例也会让 12288/16384 字节不同（实测）。所以基线一律取自同一次 3D 通过。
///
/// 抓的假绿：只写 uniform 不进 shader（`thin_vs_expected` 会等于 `expected_vs_off` 而非 0）、
/// 把近区也一起稀掉（近批帧变）、远批根本没画出来（`expected_vs_off` 为 0）。
/// 用位置哈希而不是静态下标这一条不靠像素证：下标是静态的 ⇒ 同一只每帧留/走一致，不闪。
#[test]
fn gpu_motion_thin_keeps_one_in_stride_only_beyond_the_radius() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-thin") else {
        return gpu_skipped("request_adapter/compute");
    };
    fixture.renderer.set_gpu_motion_batches(true);
    let empty = fixture.render(Vec::new());

    // 远批：路径在 z = -4（相机 z=+4 ⇒ 距离 8 m），四只在画内、互不相邻。
    let far = motion_batch_at(&SLOTS, -4.0);
    fixture.renderer.set_gpu_motion_thin(None);
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let off = fixture.render(vec![batch_drawable(&[])]);
    if !off.hdr {
        return gpu_skipped("hdr_active");
    }
    let far_drawn = byte_diff(&off.pixels, &empty.pixels);

    // 开档：半径 5 m（< 8 m）⇒ 远批四只只留下标 0 / 2。
    fixture.renderer.set_gpu_motion_thin(Some((5.0, 2)));
    fixture.renderer.stage_motion_batch(1, far, FROZEN);
    let thinned = fixture.render(vec![batch_drawable(&[])]);
    let thin_vs_off = byte_diff(&thinned.pixels, &off.pixels);

    // 同一次 3D 通过里的参照：只 stage 下标 0 / 2。
    fixture.renderer.set_gpu_motion_thin(None);
    fixture
        .renderer
        .stage_motion_batch(1, motion_batch_at(&[SLOTS[0], SLOTS[2]], -4.0), FROZEN);
    let expected = fixture.render(vec![batch_drawable(&[])]);
    let thin_vs_expected = byte_diff(&thinned.pixels, &expected.pixels);
    let expected_vs_off = byte_diff(&expected.pixels, &off.pixels);
    println!(
        "thin thin_vs_off={thin_vs_off} thin_vs_expected={thin_vs_expected} \
         expected_vs_off={expected_vs_off} of {} far_drawn={far_drawn} \
         draw_calls={}/{} triangles={}/{}",
        off.pixels.len(),
        thinned.draw_calls,
        off.draw_calls,
        thinned.triangles,
        off.triangles
    );
    assert_eq!(thinned.draw_calls, off.draw_calls, "密度档不许改 draw 数");
    assert_eq!(
        thinned.triangles, off.triangles,
        "密度档不许改三角形数（indirect 只缩实例数）"
    );
    assert!(
        expected_vs_off > 0,
        "少 stage 两只必须改变像素（否则逐字节相同是空洞的）"
    );
    assert_eq!(
        thin_vs_expected, 0,
        "远区只留下标 0/2 ⇒ 必须与只 stage 这两只逐字节相同（实测 {thin_vs_expected} 字节不同）"
    );
    assert!(
        thin_vs_off > 0,
        "关档四只 vs 开档两只必须有像素差（uniform 没进 shader？实测 {thin_vs_off} 字节不同）"
    );

    // 近批对照：距离 4 m < 半径 5 m ⇒ 开档帧必须与关档逐字节相同。
    let near = motion_batch(&SLOTS);
    fixture.renderer.set_gpu_motion_thin(None);
    fixture.renderer.stage_motion_batch(1, near.clone(), FROZEN);
    let near_off = fixture.render(vec![batch_drawable(&[])]);
    // 非空洞：近批像素必须对实例数敏感，否则"逐字节相同"也可能只是实例根本没画。
    fixture
        .renderer
        .stage_motion_batch(1, motion_batch(&[SLOTS[0], SLOTS[2]]), FROZEN);
    let near_reduced = fixture.render(vec![batch_drawable(&[])]);
    let near_sensitive = byte_diff(&near_reduced.pixels, &near_off.pixels);
    assert!(
        near_sensitive > 0,
        "近批像素必须对实例数敏感（否则下面逐字节相同是空洞的）"
    );
    fixture.renderer.set_gpu_motion_thin(Some((5.0, 2)));
    fixture.renderer.stage_motion_batch(1, near.clone(), FROZEN);
    let near_thin = fixture.render(vec![batch_drawable(&[])]);
    let near_diff = byte_diff(&near_thin.pixels, &near_off.pixels);
    println!("thin near sensitive={near_sensitive} diff_bytes={near_diff}");
    assert_eq!(
        near_diff, 0,
        "半径内的实例一只都不许动（实测 {near_diff} 字节不同）"
    );

    // 步长 1 = 关：归一化后与关档逐字节相同（不是"稀成 1/1 但走了另一条路"）。
    fixture.renderer.set_gpu_motion_thin(Some((5.0, 1)));
    fixture.renderer.stage_motion_batch(1, near, FROZEN);
    let stride_one = fixture.render(vec![batch_drawable(&[])]);
    let stride_one_diff = byte_diff(&stride_one.pixels, &near_off.pixels);
    println!("thin stride_one diff_bytes={stride_one_diff}");
    assert_eq!(
        stride_one_diff, 0,
        "步长 1 必须归一为关档（实测 {stride_one_diff} 字节不同）"
    );
    assert_eq!(
        fixture.renderer.gpu_motion_thin(),
        None,
        "步长 1 不许留在渲染器状态里"
    );
}

/// P3-R267：第二远带与第一带取**交集**——第一带已留下的下标集合上再按 `index % k == 0` 抽。
///
/// 判别口径：远批下标 0..3，近带 5 m/k=2 留 `{0,2}`，第二带 7 m/k=3 留 `{0,3}`，
/// 交集 = **只留下标 0**。若第二带是覆盖语义会留 `{0,3}`，若按压缩后的位置重排会被
/// 别的集合命中；两种情况都与"只 stage 下标 0"的帧不同。基线一律取自同一次 3D 通过。
///
/// 反面：第二带半径不比第一带远时归一为关（帧回到只开第一带）；只开第二带（第一带关）
/// 不得生效。
#[test]
fn gpu_motion_thin_far_selects_intersection_of_bands() {
    let Some(mut fixture) = fixture("rustlua-test-batch-motion-thin-far") else {
        return gpu_skipped("request_adapter/compute");
    };
    fixture.renderer.set_gpu_motion_batches(true);
    let empty = fixture.render(Vec::new());

    let far = motion_batch_at(&SLOTS, -4.0);
    fixture.renderer.set_gpu_motion_thin(None);
    fixture.renderer.set_gpu_motion_thin_far(None);
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let off = fixture.render(vec![batch_drawable(&[])]);
    if !off.hdr {
        return gpu_skipped("hdr_active");
    }
    let far_drawn = byte_diff(&off.pixels, &empty.pixels);
    assert!(
        far_drawn > 0,
        "远批四只在关档时必须真的露出像素（否则后面的逐字节相等全是空洞）"
    );

    // 只开第一带：5 m / k=2 ⇒ {0,2}。
    fixture.renderer.set_gpu_motion_thin(Some((5.0, 2)));
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let near_band = fixture.render(vec![batch_drawable(&[])]);
    let near_band_vs_off = byte_diff(&near_band.pixels, &off.pixels);

    // 叠第二带：7 m / k=3 ⇒ 交集 {0}。
    fixture.renderer.set_gpu_motion_thin_far(Some((7.0, 3)));
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let both = fixture.render(vec![batch_drawable(&[])]);
    let both_vs_near = byte_diff(&both.pixels, &near_band.pixels);

    // 参照：两带都关、只 stage 下标 0。
    fixture.renderer.set_gpu_motion_thin(None);
    fixture.renderer.set_gpu_motion_thin_far(None);
    fixture
        .renderer
        .stage_motion_batch(1, motion_batch_at(&[SLOTS[0]], -4.0), FROZEN);
    let expected = fixture.render(vec![batch_drawable(&[])]);
    let both_vs_expected = byte_diff(&both.pixels, &expected.pixels);
    println!(
        "thin_far near_band_vs_off={near_band_vs_off} both_vs_near={both_vs_near} \
         both_vs_expected={both_vs_expected} of {} far_drawn={far_drawn} \
         draw_calls={}/{} triangles={}/{}",
        off.pixels.len(),
        both.draw_calls,
        off.draw_calls,
        both.triangles,
        off.triangles
    );
    assert!(
        near_band_vs_off > 0,
        "只开第一带必须与关档有像素差（否则交集参照不成立，实测 {near_band_vs_off} 字节）"
    );
    assert!(
        both_vs_near > 0,
        "第二带必须真的再稀一层（{both_vs_near} 字节不同 ⇒ 没生效？）"
    );
    assert_eq!(
        both_vs_expected, 0,
        "两带交集 = 只留下标 0 ⇒ 必须与只 stage 这一只逐字节相同（实测 {both_vs_expected} 字节不同）"
    );
    assert_eq!(both.draw_calls, off.draw_calls, "第二带不许改 draw 数");
    assert_eq!(
        both.triangles, off.triangles,
        "第二带不许改三角形数（indirect 只缩实例数）"
    );

    // 反面 1：第二带半径不比第一带远（3 m < 5 m）⇒ arena 归一为关，帧回到只开第一带。
    fixture.renderer.set_gpu_motion_thin(Some((5.0, 2)));
    fixture.renderer.set_gpu_motion_thin_far(Some((3.0, 2)));
    fixture.renderer.stage_motion_batch(1, far.clone(), FROZEN);
    let reversed = fixture.render(vec![batch_drawable(&[])]);
    let reversed_diff = byte_diff(&reversed.pixels, &near_band.pixels);
    println!("thin_far reversed_diff={reversed_diff}");
    assert_eq!(
        reversed_diff, 0,
        "第二带更近必须整体不生效（实测 {reversed_diff} 字节不同）"
    );

    // 反面 2：只开第二带（第一带关）不得生效。
    fixture.renderer.set_gpu_motion_thin(None);
    fixture.renderer.set_gpu_motion_thin_far(Some((7.0, 3)));
    fixture.renderer.stage_motion_batch(1, far, FROZEN);
    let orphan = fixture.render(vec![batch_drawable(&[])]);
    let orphan_diff = byte_diff(&orphan.pixels, &off.pixels);
    println!("thin_far orphan_diff={orphan_diff}");
    assert_eq!(
        orphan_diff, 0,
        "第一带关时第二带不得单独生效（实测 {orphan_diff} 字节不同）"
    );

    // 状态归一：步长 1 不许留在渲染器状态里。
    fixture.renderer.set_gpu_motion_thin_far(Some((7.0, 1)));
    assert_eq!(fixture.renderer.gpu_motion_thin_far(), None);
}

/// P3-R284：帧路径的 compute 绑定组按批次身份复用（R270 只持久化了缓冲，绑定组仍每帧新建）。
///
/// 钉三件事：
/// * **稳态零新建**：同一 `Arc` 身份 + 同一 arena，连画 4 帧只建 1 次绑定组，像素逐字节相同。
/// * **arena 换代必须重绑**：绑定 4/5 绑的是**整块**实例 arena（矩阵格偏移走 params）。别的
///   内容把 arena 撑爆换缓冲后，原批次下一帧必须换新绑定组——否则烘焙写进旧缓冲、draw 读新
///   缓冲（零初始化）⇒ 整批退化成零矩阵。判据是"换代后只画原批次"的帧与首帧逐字节相同。
/// * **回滚档同画**：`set_bake_cache_enabled(false)` 走每次新建缓冲 + 不缓存绑定组的旧入口，
///   像素必须与缓存档相同（缓存不是"换了张图"）。
#[test]
fn gpu_motion_frame_reuses_compute_bind_group_and_rebinds_after_arena_growth() {
    let Some(mut fixture) = fixture("rustlua-test-bake-bind-cache") else {
        return gpu_skipped("request_adapter/compute");
    };
    let batch = std::sync::Arc::new(motion_batch(&SLOTS));
    fixture.renderer.set_gpu_motion_batches(true);
    // 帧路径只有在剔除/密度/LOD 至少开一档时才走**缓存压缩**入口（五档全 None 会落到非缓存
    // 旧路径，那条每帧新建缓冲、本来就不缓存绑定组）。半径给足，4 只全留。
    fixture.renderer.set_gpu_motion_cull_distance(1000.0);
    fixture
        .renderer
        .stage_motion_batch(1, batch.clone(), FROZEN);
    let first = fixture.render(vec![batch_drawable(&[])]);
    if !first.hdr {
        return gpu_skipped("hdr_active");
    }
    let creates_first = fixture.renderer.compute_stats().bind_group_creates;
    let serials_first = fixture.renderer.bake_bind_serials;
    let epoch_first = fixture.renderer.instance_arena_epoch;
    assert!(creates_first >= 1, "首帧必须真的建过 compute 绑定组");

    // ① 稳态：同一身份连画 3 帧，零新建、零新身份、像素逐字节不变。
    for round in 0..3 {
        fixture
            .renderer
            .stage_motion_batch(1, batch.clone(), FROZEN);
        let frame = fixture.render(vec![batch_drawable(&[])]);
        assert_eq!(
            byte_diff(&frame.pixels, &first.pixels),
            0,
            "稳态第 {round} 帧必须与首帧逐字节相同"
        );
    }
    assert_eq!(
        fixture.renderer.compute_stats().bind_group_creates,
        creates_first,
        "稳态帧不许再建 compute 绑定组（R284 前是每帧每批一次）"
    );
    assert_eq!(
        fixture.renderer.bake_bind_serials, serials_first,
        "稳态帧不许再发新的绑定组身份"
    );
    assert!(
        fixture.renderer.compute_stats().bind_group_cache_hits >= 3,
        "稳态 3 帧必须真的命中缓存，而不是根本没走烘焙"
    );

    // ② 回滚档同画：关掉缓存走旧入口，像素必须一致。
    fixture.renderer.set_bake_cache_enabled(false);
    fixture
        .renderer
        .stage_motion_batch(1, batch.clone(), FROZEN);
    let uncached = fixture.render(vec![batch_drawable(&[])]);
    assert_eq!(
        byte_diff(&uncached.pixels, &first.pixels),
        0,
        "回滚档必须与缓存档同画"
    );
    fixture.renderer.set_bake_cache_enabled(true);

    // ③ 换代：只画另一批 3000 只（3000×64B 起 > 128 KiB 初始容量）把 arena 撑爆。
    //    这一帧故意不带原批次——否则深度排序可能让原批次先预留，换代就测不到它头上。
    let big_slots: Vec<f32> = (0..3000).map(|i| -8.0 + (i as f32) * 0.005).collect();
    let big = std::sync::Arc::new(motion_batch(&big_slots));
    let mut big_draw = batch_drawable(&[]);
    big_draw.entity = rustlua_scene::scene::SceneEntityId(2);
    big_draw.name = "CrowdBig".to_owned();
    fixture.renderer.stage_motion_batch(2, big, FROZEN);
    let grown = fixture.render(vec![big_draw]);
    assert!(grown.draw_calls >= 1, "大批必须真的出了 draw");
    assert!(
        fixture.renderer.instance_arena_epoch > epoch_first,
        "夹具必须真的把 arena 撑爆换代（epoch {} → {}），否则这一条是空洞的",
        epoch_first,
        fixture.renderer.instance_arena_epoch
    );
    let serials_grown = fixture.renderer.bake_bind_serials;

    // ④ 换代后只画原批次：必须与首帧逐字节相同，且必须发过新身份。
    fixture.renderer.stage_motion_batch(1, batch, FROZEN);
    let after = fixture.render(vec![batch_drawable(&[])]);
    let after_diff = byte_diff(&after.pixels, &first.pixels);
    println!(
        "bake_bind_cache creates_first={creates_first} steady_creates={} serials={serials_first}→{serials_grown}→{} epoch={epoch_first}→{} after_growth_diff={after_diff}",
        fixture.renderer.compute_stats().bind_group_creates,
        fixture.renderer.bake_bind_serials,
        fixture.renderer.instance_arena_epoch,
    );
    assert_eq!(
        after_diff, 0,
        "arena 换代后必须重绑（实测 {after_diff} 字节不同）"
    );
    assert!(
        fixture.renderer.bake_bind_serials > serials_grown,
        "arena 换代后原批次必须拿到新的绑定组身份"
    );
}

/// P3-R287：种子缓存必须与「每条 draw 重算」逐位相同——含零缩放、空实例表、静态批次
/// （走 `positions()` 而不是路径节点）、非法缩放与无穷半径这些退化形状。
/// 比的是 `to_bits()`：NaN 也要同位，`assert_eq!(f32)` 会把 NaN 判成不等而漏掉形状。
#[test]
fn conservative_bounds_seed_is_bit_identical_to_per_draw_recompute() {
    use crate::renderer::world_sprite::{
        batch_bounds_seed, conservative_batch_bounds, expanded_batch_bounds,
    };

    let mut static_batch = motion_batch(&SLOTS);
    static_batch.motion = None;
    let mut nan_scale = motion_batch(&SLOTS);
    nan_scale.instances[0].scale = [f32::NAN, 1.0, 1.0];
    let batches = [
        motion_batch_scaled(&SLOTS, 0.0, ANT_SCALE),
        motion_batch_scaled(&SLOTS, 3.0, 0.0),
        motion_batch_scaled(&[], 0.0, ANT_SCALE),
        static_batch,
        nan_scale,
    ];
    let mut cases = 0;
    for batch in &batches {
        let seed = batch_bounds_seed(batch);
        for radius in [0.0_f32, 0.5, 2.0, f32::INFINITY] {
            for scale in [0.0_f32, 1.0, 3.5] {
                let direct = conservative_batch_bounds(batch, radius, scale);
                let cached = expanded_batch_bounds(seed, radius, scale);
                for (axis, (a, b)) in [
                    (direct.min.x, cached.min.x),
                    (direct.min.y, cached.min.y),
                    (direct.min.z, cached.min.z),
                    (direct.max.x, cached.max.x),
                    (direct.max.y, cached.max.y),
                    (direct.max.z, cached.max.z),
                ]
                .into_iter()
                .enumerate()
                {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "轴 {axis} 半径 {radius} 缩放 {scale}：缓存种子与逐 draw 重算不同位"
                    );
                }
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 60, "5 个批次 × 4 半径 × 3 缩放 = 60 组对账");
}
