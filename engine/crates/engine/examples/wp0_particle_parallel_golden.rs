//! P2-① 证据：P1-③ 粒子并行（`step_particles_with_parallelism`）的像素级一致性断言。
//!
//! 粒子并行是**执行路径优化**：同样的发射/推进语义，串行与多线程必须产生
//! 逐字节相同的粒子状态，否则并行就不是等价变换。证据：
//! * 同一 `unified.scene`（含 `rate=24` 连续发射器）各推进 30 帧；
//! * A 用强制串行（`Parallelism::SEQUENTIAL`），B 用 4 线程且**阈值压到 1**
//!   （`with_min_items(1)`）保证并行路径真实启用；
//! * 渲染两帧后 read-back 全帧像素**逐字节相等**。

use std::{env, fs, path::PathBuf};

use engine::{
    core::{games_dir, par::Parallelism, scene::Scene},
    host::{RenderOutcome, Renderer, RuntimeScene},
};
use serde_json::json;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
const FRAMES: u32 = 30;
const DELTA: f32 = 1.0 / 60.0;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let scene_path = root.join("scenes/unified.scene");
    let scene = Scene::load(&scene_path).expect("load unified scene");

    // A：强制串行；B：4 线程并行（阈值压到 1，保证小场景也真走并行路径）。
    let mut runtime_sequential =
        RuntimeScene::from_scene_edit(Some(&root), scene.clone()).expect("runtime (sequential)");
    let mut runtime_parallel =
        RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime (parallel)");

    for _ in 0..FRAMES {
        runtime_sequential.step_scene_emitters(DELTA);
        runtime_sequential.with_script(|script| {
            script.step_particles_with_parallelism(DELTA, Parallelism::SEQUENTIAL);
        });
        runtime_parallel.step_scene_emitters(DELTA);
        runtime_parallel.with_script(|script| {
            script.step_particles_with_parallelism(
                DELTA,
                Parallelism::with_threads(4).with_min_items(1),
            );
        });
    }

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the particle parallel golden");
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-particle-parallel-golden"),
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
        label: Some("wp0-particle-parallel-golden-target"),
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

    let mesh_seq = runtime_sequential
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (seq)");
    assert!(matches!(
        renderer.render_into_texture(&mesh_seq, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_seq = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (seq)");
    let metrics_seq = renderer.last_frame_metrics();

    let mesh_par = runtime_parallel
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (par)");
    assert!(matches!(
        renderer.render_into_texture(&mesh_par, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_par = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (par)");
    let metrics_par = renderer.last_frame_metrics();

    assert_eq!(
        pixels_par.len(),
        pixels_seq.len(),
        "两次 read-back 缓冲长度不一致（尺寸不该变）"
    );
    let identical = pixels_par == pixels_seq;
    if !identical {
        let diff = pixels_seq
            .iter()
            .zip(pixels_par.iter())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .unwrap_or(0);
        panic!(
            "粒子并行改变了渲染像素：首个差异字节 offset={diff}（缓冲 {} 字节）\
             —— 并行推进不是等价变换（粒子状态被线程数影响）",
            pixels_seq.len()
        );
    }

    // 两帧必须真的有粒子内容（否则"一致"是空转一致）。
    let background = [pixels_seq[0], pixels_seq[1], pixels_seq[2]];
    let content = count_non_background(&pixels_seq, &background);
    assert!(
        content >= 500,
        "推进 30 帧后粒子帧内容过少：{content}（发射器没工作？）"
    );

    let report = json!({
        "report_version": "wp0-particle-parallel-golden-v1",
        "scene": "unified.scene (ParticleEmitter rate=24)",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "frames": FRAMES,
            "delta": DELTA,
            "sequential": "Parallelism::SEQUENTIAL",
            "parallel": "Parallelism::with_threads(4).with_min_items(1)",
            "pixels_identical": identical,
            "buffer_bytes": pixels_seq.len(),
            "content_pixels": content,
            "vertices_seq": mesh_seq.vertices.len(),
            "vertices_par": mesh_par.vertices.len(),
            "draw_calls_seq": metrics_seq.draw_calls,
            "draw_calls_par": metrics_par.draw_calls,
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
        "wp0_particle_parallel_golden_ok pixels_identical=true content={content} frames={FRAMES}"
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
