//! WP0 3D evidence benchmark: fixed offscreen scene, mesh stairs, CPU allocation counters.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use engine::{
    core::{
        games_dir,
        scene::{Scene, SceneEntityId},
    },
    host::{RenderOutcome, Renderer, RuntimeScene},
};

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
}

fn main() {
    let config = parse_config();
    let root = games_dir().join("examples").join("game_api");
    let mut reports = Vec::new();
    for mesh_count in [100usize, 1_000, 10_000] {
        reports.push(run_stair(&root, mesh_count, config.cycles));
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
            "--help" | "-h" => {
                println!("Usage: wp0_3d_stress [--cycles N] [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(cycles > 0, "--cycles must be greater than zero");
    Config { output, cycles }
}

fn run_stair(root: &Path, mesh_count: usize, cycles: usize) -> String {
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root), build_scene(mesh_count)).expect("runtime scene");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the 3D benchmark");
    let timestamp_features =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let required_features = if adapter.features().contains(timestamp_features) {
        timestamp_features
    } else {
        wgpu::Features::empty()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-3d-stress"),
        required_features,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request benchmark device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.to_path_buf()), true, true)
            .expect("renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp0-3d-target"),
        size: wgpu::Extent3d {
            width: 640,
            height: 360,
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
    let mut samples = Vec::with_capacity(cycles);
    let mut gpu_samples = Vec::with_capacity(cycles);
    let mut last = DefaultMetrics::default();
    for _ in 0..3 {
        render_once(&mut runtime, &mut renderer, &view, &mut last);
    }
    reset_allocations();
    for _ in 0..cycles {
        let started = Instant::now();
        render_once(&mut runtime, &mut renderer, &view, &mut last);
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
    format!(
        "{{\"mesh_count\":{mesh_count},\"samples\":{},\"render\":{{\"cpu_frame_us\":{},\"gpu_frame_us\":{gpu_json},\"gpu_timestamp_supported\":{},\"gpu_timestamp_reason\":\"{gpu_reason}\",\"draw_calls\":{},\"instances\":{},\"triangles\":{}}},\"cpu_alloc_count\":{},\"cpu_alloc_bytes\":{},\"cpu_live_bytes\":{},\"cpu_peak_bytes\":{},\"known_renderer_bytes\":{{\"vertex_buffer_capacity\":{},\"vertex_bytes_peak\":{}}}}}",
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
fn render_once(
    runtime: &mut RuntimeScene,
    renderer: &mut Renderer,
    view: &wgpu::TextureView,
    last: &mut DefaultMetrics,
) {
    let mesh = runtime.vertices(640, 360).expect("2D extraction");
    let scene = runtime.scene_3d(640.0 / 360.0);
    assert!(matches!(
        renderer.render_into_texture(&mesh, scene.as_ref(), view, 640, 360),
        RenderOutcome::Presented
    ));
    let metrics = renderer.last_frame_metrics();
    last.cpu_frame_us = metrics.cpu_frame_us;
    last.gpu_frame_us = metrics.gpu_frame_us;
    last.draw_calls = metrics.draw_calls;
    last.instances = metrics.instances;
    last.triangles = metrics.triangles;
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
        let x = (index % 100) as f32 * 0.18 - 9.0;
        let z = (index / 100) as f32 * 0.18 - (mesh_count as f32 / 100.0) * 0.09;
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
