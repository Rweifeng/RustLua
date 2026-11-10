//! P3-R183：剔除计数门——「剔掉多少实例」必须可量。
//!
//! 为什么不能只测「视锥外的东西没画出来」：那条既有行为（M8-08）没有读数，
//! 下一刀（遮挡剔除/植被 LOD）的收益就没有基准。这里钉住三件事：
//! ① 候选数 = 进入剔除判定的 drawable×primitive 数（与画没画无关）；
//! ② 视锥外的实例计入 `cull_frustum`，且每次调用重置（不跨帧累积）；
//! ③ `--no-cull` 对照档候选数不变、剔除数恒 0——两档 A/B 因此可比。
//!
//! 网格用地面哨兵（渲染器自合成，不依赖工程资源）；视锥外的两块放在 x=±500，
//! 相机 far=100，无论包围球怎么算都不可能在视锥内——判据不靠浮点边缘。

use super::*;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;

const SIZE: u32 = 32;

fn test_device(tag: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some(tag),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    Some((device, queue))
}

fn drawable(entity: u64, x: f32) -> rustlua_render::frame::Scene3dDrawable {
    drawable_at(entity, x, -1.0)
}

fn drawable_at(entity: u64, x: f32, z: f32) -> rustlua_render::frame::Scene3dDrawable {
    rustlua_render::frame::Scene3dDrawable {
        entity: rustlua_scene::scene::SceneEntityId(entity),
        name: format!("plane-{entity}"),
        mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(x, 0.0, z)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: rustlua_render::frame::Scene3dBounds {
            local_aabb: rustlua_math::math3d::Aabb3 {
                min: Vec3::new(-4.0, -4.0, -0.1),
                max: Vec3::new(4.0, 4.0, 0.1),
            },
            source: rustlua_render::frame::BoundsSource::Fallback,
        },
    }
}

fn scene_with(drawables: Vec<rustlua_render::frame::Scene3dDrawable>) -> Scene3d {
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables,
    }
}

#[test]
fn cull_counters_report_candidates_and_frustum_rejects() {
    let Some((device, queue)) = test_device("rustlua-cull-counters-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, true, true).expect("offscreen renderer");

    // 1 块在视锥内、2 块在 x=±500（far=100 之外）。
    let scene = scene_with(vec![
        drawable(1, 0.0),
        drawable(2, 500.0),
        drawable(3, -500.0),
    ]);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_candidates, 3, "候选数应含全部三块");
    assert_eq!(renderer.frame_cull_frustum, 2, "视锥外两块应被剔掉");
    assert_eq!(draws.len(), 1, "只有视锥内那块出 draw");

    // 每次调用重置：第二帧只剩视锥内一块，计数不许跨帧累积。
    let scene = scene_with(vec![drawable(1, 0.0)]);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_candidates, 1);
    assert_eq!(renderer.frame_cull_frustum, 0);
    assert_eq!(draws.len(), 1);

    // 空场景：两个计数都归零（不是上一帧的残值）。
    let draws = renderer.frame_3d_draws(&scene_with(Vec::new()));
    assert_eq!(renderer.frame_cull_candidates, 0);
    assert_eq!(renderer.frame_cull_frustum, 0);
    assert!(draws.is_empty());
}

#[test]
fn no_cull_keeps_candidates_but_reports_zero_rejects() {
    let Some((device, queue)) = test_device("rustlua-no-cull-counters-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, true).expect("offscreen renderer");
    let scene = scene_with(vec![
        drawable(1, 0.0),
        drawable(2, 500.0),
        drawable(3, -500.0),
    ]);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(
        renderer.frame_cull_candidates, 3,
        "对照档候选数必须与剔除档相同，否则 A/B 分母不一致"
    );
    assert_eq!(renderer.frame_cull_frustum, 0, "剔除关闭时不许报剔除数");
    let instances: usize = draws.iter().map(|draw| draw.models.len()).sum();
    assert_eq!(
        instances, 3,
        "对照档三块都画（实例化合批后 draw 数可能为 1）"
    );
}

/// P3-R184：`--skip-mesh` 归因开关——含子串的 drawable 在缓存查询与剔除之前整个跳过：
/// 不进候选分母、计入 `frame_skip_mesh`、逐帧重置、`None` 回滚档行为不变。
#[test]
fn skip_mesh_filter_skips_matching_drawables_before_culling() {
    let Some((device, queue)) = test_device("rustlua-skip-mesh-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, true, true).expect("offscreen renderer");
    let mut grass = drawable(2, 0.0);
    grass.mesh_path = "assets/models/grass.glb".to_owned();
    let scene = scene_with(vec![drawable(1, 0.0), grass]);

    // 未开开关：假路径不在缓存里、被缓存门跳过，但不进 skip 计数。
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_skip_mesh, 0, "未开开关不许报 skip 数");
    assert_eq!(renderer.frame_cull_candidates, 1);
    assert_eq!(draws.iter().map(|draw| draw.models.len()).sum::<usize>(), 1);

    renderer.set_skip_mesh_filter(Some("grass".to_owned()));
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_skip_mesh, 1, "路径含子串应整个跳过");
    assert_eq!(
        renderer.frame_cull_candidates, 1,
        "被跳过的不进候选分母（A/B 剔除率口径不被开关污染）"
    );
    assert_eq!(draws.iter().map(|draw| draw.models.len()).sum::<usize>(), 1);

    // 逐帧重置：空场景归零，不是上一帧残值。
    let draws = renderer.frame_3d_draws(&scene_with(Vec::new()));
    assert_eq!(renderer.frame_skip_mesh, 0);
    assert!(draws.is_empty());

    // 回滚档：关掉开关后同一路径不再计入 skip。
    renderer.set_skip_mesh_filter(None);
    let mut grass = drawable(3, 0.0);
    grass.mesh_path = "assets/models/grass.glb".to_owned();
    renderer.frame_3d_draws(&scene_with(vec![grass]));
    assert_eq!(renderer.frame_skip_mesh, 0);
}

/// P3-R206：最大绘制距离档——默认 0 = 关闭（候选/剔除/实例数都不变）；开了以后超出
/// 「距离 + 包围球半径」的实例并入 `cull_frustum`（与视锥共用分母），关掉即恢复。
#[test]
fn max_draw_distance_dial_rejects_far_draws() {
    let Some((device, queue)) = test_device("rustlua-draw-distance-device") else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, true, true).expect("offscreen renderer");

    // 相机在 z=+4 朝 -z：近块圆心距 5；远块 z=-60 圆心距 64，仍在 far=100 的视锥内。
    let scene = scene_with(vec![drawable(1, 0.0), drawable_at(2, 0.0, -60.0)]);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_candidates, 2);
    assert_eq!(renderer.frame_cull_frustum, 0, "默认档不许剔任何实例");
    assert_eq!(renderer.frame_cull_distance(), 0, "距离档关闭时白送一个 0");
    assert_eq!(draws.iter().map(|draw| draw.models.len()).sum::<usize>(), 2);

    renderer.set_max_draw_distance(20.0);
    let draws = renderer.frame_3d_draws(&scene);
    assert_eq!(
        renderer.frame_cull_candidates, 2,
        "距离档不改候选分母（两档 A/B 可比）"
    );
    assert_eq!(renderer.frame_cull_frustum, 1, "远块应被距离档剔掉");
    assert_eq!(
        renderer.frame_cull_distance(),
        1,
        "距离档的剔除量要能单独归因（同时含在 frustum 里）"
    );
    assert_eq!(
        draws.iter().map(|draw| draw.models.len()).sum::<usize>(),
        1,
        "只有近块出实例"
    );

    // 分类档：只掐匹配子串的网格（植被 LOD 的第一级），不匹配的整类原样保留。
    renderer.set_max_draw_distance_filter(Some("grass".to_owned()));
    renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_frustum, 0, "路径不匹配的类不参与距离档");
    renderer.set_max_draw_distance_filter(Some(GROUND_PLANE_SENTINEL.to_owned()));
    renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_frustum, 1, "匹配的类照常剔除远块");
    assert_eq!(renderer.frame_cull_distance(), 1);
    renderer.set_max_draw_distance_filter(None);

    // 回读面：宿主/工具报「生效档」不能靠猜（归一后的值才是真的）。
    assert_eq!(renderer.max_draw_distance(), 20.0);
    assert_eq!(renderer.max_draw_distance_filter(), None);
    renderer.set_max_draw_distance(f32::NAN);
    assert_eq!(renderer.max_draw_distance(), 0.0);
    renderer.set_max_draw_distance(20.0);
    renderer.set_max_draw_distance_filter(Some("grass".to_owned()));
    assert_eq!(renderer.max_draw_distance_filter(), Some("grass"));

    // 非法值归一为关闭：负/NaN 不许把全场景剔空。
    renderer.set_max_draw_distance(f32::NAN);
    renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_frustum, 0);
    assert_eq!(renderer.frame_cull_distance(), 0);
    renderer.set_max_draw_distance(-5.0);
    renderer.frame_3d_draws(&scene);
    assert_eq!(renderer.frame_cull_frustum, 0);
    assert_eq!(renderer.frame_cull_distance(), 0);
}
