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
}

fn main() {
    let config = parse_config();
    if config.cull_headroom {
        for mesh_count in [100usize, 1_000, 10_000] {
            println!("{}", cull_headroom(mesh_count));
        }
        return;
    }
    let root = games_dir().join("examples").join("game_api");
    let mut reports = Vec::new();
    for mesh_count in [100usize, 1_000, 10_000] {
        reports.push(run_stair(&root, mesh_count, config.cycles, config.profile));
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
            "--help" | "-h" => {
                println!(
                    "Usage: wp0_3d_stress [--cycles N] [--output PATH] [--profile] [--cull-headroom]"
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
    }
}

fn run_stair(root: &Path, mesh_count: usize, cycles: usize, profile: bool) -> String {
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root), build_scene(mesh_count)).expect("runtime scene");
    let fixture = OffscreenTarget::acquire(
        &OffscreenRequest::new("wp0-3d-stress", 640, 360).with_timestamps(),
    )
    .expect("offscreen fixture for wp0-3d-stress");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.to_path_buf()), true, true)
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
        format!(
            "{{\"vertices_us\":{},\"extract_us\":{},\"render_us\":{},\"render_share_pct\":{}}}",
            phases.vertices_us / n,
            phases.extract_us / n,
            phases.render_us / n,
            // 渲染段占三段之和的百分比：决定"下一步该优化提取还是优化渲染"的那把尺子。
            (100 * phases.render_us
                / (phases.vertices_us + phases.extract_us + phases.render_us).max(1))
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

/// `--profile` 的分段累计（微秒）。三段的分界就是宿主每帧真正做的三件事：
/// 2D 顶点提取、3D 提取（`scene_3d` 把 ECS 摊平成 drawable）、以及一次 `render_into_texture`
/// （剔除 + 合批 + 写实例缓冲 + 录制 + 提交）。
#[derive(Default, Clone, Copy)]
struct PhaseTotals {
    vertices_us: u128,
    extract_us: u128,
    render_us: u128,
}

fn render_once(
    runtime: &mut RuntimeScene,
    renderer: &mut Renderer,
    view: &wgpu::TextureView,
    last: &mut DefaultMetrics,
    phases: &mut PhaseTotals,
    profile: bool,
) -> PhaseTotals {
    let started_vertices = profile.then(Instant::now);
    let mesh = runtime.vertices(640, 360).expect("2D extraction");
    let vertices_us = started_vertices.map_or(0, |t| t.elapsed().as_micros());
    let started_extract = profile.then(Instant::now);
    let scene =
        runtime.scene_3d(rustlua_camera::camera::Viewport::new(640.0, 360.0).expect("viewport"));
    let extract_us = started_extract.map_or(0, |t| t.elapsed().as_micros());
    let started_render = profile.then(Instant::now);
    assert!(matches!(
        renderer.render_into_texture(&mesh, scene.as_ref(), view, 640, 360, true),
        RenderOutcome::Presented
    ));
    let render_us = started_render.map_or(0, |t| t.elapsed().as_micros());
    let metrics = renderer.last_frame_metrics();
    last.cpu_frame_us = metrics.cpu_frame_us;
    last.gpu_frame_us = metrics.gpu_frame_us;
    last.draw_calls = metrics.draw_calls;
    last.instances = metrics.instances;
    last.triangles = metrics.triangles;
    let frame = PhaseTotals {
        vertices_us,
        extract_us,
        render_us,
    };
    phases.vertices_us += frame.vertices_us;
    phases.extract_us += frame.extract_us;
    phases.render_us += frame.render_us;
    frame
}

/// 第 `index` 个实例在场景里的 `(x, z)`（`y` 恒为 0）。`build_scene` 与剔除余量读数共用这一处，
/// 免得"量的布局"和"画的布局"各算一遍、哪天改了公式就悄悄错位。
fn instance_position(index: usize, mesh_count: usize) -> (f32, f32) {
    let x = (index % 100) as f32 * 0.18 - 9.0;
    let z = (index / 100) as f32 * 0.18 - (mesh_count as f32 / 100.0) * 0.09;
    (x, z)
}

fn build_scene(mesh_count: usize) -> Scene {
    let mut scene = Scene::new(format!("wp0-3d-{mesh_count}"));
    scene.add_entity(SceneEntityId(1), "Camera", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            "projection=perspective;position=0,0,20;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );
    scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Light".into(),
            "direction=0.4,-0.8,0.5;color=1,1,1;intensity=1".into(),
        );
    for index in 0..mesh_count {
        let id = SceneEntityId(index as u64 + 3);
        scene.add_entity(id, format!("Mesh{index}"), None).unwrap();
        let (x, z) = instance_position(index, mesh_count);
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
fn cull_headroom(mesh_count: usize) -> String {
    let viewport = rustlua_camera::camera::Viewport::new(640.0, 360.0).expect("viewport");
    let radius = 0.08_f32 * 3.0_f32.sqrt();
    let fixture = cull_headroom_for(
        mesh_count,
        viewport,
        "fixture",
        rustlua_math::math3d::Vec3::new(0.0, 0.0, 20.0),
        rustlua_math::math3d::Vec3::ZERO,
        radius,
    );
    // 对照：同一位置、朝 +Z 看（背离整个 100×100 网格）⇒ 理论上一个都看不见。
    let control = cull_headroom_for(
        mesh_count,
        viewport,
        "control_looking_away",
        rustlua_math::math3d::Vec3::new(0.0, 0.0, 20.0),
        rustlua_math::math3d::Vec3::new(0.0, 0.0, 40.0),
        radius,
    );
    format!("{fixture}\n{control}")
}

fn cull_headroom_for(
    mesh_count: usize,
    viewport: rustlua_camera::camera::Viewport,
    label: &str,
    position: rustlua_math::math3d::Vec3,
    target: rustlua_math::math3d::Vec3,
    radius: f32,
) -> String {
    let camera = rustlua_camera::camera::Camera3D::perspective(
        position,
        target,
        60.0_f32.to_radians(),
        0.1,
        100.0,
    )
    .expect("benchmark camera");
    let frustum = camera.frustum(viewport).expect("benchmark frustum");
    let mut inside = 0usize;
    for index in 0..mesh_count {
        let (x, z) = instance_position(index, mesh_count);
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
        "wp0_3d_cull_headroom camera={label} mesh_count={mesh_count} total={mesh_count} \
         inside={inside} outside={outside} outside_pct={outside_pct:.1} radius={radius:.3}"
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
