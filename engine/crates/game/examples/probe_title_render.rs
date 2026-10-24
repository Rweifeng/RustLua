//! 临时探针（M63 修复用，用完即删）：引擎离屏渲染 prefab 文档 → PNG，验证编辑器预览
//! 路径的真实像素（纹理加载是否失败、背景是否黑）。
//!
//! Usage: cargo run -q -p game --example probe_title_render -- <game_jhyt-root> --offscreen

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_gpu::Renderer;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::Scene;
use std::{env, path::PathBuf};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let project = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            rustlua_infra::games_dir()
                .join("examples")
                .join("game_jhyt")
        });
    let project = project.canonicalize()?;
    let prefab_flag = args.iter().position(|arg| arg == "--prefab");
    let sceneview_flag = args.iter().any(|arg| arg == "--sceneview");

    // 构造与编辑器 Scene View 相同的场景来源。
    let mut runtime = if let Some(index) = prefab_flag {
        let prefab_path = args.get(index + 1).ok_or("--prefab 后要跟 prefab 路径")?;
        let prefab = Prefab::load(project.join(prefab_path))?;
        let mut scene = Scene::new(prefab.name.clone());
        scene.entities = prefab.entities.clone();
        scene.metadata = prefab.metadata.clone();
        println!(
            "prefab_mode name={} entities={}",
            prefab.name,
            scene.entities.len()
        );
        RuntimeScene::from_scene_edit(Some(project.as_path()), scene)?
    } else {
        println!("boot_mode scene.load('main')");
        RuntimeScene::new(Some(project.as_path()))?
    };

    if sceneview_flag {
        // 精确复现 Scene View 的 UI 覆盖层调用序列：
        // game_camera 中心 = DEFAULT_2D_CAMERA_CENTER(640,360)、zoom=1；
        // set_render_ui_overlay(true) + set_ui_reference([1280,720]) + vertices(1280,720)。
        use rustlua_camera::camera::Camera2D;
        use rustlua_math::math2d::Vec2;
        let game_camera =
            Camera2D::new(Vec2::new(640.0, 360.0), 1.0).expect("default camera center is valid");
        runtime.set_camera_2d(game_camera);
        runtime.set_render_ui_overlay(true);
        runtime.set_ui_reference([VIEW_W as f32, VIEW_H as f32]);
        println!("sceneview_mode camera=(640,360) reference=1280x720");
    }

    for _ in 0..4 {
        runtime.vertices(VIEW_W, VIEW_H)?;
        runtime.run_script_steps(1, DT, &InputSnapshot::default())?;
    }
    if prefab_flag.is_none() {
        runtime.vertices(VIEW_W, VIEW_H)?;
        runtime.run_script_steps(3, DT, &InputSnapshot::default())?;
    }

    // 离屏渲染：wgpu device + Renderer + 目标纹理 + 读回。
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .map_err(|error| format!("no GPU adapter: {error}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("probe-title-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| format!("no GPU device: {error}"))?;
    let device_for_target = device.clone();
    let mut renderer =
        Renderer::new_offscreen(device, queue, Some(project.to_path_buf()), false, false)?;
    let target = device_for_target.create_texture(&wgpu::TextureDescriptor {
        label: Some("probe-title-target"),
        size: wgpu::Extent3d {
            width: VIEW_W,
            height: VIEW_H,
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

    let mesh = runtime.vertices(VIEW_W, VIEW_H)?;
    let viewport = Viewport::new(VIEW_W as f32, VIEW_H as f32).unwrap_or(Viewport {
        width: 1.0,
        height: 1.0,
    });
    let scene3d = runtime.scene_3d(viewport);
    renderer.render_into_texture(&mesh, scene3d.as_ref(), &view, VIEW_W, VIEW_H);

    // 轮询读回（非阻塞接口，多试几次）。
    let mut pixels = None;
    for _ in 0..30 {
        match renderer.readback_frame(&target, VIEW_W, VIEW_H) {
            rustlua_gpu::ReadbackStatus::Ready { pixels: got, .. } => {
                pixels = Some(got);
                break;
            }
            rustlua_gpu::ReadbackStatus::Failed(error) => {
                return Err(format!("readback failed: {error}").into());
            }
            rustlua_gpu::ReadbackStatus::Pending => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    let pixels = pixels.ok_or("readback never ready")?;

    // 统计：四个角的像素 + 中心列抽样，判断背景是否夜空（非纯黑）。
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = (y as usize * VIEW_W as usize + x as usize) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    let sample = |x: u32, y: u32| {
        let p = at(x, y);
        println!("  px({x},{y}) = {},{},{},{}", p[0], p[1], p[2], p[3]);
    };
    println!("像素抽样（夜空背景应为非纯黑、有蓝调）：");
    sample(100, 100);
    sample(640, 60);
    sample(640, 360);
    sample(1000, 600);
    // 非全黑判定：取中心区若干点看有没有 >40 的亮度。
    let mut lit = 0;
    for y in (200..520).step_by(40) {
        for x in (200..1080).step_by(80) {
            let p = at(x, y);
            if p[0].max(p[1]).max(p[2]) > 40 {
                lit += 1;
            }
        }
    }
    let grid = (320 / 40) * (880 / 80);
    println!("中心区亮点数（>40）：{lit}/{grid}");

    image::save_buffer(
        "C:\\GitHub\\RustLua\\engine\\title_offscreen.png",
        &pixels,
        VIEW_W,
        VIEW_H,
        image::ColorType::Rgba8,
    )?;
    println!("saved title_offscreen.png");
    Ok(())
}
