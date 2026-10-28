//! 把 TankBattle 工程离屏渲染成 PNG（软光栅 fallback adapter，无窗口、无输入）。
//!
//!     cargo run -q -p engine --example tank_shot -- E:/Qoder/TankBattle target/tank-shot.png
//!
//! 第三个参数可选：先步进的帧数（默认 120，约 2 秒，能刷出 1 个敌人并让 HUD 画满）。
//! 同时打印运行态 JSON 与每条 draw 的绑定，作为"画面上真的有什么"的证据。
use std::path::PathBuf;

use engine::host::RuntimeScene;
use rustlua_gpu::{RenderOutcome, Renderer};
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 720;

fn main() -> Result<(), String> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "E:/Qoder/TankBattle".to_owned()),
    );
    let output = PathBuf::from(
        std::env::args()
            .nth(2)
            .unwrap_or_else(|| "target/tank-shot.png".to_owned()),
    );
    let frames: usize = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(120);

    let scene_path = root.join("scenes").join("tank_battle.scene");
    let scene = Scene::load(&scene_path)
        .unwrap_or_else(|error| panic!("load {}: {error}", scene_path.display()));
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("instantiate");
    for _ in 0..frames {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .map_err(|error| format!("script steps: {error}"))?;
    }
    let state = runtime
        .with_script(|script| script.eval_json("__tank_debug_state()"))
        .map_err(|error| format!("debug state: {error}"))?;
    println!("state={state}");

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback adapter is required");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("tank-shot"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| format!("device: {error}"))?;
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, Some(root), false, true)
        .map_err(|error| format!("renderer: {error}"))?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tank-shot-target"),
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
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .map_err(|error| format!("vertices: {error}"))?;
    if !matches!(
        renderer.render_into_texture(&mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ) {
        return Err("render did not present".to_owned());
    }
    let pixels = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .map_err(|error| format!("readback: {error}"))?;
    let mut image = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        WIDTH,
        HEIGHT,
        image::Rgba([0, 0, 0, 0]),
    );
    for (index, pixel) in image.pixels_mut().enumerate() {
        let src = index * 4;
        *pixel = image::Rgba([
            pixels[src],
            pixels[src + 1],
            pixels[src + 2],
            pixels[src + 3],
        ]);
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir: {error}"))?;
    }
    image
        .save(&output)
        .map_err(|error| format!("save: {error}"))?;
    for draw in &mesh.draws {
        let path = if draw.texture == 0 {
            "<white>".to_owned()
        } else {
            mesh.paths
                .get((draw.texture - 1) as usize)
                .cloned()
                .unwrap_or_else(|| "<missing>".to_owned())
        };
        println!(
            "draw tex={:>2} {:<52} count={:<6} sdf={} bitmap={} vector={}",
            draw.texture, path, draw.count, draw.sdf, draw.bitmap, draw.vector
        );
    }
    println!("wrote {} (draws={})", output.display(), mesh.draws.len());
    Ok(())
}
