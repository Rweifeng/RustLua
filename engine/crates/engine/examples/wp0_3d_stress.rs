//! WP0 3D evidence benchmark: fixed offscreen scene, mesh stairs, CPU allocation counters.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use engine::host::RuntimeScene;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_scene::scene::{Scene, SceneEntityId};

struct CountingAllocator;
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
static BASELINE_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            let live = LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed)
                + layout.size() as u64;
            let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
            while live > peak {
                match PEAK_BYTES.compare_exchange_weak(
                    peak,
                    live,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(current) => peak = current,
                }
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            let old = layout.size() as u64;
            let new = new_size as u64;
            let live = if new >= old {
                LIVE_BYTES.fetch_add(new - old, Ordering::Relaxed) + new - old
            } else {
                LIVE_BYTES.fetch_sub(old - new, Ordering::Relaxed) - (old - new)
            };
            let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
            while live > peak {
                match PEAK_BYTES.compare_exchange_weak(
                    peak,
                    live,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(current) => peak = current,
                }
            }
        }
        new_ptr
    }
}

#[derive(Clone, Copy)]
struct Stats {
    count: usize,
    min_us: u128,
    average_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    max_us: u128,
}

struct Config {
    output: Option<PathBuf>,
    cycles: usize,
    /// `--profile`：把每轮拆成 2D 顶点 / 3D 提取 / 渲染三段并写进 JSON。
    /// 为什么需要它：`cpu_frame_us` 只回答"整帧多慢"，不回答"慢在提取还是慢在渲染"；
    /// 没有这个拆分，"下一步做 BVH/遮挡剔除"就只能靠猜（docs/53 的口径是每帧分配，
    /// 不是每帧耗时归因）。默认关闭，保持既有报告格式逐字不变。
    profile: bool,
    /// `--cull-headroom`：只用引擎自己的视锥判据数一遍"有多少实例根本看不见"，
    /// 不跑渲染、不写报告。它回答的是"剔除这个方向值不值得动手"，见 `cull_headroom`。
    cull_headroom: bool,
    /// `--scatter`（M83-05）：把网格铺开到 1000×1000 量级的"散开布局"，相机贴近一角。
    /// 现有默认布局的 100×100 网格整体落在视锥里（`outside_pct=0.0`），在任何剔除结构上
    /// 读数都不会动——那等于没有基准。散开布局让 `--cull-headroom` 报出**非零**上限，
    /// 将来判"做不做 BVH/遮挡剔除"才有可比的读数。默认关闭：默认布局与报告格式逐字不变。
    scatter: bool,
}

fn main() {
    let config = parse_config();
    if config.cull_headroom {
        for mesh_count in [100usize, 1_000, 10_000] {
            println!("{}", cull_headroom(mesh_count, config.scatter));
        }
        return;
    }
    let root = games_dir().join("examples").join("game_api");
    let mut reports = Vec::new();
    for mesh_count in [100usize, 1_000, 10_000] {
        reports.push(run_stair(
            &root,
            mesh_count,
            config.cycles,
            config.profile,
            config.scatter,
        ));
    }
    let json = format_report(&reports, config.cycles);
    if let Some(path) = config.output {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(&path, json).expect("write 3D report");
        println!("report: {}", path.display());
    } else {
        print!("{json}");
    }
}

fn parse_config() -> Config {
    let mut output = None;
    let mut cycles = 30;
    let mut profile = false;
    let mut cull_headroom = false;
    let mut scatter = false;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ))
            }
            "--cycles" => {
                cycles = args
                    .next()
                    .expect("--cycles requires a value")
                    .parse()
                    .expect("--cycles must be an integer")
            }
            "--profile" => profile = true,
            "--cull-headroom" => cull_headroom = true,
            "--scatter" => scatter = true,
            "--help" | "-h" => {
                println!(
                    "Usage: wp0_3d_stress [--cycles N] [--output PATH] [--profile] [--cull-headroom] [--scatter]"
                );
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(cycles > 0, "--cycles must be greater than zero");
    Config {
        output,
        cycles,
        profile,
        cull_headroom,
        scatter,
    }
}

fn run_stair(
    root: &Path,
    mesh_count: usize,
    cycles: usize,
    profile: bool,
    scatter: bool,
) -> String {
    let mut runtime = RuntimeScene::from_scene_edit(Some(root), build_scene(mesh_count, scatter))
        .expect("runtime scene");
    let fixture = OffscreenTarget::acquire(
        &OffscreenRequest::new("wp0-3d-stress", 640, 360).with_timestamps(),
    )
    .expect("offscreen fixture for wp0-3d-stress");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    // M83-07 诊断开关（默认行为不变）：把 cull / instancing 单独关掉，看"固定每帧分配"
    // 是否随它们变化——总分配在 100/1000/10000 三档同样多出约 68 次/帧，先按子系统排除。
    let env_on = |key: &str| std::env::var(key).map(|v| v != "0").unwrap_or(true);
    let cull_enabled = env_on("WP0_CULL");
    let instancing_enabled = env_on("WP0_INSTANCING");
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue,
        Some(root.to_path_buf()),
        cull_enabled,
        instancing_enabled,
    )
    .expect("renderer");
    let view = fixture.view();
    let mut samples = Vec::with_capacity(cycles);
    let mut gpu_samples = Vec::with_capacity(cycles);
    let mut last = DefaultMetrics::default();
    let mut phases = PhaseTotals::default();
    for _ in 0..3 {
        let _ = render_once(
            &mut runtime,
            &mut renderer,
            view,
            &mut last,
            &mut phases,
            profile,
        );
    }
    phases = PhaseTotals::default();
    reset_allocations();
    for _ in 0..cycles {
        let started = Instant::now();
        let _ = render_once(
            &mut runtime,
            &mut renderer,
            view,
            &mut last,
            &mut phases,
            profile,
        );
        samples.push(started.elapsed());
        if let Some(gpu_us) = last.gpu_frame_us {
            gpu_samples.push(Duration::from_micros(gpu_us));
        }
    }
    // Renderer polling stays non-blocking. The benchmark may submit a few drain
    // frames so the last in-flight timestamp has a chance to become visible.
    if gpu_samples.is_empty()
        && let Some(gpu_us) = renderer.wait_for_gpu_timestamp(Duration::from_secs(2))
    {
        gpu_samples.push(Duration::from_micros(gpu_us));
    }
    let stats = summarize(samples);
    let baseline = BASELINE_LIVE_BYTES.load(Ordering::Relaxed);
    let live_delta = LIVE_BYTES.load(Ordering::Relaxed).saturating_sub(baseline);
    let peak_delta = PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(baseline);
    let gpu_stats = (!gpu_samples.is_empty()).then(|| summarize(gpu_samples));
    let gpu_json = gpu_stats.map_or_else(|| "null".to_owned(), stats_json);
    let gpu_reason = if gpu_stats.is_some() {
        "resolved"
    } else {
        renderer.gpu_timestamp_reason().unwrap_or("pending")
    };
    let profile_json = if profile {
        let n = cycles as u128;
        let per = |value: u64| value as u128 / n;
        format!(
            "{{\"vertices_us\":{},\"extract_us\":{},\"render_us\":{},\"render_share_pct\":{},\
             \"phase_allocs\":{{\
             \"vertices\":{{\"count\":{},\"bytes\":{}}},\
             \"extract\":{{\"count\":{},\"bytes\":{}}},\
             \"render\":{{\"count\":{},\"bytes\":{}}}}}}}",
            phases.vertices_us / n,
            phases.extract_us / n,
            phases.render_us / n,
            // 渲染段占三段之和的百分比：决定"下一步该优化提取还是优化渲染"的那把尺子。
            (100 * phases.render_us
                / (phases.vertices_us + phases.extract_us + phases.render_us).max(1)),
            per(phases.vertices_alloc_count),
            per(phases.vertices_alloc_bytes),
            per(phases.extract_alloc_count),
            per(phases.extract_alloc_bytes),
            per(phases.render_alloc_count),
            per(phases.render_alloc_bytes)
        )
    } else {
        "null".to_owned()
    };
    format!(
        "{{\"mesh_count\":{mesh_count},\"samples\":{},\"render\":{{\"cpu_frame_us\":{},\"gpu_frame_us\":{gpu_json},\"gpu_timestamp_supported\":{},\"gpu_timestamp_reason\":\"{gpu_reason}\",\"draw_calls\":{},\"instances\":{},\"triangles\":{}}},\"cpu_alloc_count\":{},\"cpu_alloc_bytes\":{},\"cpu_live_bytes\":{},\"cpu_peak_bytes\":{},\"known_renderer_bytes\":{{\"vertex_buffer_capacity\":{},\"vertex_bytes_peak\":{}}},\"profile\":{profile_json}}}",
        stats_json(stats),
        last.cpu_frame_us,
        renderer.gpu_timestamp_supported(),
        last.draw_calls,
        last.instances,
        last.triangles,
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        live_delta,
        peak_delta,
        renderer.vertex_buffer_capacity(),
        renderer.vertex_bytes_peak()
    )
}

#[derive(Default)]
struct DefaultMetrics {
    cpu_frame_us: u64,
    gpu_frame_us: Option<u64>,
    draw_calls: u64,
    instances: u64,
    triangles: u64,
}

/// `--profile` 的分段累计（微秒 + 分配）。三段的分界就是宿主每帧真正做的三件事：
/// 2D 顶点提取、3D 提取（`scene_3d` 把 ECS 摊平成 drawable）、以及一次 `render_into_texture`
/// （剔除 + 合批 + 写实例缓冲 + 录制 + 提交）。
///
/// 分配也按同一三段累计（M83-07）：总分配数在三档 mesh 数上**同样多出约 68 次/帧**，
/// 这只能是某段里的"固定每帧新建集合"，先按段切分才能把范围缩到函数级。
#[derive(Default, Clone, Copy)]
struct PhaseTotals {
    vertices_us: u128,
    extract_us: u128,
    render_us: u128,
    vertices_alloc_count: u64,
    vertices_alloc_bytes: u64,
    extract_alloc_count: u64,
    extract_alloc_bytes: u64,
    render_alloc_count: u64,
    render_alloc_bytes: u64,
}

/// 读一次全局分配计数（`(count, bytes)`）。
fn alloc_snapshot() -> (u64, u64) {
    (
        ALLOC_COUNT.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

/// 两快照之差。
fn alloc_delta(before: (u64, u64), after: (u64, u64)) -> (u64, u64) {
    (
        after.0.saturating_sub(before.0),
        after.1.saturating_sub(before.1),
    )
}

fn render_once(
    runtime: &mut RuntimeScene,
    renderer: &mut Renderer,
    view: &wgpu::TextureView,
    last: &mut DefaultMetrics,
    phases: &mut PhaseTotals,
    profile: bool,
) -> PhaseTotals {
    let frame_alloc_before = std::env::var_os("P2_FRAME").map(|_| alloc_snapshot());
    let started_vertices = profile.then(Instant::now);
    let alloc_before_vertices = profile.then(alloc_snapshot);
    let mesh = runtime.vertices(640, 360).expect("2D extraction");
    let alloc_vertices =
        profile.then(|| alloc_delta(alloc_before_vertices.expect("snapshot"), alloc_snapshot()));
    let vertices_us = started_vertices.map_or(0, |t| t.elapsed().as_micros());
    let started_extract = profile.then(Instant::now);
    let alloc_before_extract = profile.then(alloc_snapshot);
    let scene =
        runtime.scene_3d(rustlua_camera::camera::Viewport::new(640.0, 360.0).expect("viewport"));
    let alloc_extract =
        profile.then(|| alloc_delta(alloc_before_extract.expect("snapshot"), alloc_snapshot()));
    let extract_us = started_extract.map_or(0, |t| t.elapsed().as_micros());
    let started_render = profile.then(Instant::now);
    let alloc_before_render = profile.then(alloc_snapshot);
    assert!(matches!(
        renderer.render_into_texture(&mesh, scene.as_ref(), view, 640, 360, true),
        RenderOutcome::Presented
    ));
    let alloc_render =
        profile.then(|| alloc_delta(alloc_before_render.expect("snapshot"), alloc_snapshot()));
    let render_us = started_render.map_or(0, |t| t.elapsed().as_micros());
    let metrics = renderer.last_frame_metrics();
    last.cpu_frame_us = metrics.cpu_frame_us;
    last.gpu_frame_us = metrics.gpu_frame_us;
    last.draw_calls = metrics.draw_calls;
    last.instances = metrics.instances;
    last.triangles = metrics.triangles;
    let (vertices_alloc_count, vertices_alloc_bytes) = alloc_vertices.unwrap_or((0, 0));
    let (extract_alloc_count, extract_alloc_bytes) = alloc_extract.unwrap_or((0, 0));
    let (render_alloc_count, render_alloc_bytes) = alloc_render.unwrap_or((0, 0));
    let frame = PhaseTotals {
        vertices_us,
        extract_us,
        render_us,
        vertices_alloc_count,
        vertices_alloc_bytes,
        extract_alloc_count,
        extract_alloc_bytes,
        render_alloc_count,
        render_alloc_bytes,
    };
    phases.vertices_us += frame.vertices_us;
    phases.extract_us += frame.extract_us;
    phases.render_us += frame.render_us;
    phases.vertices_alloc_count += frame.vertices_alloc_count;
    phases.vertices_alloc_bytes += frame.vertices_alloc_bytes;
    phases.extract_alloc_count += frame.extract_alloc_count;
    phases.extract_alloc_bytes += frame.extract_alloc_bytes;
    phases.render_alloc_count += frame.render_alloc_count;
    phases.render_alloc_bytes += frame.render_alloc_bytes;
    if let Some(before) = frame_alloc_before {
        drop(mesh);
        drop(scene);
        let (all, _) = alloc_delta(before, alloc_snapshot());
        let covered =
            frame.vertices_alloc_count + frame.extract_alloc_count + frame.render_alloc_count;
        println!(
            "[P2] total={} v={} e={} r={} uncovered={}",
            all,
            frame.vertices_alloc_count,
            frame.extract_alloc_count,
            frame.render_alloc_count,
            all.saturating_sub(covered)
        );
    }
    frame
}

/// 第 `index` 个实例在场景里的 `(x, z)`（`y` 恒为 0）。`build_scene` 与剔除余量读数共用这一处，
/// 免得"量的布局"和"画的布局"各算一遍、哪天改了公式就悄悄错位。
fn instance_position(index: usize, mesh_count: usize) -> (f32, f32) {
    let x = (index % 100) as f32 * 0.18 - 9.0;
    let z = (index / 100) as f32 * 0.18 - (mesh_count as f32 / 100.0) * 0.09;
    (x, z)
}

/// `--scatter` 布局的网格边长（`ceil(sqrt(n))`）：10,000 个实例 ⇒ 100×100。
fn scatter_side(mesh_count: usize) -> usize {
    let mut side = 1usize;
    while side * side < mesh_count {
        side += 1;
    }
    side
}

/// `--scatter` 布局：边长 10 个单位的方格铺开（10,000 ⇒ 约 990×990），整体居中于原点。
fn instance_position_scatter(index: usize, mesh_count: usize) -> (f32, f32) {
    const SPACING: f32 = 10.0;
    let side = scatter_side(mesh_count);
    let half = (side as f32 - 1.0) * 0.5;
    let x = (index % side) as f32 * SPACING - half * SPACING;
    let z = (index / side) as f32 * SPACING - half * SPACING;
    (x, z)
}

/// `--scatter` 布局的相机：贴在场地 **+X 一侧**（离最近一排只有 20 个单位）向 -X 看。
/// 这样 ±(extent/2) 的 z 跨度在近侧张角远超水平半视场（640×360、fov 60 ⇒ ≈45.7°），
/// 大量实例落在视锥外——那正是剔除该拿的收益。默认布局的相机在 (0,0,20) 看向原点，
/// 整张 100×100 网格都在视锥内，读数恒为 0。
fn scatter_camera(mesh_count: usize) -> (f32, f32, f32) {
    let extent = (scatter_side(mesh_count) as f32 - 1.0) * 10.0;
    (extent * 0.5 + 20.0, 6.0, 0.0)
}

fn build_scene(mesh_count: usize, scatter: bool) -> Scene {
    let mut scene = Scene::new(format!("wp0-3d-{mesh_count}"));
    scene.add_entity(SceneEntityId(1), "Camera", None).unwrap();
    // 默认布局：100×100 网格全部落在视锥里（`--cull-headroom` 报 outside_pct=0.0）。
    // `--scatter`（M83-05）：网格铺开到千级范围、相机落到一角，让"看不见的实例"占多数——
    // 这是判"做不做剔除"唯一有意义的基准（没有可剔除对象时，任何结构都不会改善读数）。
    let camera_component = if scatter {
        let camera = scatter_camera(mesh_count);
        format!(
            "projection=perspective;position={},{},{};target=0,0,0;fov=60;near=0.1;far=5000",
            camera.0, camera.1, camera.2
        )
    } else {
        "projection=perspective;position=0,0,20;target=0,0,0;fov=60;near=0.1;far=100".into()
    };
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Camera".into(), camera_component);
    scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
    // M83-07 诊断开关（默认不变）：去掉方向光 ⇒ 没有阴影投射者，用来量"阴影链"占多少
    // 固定每帧分配。`WP0_NO_LIGHT=1` 只在诊断时设。
    if std::env::var("WP0_NO_LIGHT")
        .map(|v| v != "1")
        .unwrap_or(true)
    {
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Light".into(),
                "direction=0.4,-0.8,0.5;color=1,1,1;intensity=1".into(),
            );
    }
    // M83-07 诊断开关（默认不变）：`WP0_NO_MESHES=1` 生成"只有相机/灯光、零网格"的场景，
    // 用来把渲染段的分配拆成"每帧脚手架"与"逐 drawable 收集"两部分。
    let no_meshes = std::env::var("WP0_NO_MESHES")
        .map(|v| v == "1")
        .unwrap_or(false);
    let mesh_total = if no_meshes { 0 } else { mesh_count };
    for index in 0..mesh_total {
        let id = SceneEntityId(index as u64 + 3);
        scene.add_entity(id, format!("Mesh{index}"), None).unwrap();
        let (x, z) = if scatter {
            instance_position_scatter(index, mesh_count)
        } else {
            instance_position(index, mesh_count)
        };
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={x},0,{z};rotation=0,0,0,1;scale=0.08,0.08,0.08"),
        );
        entity.components.insert(
            "MeshRenderer".into(),
            "mesh=assets/models/m8-test-triangle.glb".into(),
        );
    }
    scene
}

/// 视锥剔除的**理论上限**：用引擎自己的 `Camera3D::frustum` + `contains_sphere` 数一遍，
/// 有多少实例根本不在视锥里。这是 docs/53 那条"下一步做 BVH/遮挡剔除"要回答的第一个问题——
/// 如果当前基准里 100% 的实例都在视锥内，那么无论剔除写得多好，**读数都不会动**，
/// 先补场景比先写结构重要。
/// 视锥剔除的**理论上限**：用引擎自己的 `Camera3D::frustum` + `contains_sphere` 数一遍，
/// 有多少实例根本不在视锥里。这是 docs/53 那条"下一步做 BVH/遮挡剔除"要回答的第一个问题——
/// 如果当前基准里 100% 的实例都在视锥内，那么无论剔除写得多好，**读数都不会动**，
/// 先补场景比先写结构重要。
///
/// 实例的包围球半径取变换 scale 的外接球（`0.08*sqrt(3)`，单位网格的最坏假设）；视口用基准的 640×360。
/// 同时给一个**阳性对照**（相机朝反方向看）：它必须报 `outside_pct=100.0`，否则说明这个读数
/// 自己就是坏的（永远返回"全在里面"的读数比没有读数更糟）。
fn cull_headroom(mesh_count: usize, scatter: bool) -> String {
    let viewport = rustlua_camera::camera::Viewport::new(640.0, 360.0).expect("viewport");
    let radius = 0.08_f32 * 3.0_f32.sqrt();
    // 默认布局：相机在 (0,0,20) 看向原点，整张 100×100 网格都在视锥内（outside=0）。
    // 散开布局：相机贴场地一角，预期**大部分实例在视锥外**——那才是剔除能拿到收益的场景。
    let (fixture_pos, fixture_target, far) = if scatter {
        let (x, y, z) = scatter_camera(mesh_count);
        (
            rustlua_math::math3d::Vec3::new(x, y, z),
            rustlua_math::math3d::Vec3::ZERO,
            5000.0,
        )
    } else {
        (
            rustlua_math::math3d::Vec3::new(0.0, 0.0, 20.0),
            rustlua_math::math3d::Vec3::ZERO,
            100.0,
        )
    };
    let fixture = cull_headroom_for(
        mesh_count,
        viewport,
        "fixture",
        fixture_pos,
        fixture_target,
        radius,
        far,
        scatter,
    );
    // 阳性对照：同一位置看**反方向**，必须报 100% 外——否则说明读数本身坏了
    // （永远返回"全在里面"的读数比没有读数更糟）。
    let (control_pos, control_target) = if scatter {
        (
            fixture_pos,
            rustlua_math::math3d::Vec3::new(fixture_pos.x + 50.0, fixture_pos.y, fixture_pos.z),
        )
    } else {
        (
            rustlua_math::math3d::Vec3::new(0.0, 0.0, 20.0),
            rustlua_math::math3d::Vec3::new(0.0, 0.0, 40.0),
        )
    };
    let control = cull_headroom_for(
        mesh_count,
        viewport,
        "control_looking_away",
        control_pos,
        control_target,
        radius,
        far,
        scatter,
    );
    format!("{fixture}\n{control}")
}

#[allow(clippy::too_many_arguments)] // 探针参数彼此独立，拆结构体只会加一层映射。
fn cull_headroom_for(
    mesh_count: usize,
    viewport: rustlua_camera::camera::Viewport,
    label: &str,
    position: rustlua_math::math3d::Vec3,
    target: rustlua_math::math3d::Vec3,
    radius: f32,
    far: f32,
    scatter: bool,
) -> String {
    let camera = rustlua_camera::camera::Camera3D::perspective(
        position,
        target,
        60.0_f32.to_radians(),
        0.1,
        far,
    )
    .expect("benchmark camera");
    let frustum = camera.frustum(viewport).expect("benchmark frustum");
    let mut inside = 0usize;
    for index in 0..mesh_count {
        let (x, z) = if scatter {
            instance_position_scatter(index, mesh_count)
        } else {
            instance_position(index, mesh_count)
        };
        if frustum.contains_sphere(rustlua_math::math3d::Vec3::new(x, 0.0, z), radius) {
            inside += 1;
        }
    }
    let outside = mesh_count - inside;
    let outside_pct = if mesh_count == 0 {
        0.0
    } else {
        outside as f64 * 100.0 / mesh_count as f64
    };
    format!(
        "wp0_3d_cull_headroom camera={label} layout={} mesh_count={mesh_count} total={mesh_count} \
         inside={inside} outside={outside} outside_pct={outside_pct:.1} radius={radius:.3}",
        if scatter { "scatter" } else { "dense" }
    )
}

fn reset_allocations() {
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    BASELINE_LIVE_BYTES.store(baseline, Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
}
fn summarize(mut values: Vec<Duration>) -> Stats {
    values.sort_unstable();
    let at = |p: usize| ((values.len() * p).saturating_sub(1) / 100).min(values.len() - 1);
    Stats {
        count: values.len(),
        min_us: values[0].as_micros(),
        average_us: values.iter().map(Duration::as_micros).sum::<u128>() / values.len() as u128,
        p50_us: values[at(50)].as_micros(),
        p95_us: values[at(95)].as_micros(),
        p99_us: values[at(99)].as_micros(),
        max_us: values[values.len() - 1].as_micros(),
    }
}
fn stats_json(s: Stats) -> String {
    format!(
        "{{\"count\":{},\"min_us\":{},\"average_us\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"max_us\":{}}}",
        s.count, s.min_us, s.average_us, s.p50_us, s.p95_us, s.p99_us, s.max_us
    )
}
fn format_report(stairs: &[String], cycles: usize) -> String {
    format!(
        "{{\"report_version\":\"wp0-v3\",\"benchmark\":\"3d_render_stress\",\"target\":{{\"width\":640,\"height\":360,\"cycles\":{cycles},\"warmup\":3}},\"stairs\":[{}]}}\n",
        stairs.join(",")
    )
}
