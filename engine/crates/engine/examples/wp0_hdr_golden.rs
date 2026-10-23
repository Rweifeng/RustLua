//! P3-R01 HDR golden（`wp0-hdr-golden-v1`）：**Tonemap 之前有 >1 的线性值，之后 ≤1，
//! 且 UI 不被 tonemap 改色**。
//!
//! 与 2D/HUD golden 的分工：那两条只看 8 位目标，`Rgba16Float` 是否存在、ACES 是否真的把
//! 8.0 压回 LDR、UI 是不是画在 Tonemap 之后，都看不出。这条 golden 用同一帧给出三组读数：
//!
//! - `pre_tonemap_max`：HDR 场景目标（线性 f16）里的最大 RGB。世界 Sprite 的顶点色是 8.0，
//!   8 位目标必然把它夹到 1.0，因此该值 >1 就是"浮点场景色真的存在"的证据。
//! - `post_tonemap_max`：最终 8 位目标的归一化最大值，必须 ≤1。
//! - `ui_delta`：HDR 开/关两次渲染里同一块 UI 的最大通道差（字节）。UI 画在 Tonemap 之后，
//!   所以它必须 ≤1（量化容差）。
//!
//! 能力不可用时**不伪装 pass**：无适配器 → exit 3（BLOCKED）；有适配器但不支持 HDR →
//! 走 LDR 分支、报告里写明原因、exit 3。只有三条读数都成立才 exit 0。
//!
//! 运行：`cargo run -p engine --example wp0_hdr_golden -- --output artifacts/p3/hdr`

use std::{env, fs, path::PathBuf, process::ExitCode};

use engine::host::frame::{
    OverlayVertex, SceneDraw, SceneMesh, WHITE_TEXTURE_PATH, WorldSpriteVertex,
};
use engine::host::{RenderOutcome, RenderSettings, Renderer, ToggleMode};

use rustlua_camera::camera::{Camera2D, Viewport};
use rustlua_ecs::transform::Vec2;
use rustlua_render::layer::RenderLayer;
use rustlua_render::sprite_batch::SpriteDrawBatch;
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// 世界 Sprite 的线性颜色：8.0 远超 LDR 上限，只有浮点场景目标能原样保存。
const HDR_WORLD: f32 = 8.0;
/// UI 的纯红：ACES/Reinhard 都会把它改成暗红，因此"UI 颜色没变"能证明分界正确。
const UI_COLOR: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn main() -> ExitCode {
    let output = parse_output();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        eprintln!("BLOCKED: no GPU adapter available; HDR golden cannot run (没跑 ≠ 通过)");
        return ExitCode::from(3);
    };
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-hdr-golden"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request golden device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let capabilities = renderer.capabilities();

    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let camera = Camera2D::new(Vec2::new(0.0, 0.0), WIDTH as f32).expect("camera2d");
    renderer.set_world_sprite_camera(
        camera
            .view_projection(viewport, -1.0, 1.0)
            .expect("view projection"),
    );

    let mesh = hdr_mesh();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp0-hdr-golden-target"),
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
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // ① HDR 开启：世界 8.0 → 线性 f16 场景目标 → ACES → LDR；UI 在之后。
    renderer.set_render_settings(RenderSettings::default().with_hdr(ToggleMode::On));
    assert!(matches!(
        renderer.render_into_texture(&mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let hdr_metrics = renderer.last_frame_metrics();
    let hdr_passes: Vec<String> = renderer
        .last_frame_plan()
        .passes()
        .iter()
        .map(|pass| format!("{pass:?}"))
        .collect();

    if !hdr_metrics.hdr_active {
        // LDR 分支：仍然出图并报告，但这不是 HDR 证据，退出码必须是 BLOCKED。
        let ldr_pixels = renderer
            .copy_texture_to_cpu(&target, WIDTH, HEIGHT)
            .expect("LDR fallback readback");
        let report = json!({
            "report_version": "wp0-hdr-golden-v1",
            "status": "blocked",
            "blocked_reason": blocked_reason(&capabilities),
            "target": {"width": WIDTH, "height": HEIGHT},
            "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
            "hdr": {
                "active": false,
                "scene_format": hdr_metrics.scene_format.as_str(),
                "tonemap": null,
                "pre_tonemap_max": null,
                "post_tonemap_max": post_tonemap_max(&ldr_pixels),
                "ui_delta": null,
            },
            "passes": hdr_passes,
        });
        emit(&report, output.as_ref());
        eprintln!(
            "BLOCKED: HDR 不可用（{}）；已走 LDR 分支，未产生 HDR 证据（没跑 ≠ 通过）",
            blocked_reason(&capabilities)
        );
        return ExitCode::from(3);
    }

    let scene = renderer
        .read_scene_color_to_cpu()
        .expect("HDR scene color readback");
    let pre_tonemap_max = scene.max_rgb();
    let hdr_pixels = renderer
        .copy_texture_to_cpu(&target, WIDTH, HEIGHT)
        .expect("HDR frame readback");
    let post = post_tonemap_max(&hdr_pixels);
    let ui_hdr = sample_ui(&hdr_pixels, WIDTH);

    // ② 同一帧关闭 HDR：UI 像素必须一致（UI 画在 Tonemap 之后）。
    renderer.set_render_settings(RenderSettings::default());
    assert!(matches!(
        renderer.render_into_texture(&mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    assert!(
        !renderer.last_frame_metrics().hdr_active,
        "回滚档必须是 LDR"
    );
    let ldr_pixels = renderer
        .copy_texture_to_cpu(&target, WIDTH, HEIGHT)
        .expect("LDR frame readback");
    let ui_ldr = sample_ui(&ldr_pixels, WIDTH);
    let ui_delta = (0..4)
        .map(|channel| ui_hdr[channel].abs_diff(ui_ldr[channel]))
        .max()
        .unwrap_or(0);
    let world_sample = sample_pixel(&hdr_pixels, WIDTH, WIDTH / 4, HEIGHT / 2);
    let world_sample_ldr = sample_pixel(&ldr_pixels, WIDTH, WIDTH / 4, HEIGHT / 2);

    // 负向门：任何一条读数不成立都必须非 0 退出，不能写成 pass。
    let pre_ok = pre_tonemap_max > 1.0;
    let post_ok = post <= 1.0;
    let ui_ok = ui_delta <= 1;
    let passed = pre_ok && post_ok && ui_ok;

    let report = json!({
        "report_version": "wp0-hdr-golden-v1",
        "status": if passed { "pass" } else { "fail" },
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "hdr": {
            "active": hdr_metrics.hdr_active,
            "scene_format": hdr_metrics.scene_format.as_str(),
            "tonemap": hdr_metrics.tonemap.map(|curve| curve.as_str()),
            "pre_tonemap_max": pre_tonemap_max,
            "post_tonemap_max": post,
            "ui_delta": ui_delta,
            "ui_delta_normalized": f64::from(ui_delta) / 255.0,
            "ui_sample_hdr": ui_hdr,
            "ui_sample_ldr": ui_ldr,
            "world_sample_hdr": world_sample,
            "world_sample_ldr": world_sample_ldr,
        },
        "checks": {
            "pre_tonemap_above_one": pre_ok,
            "post_tonemap_within_ldr": post_ok,
            "ui_bypasses_tonemap": ui_ok,
        },
        "passes": hdr_passes,
    });
    emit(&report, output.as_ref());

    if !passed {
        eprintln!(
            "FAIL: pre_tonemap_max={pre_tonemap_max} (需 >1), post_tonemap_max={post} (需 ≤1), ui_delta={ui_delta} (需 ≤1)"
        );
        return ExitCode::from(1);
    }
    println!(
        "PASS: pre_tonemap_max={pre_tonemap_max:.3} post_tonemap_max={post:.3} ui_delta={ui_delta}"
    );
    ExitCode::SUCCESS
}

/// 全屏世界 Sprite（线性 8.0）+ 居中 UI 纯红四边形。
fn hdr_mesh() -> SceneMesh {
    let mut mesh = SceneMesh {
        vertices: ui_quad(),
        draws: vec![SceneDraw::screen_ui(0, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: vec![SpriteDrawBatch::translucent(
            0,
            0,
            RenderLayer::World2D,
            0,
            6,
        )],
        paths: vec![WHITE_TEXTURE_PATH.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };
    for (x, y) in [
        (-0.5_f32, -0.5_f32),
        (0.5, -0.5),
        (-0.5, 0.5),
        (0.5, -0.5),
        (0.5, 0.5),
        (-0.5, 0.5),
    ] {
        mesh.world_sprites.push(WorldSpriteVertex {
            position: [x, y, 0.0],
            uv: [0.5, 0.5],
            color: [HDR_WORLD, HDR_WORLD, HDR_WORLD, 1.0],
        });
    }
    mesh
}

/// 居中的 UI 四边形（NDC 半宽 0.25）。
fn ui_quad() -> Vec<OverlayVertex> {
    [
        (-0.25_f32, -0.25_f32),
        (0.25, -0.25),
        (-0.25, 0.25),
        (0.25, -0.25),
        (0.25, 0.25),
        (-0.25, 0.25),
    ]
    .into_iter()
    .map(|(x, y)| OverlayVertex {
        position: [x, y],
        uv: [0.5, 0.5],
        color: UI_COLOR,
        sdf: 0,
    })
    .collect()
}

fn blocked_reason(capabilities: &engine::host::RendererCapabilities) -> String {
    capabilities
        .hdr_unavailable_reason()
        .unwrap_or_else(|| "HDR 管线未构造".to_owned())
}

fn sample_pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

/// UI 中心像素（NDC 原点 = 目标中心）。
fn sample_ui(pixels: &[u8], width: u32) -> [u8; 4] {
    sample_pixel(pixels, width, width / 2, HEIGHT / 2)
}

/// 最终 8 位目标的最大通道值归一化到 `0..=1`。
fn post_tonemap_max(pixels: &[u8]) -> f64 {
    pixels
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .map(|channel| f64::from(channel) / 255.0)
        .fold(0.0_f64, f64::max)
}

fn emit(report: &serde_json::Value, output: Option<&PathBuf>) {
    let text = serde_json::to_string_pretty(report).expect("serialize golden") + "\n";
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).expect("create golden output directory");
        }
        fs::write(path, text).expect("write golden report");
        println!("report: {}", path.display());
    } else {
        print!("{text}");
    }
}

fn parse_output() -> Option<PathBuf> {
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ))
            }
            "--help" | "-h" => {
                println!("Usage: wp0_hdr_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
