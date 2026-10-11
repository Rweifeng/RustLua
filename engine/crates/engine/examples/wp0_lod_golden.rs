//! RustLua-① 证据：P1-② LOD（距离驱动网格选择）的行为断言。
//!
//! LOD 的语义是：同一实体挂 `LodGroup` 后，3D 提取按**相机到模型的距离**切换网格。
//! 证据分三层（前两层是提取层语义，第三层是渲染层落点）：
//! * 基线：未注入 LOD 时 SkeletonWarrior 用 `MeshRenderer.mesh`（kaykit-warrior）；
//! * 近相机：注入 `LodGroup` 后，距离 < 阈值 → 仍是基础级网格（warrior）；
//! * 远相机：距离 > 阈值 → 切到低细节网格（m8-test-triangle）；
//! * 渲染层：同一 `Scene3d` 快照渲染成功，近/远两帧像素明显不同（网格真的换了）。

use std::{env, fs, path::PathBuf};

use engine::host::{RenderOutcome, Renderer, RuntimeScene};
use rustlua_camera::camera::{Camera3D, Viewport};
use rustlua_infra::games_dir;
use rustlua_render::lod::LodGroup;
use rustlua_render::mesh3d::MeshRenderer;
use rustlua_scene::scene::Scene;
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const HI_MESH: &str = "assets/models/kaykit-warrior.glb";
const LO_MESH: &str = "assets/models/m8-test-triangle.glb";
const LOD_THRESHOLD: f32 = 8.0;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let scene_path = root.join("scenes/demo3d.scene");
    let scene = Scene::load(&scene_path).expect("load demo3d scene");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(&root), scene).expect("instantiate demo3d");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");

    // ① 基线：注入 LOD 之前，SkeletonWarrior 的网格是 MeshRenderer 里写的 warrior。
    let baseline = runtime.scene_3d(viewport).expect("3D scene baseline");
    let warrior_baseline = baseline
        .drawables
        .iter()
        .find(|d| d.name == "SkeletonWarrior")
        .expect("SkeletonWarrior drawable");
    assert_eq!(
        warrior_baseline.mesh_path, HI_MESH,
        "基线网格应为 warrior（LOD 未注入时用 renderer.mesh）"
    );

    // ② 注入 LodGroup：基础级 warrior，距离 > 8 时切 triangle。
    runtime.with_script(|script| {
        script.with_world_mut(|world| {
            let mut target = None;
            world.for_each::<MeshRenderer>(|id, renderer| {
                if renderer.mesh == HI_MESH && target.is_none() {
                    target = Some(id);
                }
            });
            let target = target.expect("entity with warrior mesh");
            world
                .insert(
                    target,
                    LodGroup::from_component_str(&format!(
                        "mesh={HI_MESH};lod={LOD_THRESHOLD}|{LO_MESH}"
                    ))
                    .expect("lod group"),
                )
                .expect("insert lod group");
        });
    });

    // 近相机：位置 (1.5, 0, 3)，看向 warrior 所在 (1.5, 0, 0)，距离 ≈ 3 < 8 → 基础级 warrior。
    let near_camera = Camera3D {
        position: rustlua_math::math3d::Vec3::new(1.5, 0.0, 3.0),
        target: rustlua_math::math3d::Vec3::new(1.5, 0.0, 0.0),
        up: rustlua_math::math3d::Vec3::Y,
        fov_y: std::f32::consts::FRAC_PI_2,
        near: 0.1,
        far: 100.0,
        pose_from_transform: false,
        enabled: true,
    };
    let near = runtime
        .scene_3d_with_camera(viewport, Some(near_camera))
        .expect("near camera 3D scene");
    let warrior_near = near
        .drawables
        .iter()
        .find(|d| d.name == "SkeletonWarrior")
        .expect("SkeletonWarrior near");
    assert_eq!(
        warrior_near.mesh_path, HI_MESH,
        "近相机（距离 < {LOD_THRESHOLD}）应保持基础级网格 warrior，实际 {}",
        warrior_near.mesh_path
    );

    // 远相机：位置 (1.5, 0, 30)，距离 ≈ 28.5 > 8 → 切到低细节 triangle。
    let far_camera = Camera3D {
        position: rustlua_math::math3d::Vec3::new(1.5, 0.0, 20.0),
        target: rustlua_math::math3d::Vec3::new(1.5, 0.0, 0.0),
        up: rustlua_math::math3d::Vec3::Y,
        fov_y: std::f32::consts::FRAC_PI_2,
        near: 0.1,
        far: 100.0,
        pose_from_transform: false,
        enabled: true,
    };
    let far = runtime
        .scene_3d_with_camera(viewport, Some(far_camera))
        .expect("far camera 3D scene");
    let warrior_far = far
        .drawables
        .iter()
        .find(|d| d.name == "SkeletonWarrior")
        .expect("SkeletonWarrior far");
    assert_eq!(
        warrior_far.mesh_path, LO_MESH,
        "远相机（距离 > {LOD_THRESHOLD}）应切到低细节 triangle，实际 {}",
        warrior_far.mesh_path
    );

    // ③ 渲染层：近/远快照都渲染成功，两帧像素必须明显不同（网格真的换了）。
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the LOD golden");
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-lod-golden"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request golden device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp0-lod-golden-target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
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

    let mesh_near = runtime.vertices(WIDTH, HEIGHT).expect("2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh_near, Some(&near), &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_near = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (near)");
    let metrics_near = renderer.last_frame_metrics();

    let mesh_far = runtime.vertices(WIDTH, HEIGHT).expect("2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh_far, Some(&far), &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_far = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (far)");
    let metrics_far = renderer.last_frame_metrics();

    let diff = count_diff(&pixels_near, &pixels_far);
    assert!(
        diff >= 5_000,
        "近/远两帧像素差异过小（{diff}）：LOD 切换没有真的改变画面（阈值切换失效）"
    );
    // 两帧都必须有内容（不是全空）。
    let near_content = count_non_background(&pixels_near, &pixels_near[0..3]);
    let far_content = count_non_background(&pixels_far, &pixels_far[0..3]);
    assert!(near_content >= 500, "近相机帧内容过少：{near_content}");
    assert!(
        far_content >= 20,
        "远相机帧内容过少：{far_content}（三角网格没画出来？）"
    );

    let report = json!({
        "report_version": "wp0-lod-golden-v1",
        "scene": "demo3d.scene + LodGroup(mesh=warrior;lod=8|triangle)",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "baseline_mesh": warrior_baseline.mesh_path,
            "near_camera_mesh": warrior_near.mesh_path,
            "far_camera_mesh": warrior_far.mesh_path,
            "lod_threshold": LOD_THRESHOLD,
            "near_distance": 3.0,
            "far_distance": 19.9,
            "near_keeps_hi": warrior_near.mesh_path == HI_MESH,
            "far_switches_lo": warrior_far.mesh_path == LO_MESH,
            "pixels_differ": diff,
            "near_content": near_content,
            "far_content": far_content,
            "near_draw_calls": metrics_near.draw_calls,
            "far_draw_calls": metrics_far.draw_calls,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize golden") + "\n";
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).expect("create golden output directory");
        }
        fs::write(&path, text).expect("write golden report");
        println!("report: {}", path.display());
    } else {
        print!("{text}");
    }
    println!(
        "wp0_lod_golden_ok near_keeps_hi=true far_switches_lo=true pixels_differ={diff} far_content={far_content}"
    );
}

fn parse_output() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--output" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// 两帧缓冲中逐通道差之和超过阈值的像素数。
fn count_diff(a: &[u8], b: &[u8]) -> u64 {
    assert_eq!(a.len(), b.len(), "readback buffer sizes differ");
    let mut count = 0_u64;
    for chunk in a.chunks(4).zip(b.chunks(4)) {
        let (p, q) = chunk;
        let difference = (i16::from(p[0]) - i16::from(q[0])).unsigned_abs()
            + (i16::from(p[1]) - i16::from(q[1])).unsigned_abs()
            + (i16::from(p[2]) - i16::from(q[2])).unsigned_abs();
        if difference > 24 {
            count += 1;
        }
    }
    count
}

/// 与背景（第一像素）通道差之和超过阈值的像素数。
fn count_non_background(pixels: &[u8], background: &[u8]) -> u64 {
    let mut count = 0_u64;
    for chunk in pixels.chunks(4) {
        let difference = (i16::from(chunk[0]) - i16::from(background[0])).unsigned_abs()
            + (i16::from(chunk[1]) - i16::from(background[1])).unsigned_abs()
            + (i16::from(chunk[2]) - i16::from(background[2])).unsigned_abs();
        if difference > 24 {
            count += 1;
        }
    }
    count
}
