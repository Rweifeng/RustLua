//! 临时探针：按“设计分辨率排版 + 真实窗口像素”出图，用来对比文字清晰度。
//!
//!     cargo run -q -p engine --example royale_text_probe -- E:/Qoder/ClashRoyale out.png 541 961 [vector|sdf]

use std::path::PathBuf;

use engine::host::RuntimeScene;
use engine::host::runtime_scene::TextRenderMode;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

const DESIGN_W: u32 = 720;
const DESIGN_H: u32 = 1280;

fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("E:/Qoder/ClashRoyale"));
    let output = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("artifacts").join("text-probe.png"));
    let width: u32 = std::env::args()
        .nth(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DESIGN_W);
    let height: u32 = std::env::args()
        .nth(4)
        .and_then(|value| value.parse().ok())
        .unwrap_or(DESIGN_H);

    let scene = Scene::load(root.join("scenes").join("battle.scene"))
        .map_err(|error| format!("加载 battle.scene：{error}"))?;
    let mut runtime =
        RuntimeScene::from_scene(Some(&root), scene).map_err(|error| format!("实例化：{error}"))?;
    match std::env::args().nth(5).as_deref() {
        Some("vector") => runtime.set_text_render_mode(TextRenderMode::Vector),
        Some("sdf") => runtime.set_text_render_mode(TextRenderMode::Sdf),
        Some("auto") | None => runtime.set_text_render_mode(TextRenderMode::Auto),
        Some(other) => return Err(format!("未知文字模式：{other}（vector|sdf|auto）")),
    }
    runtime
        .with_script(|script| script.eval_json("__royale_autoplay(true) or true"))
        .map_err(|error| format!("开启 AI 自走：{error}"))?;
    let input = InputSnapshot::default();
    runtime
        .vertices(DESIGN_W, DESIGN_H)
        .map_err(|error| format!("相机回填：{error}"))?;
    for _ in 0..300 {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .map_err(|error| format!("脚本步：{error}"))?;
    }
    let mesh = runtime
        .vertices(DESIGN_W, DESIGN_H)
        .map_err(|error| format!("HUD 提取：{error}"))?;
    // 与宿主一致：3D 相机按**设计分辨率**取景，渲染器再把设计盒 contain 到真实窗口像素。
    let viewport = rustlua_camera::camera::Viewport::new(DESIGN_W as f32, DESIGN_H as f32)
        .map_err(|error| format!("viewport：{error}"))?;
    let scene_3d = runtime
        .scene_3d(viewport)
        .ok_or_else(|| "场景里没有 3D 视图".to_owned())?;

    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new("text-probe", width, height))
        .map_err(|error| format!("离屏设备：{error}"))?;
    let mut renderer = Renderer::new_offscreen(
        fixture.device().clone(),
        fixture.queue().clone(),
        Some(root.clone()),
        false,
        true,
    )
    .map_err(|error| format!("renderer：{error}"))?;
    if !matches!(
        renderer.render_into_texture(&mesh, Some(&scene_3d), fixture.view(), width, height),
        RenderOutcome::Presented
    ) {
        return Err("离屏渲染没有出帧".to_owned());
    }
    let pixels = renderer
        .copy_texture_to_cpu(fixture.texture(), width, height)
        .map_err(|error| format!("读回像素：{error}"))?;
    let mut image = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        width,
        height,
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
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir：{error}"))?;
    }
    image
        .save(&output)
        .map_err(|error| format!("保存 PNG：{error}"))?;
    println!(
        "text_probe_ok png={} size={width}x{height}",
        output.display()
    );
    Ok(())
}
