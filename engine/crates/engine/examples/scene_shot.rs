//! 把示例工程的某个场景**离屏渲染成 PNG**：版面问题的判据是"看得见"，而"看得见"
//! 不该只有人眼这一条通道（截图要开真窗口，CI 里没有）。
//!
//!     cargo run -q -p engine --example scene_shot -- scenes/index.scene target/index-shot.png
//!
//! 第二个参数缺省 `target/index-shot.png`，第一个参数缺省 `scenes/index.scene`
//! （相对 `examples/game_api`）。同时打印每条 draw 绑定的资源与 `sdf/bitmap/vector`
//! 标记——文字"画错了"先要分清是哪条排版通道画的（矢量档与 MSDF 档的字距规则不同）。
//!
//! 走的是软光栅 fallback adapter + `Renderer::new_offscreen`，因此**没有**窗口、
//! 输入与脚本热路径；它只回答"这一帧的几何长什么样"。
use std::path::PathBuf;

use engine::host::RuntimeScene;
use rustlua_gpu::{RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

fn main() -> Result<(), String> {
    let file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "scenes/index.scene".to_owned());
    let output = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "target/index-shot.png".to_owned());
    let root = games_dir().join("examples").join("game_api");
    let path = if std::path::Path::new(&file).is_absolute() {
        PathBuf::from(&file)
    } else {
        root.join(&file)
    };

    let scene = Scene::load(&path).unwrap_or_else(|error| panic!("load {path:?}: {error}"));
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("instantiate");
    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .map_err(|error| format!("script steps: {error}"))?;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback adapter is required");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("scene-shot"),
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
        label: Some("scene-shot-target"),
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
    let target = PathBuf::from(&output);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir: {error}"))?;
    }
    image
        .save(&target)
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
    println!("wrote {} (draws={})", target.display(), mesh.draws.len());
    Ok(())
}
