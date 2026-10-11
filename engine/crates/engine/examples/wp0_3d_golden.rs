//! WP0 semantic 3D golden: scene contract plus tolerant offscreen pixel evidence.

use std::{env, fs, path::PathBuf};

use engine::host::{RenderOutcome, Renderer, RuntimeScene};
use rustlua_infra::games_dir;
use rustlua_scene::scene::Scene;
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let scene_path = root.join("scenes/demo3d.scene");
    let scene = Scene::load(&scene_path).expect("load demo3d scene");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(&root), scene).expect("instantiate demo3d");
    let viewport =
        rustlua_camera::camera::Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("3D scene");
    let names = extracted
        .drawables
        .iter()
        .map(|drawable| drawable.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 3, "demo3d drawable count changed");
    for required in ["Ground", "ShadowTriangle", "SkeletonWarrior"] {
        assert!(
            names.iter().any(|name| name == required),
            "missing {required}"
        );
    }
    assert!((extracted.camera.position.z - 6.0).abs() < 1e-5);
    assert!((extracted.camera.target.x).abs() < 1e-5);
    assert!((extracted.camera.target.y).abs() < 1e-5);
    assert!((extracted.camera.target.z).abs() < 1e-5);

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the 3D golden");
    let info = adapter.get_info();
    let timestamp_features =
        wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let required_features = if adapter.features().contains(timestamp_features) {
        timestamp_features
    } else {
        wgpu::Features::empty()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-3d-golden"),
        required_features,
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
        label: Some("wp0-3d-golden-target"),
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
    let mesh = runtime.vertices(WIDTH, HEIGHT).expect("2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback");
    let metrics = renderer.last_frame_metrics();
    assert!(metrics.draw_calls >= 4, "shadow/main passes disappeared");
    assert!(metrics.instances >= 3, "3D instances disappeared");
    assert!(metrics.triangles >= 3, "3D triangles disappeared");

    let background = [pixels[0], pixels[1], pixels[2]];
    let mut changed = 0_u64;
    let mut center_changed = 0_u64;
    let mut min_x = WIDTH;
    let mut min_y = HEIGHT;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let index = ((y * WIDTH + x) * 4) as usize;
            let difference = (i16::from(pixels[index]) - i16::from(background[0])).unsigned_abs()
                + (i16::from(pixels[index + 1]) - i16::from(background[1])).unsigned_abs()
                + (i16::from(pixels[index + 2]) - i16::from(background[2])).unsigned_abs();
            if difference > 24 {
                changed += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                if (WIDTH / 4..WIDTH * 3 / 4).contains(&x)
                    && (HEIGHT / 4..HEIGHT * 3 / 4).contains(&y)
                {
                    center_changed += 1;
                }
            }
        }
    }
    assert!(
        changed > 1_000,
        "3D frame has too few non-background pixels: {changed}"
    );
    assert!(
        center_changed > 100,
        "3D content disappeared from the center region"
    );
    assert!(min_x < max_x && min_y < max_y, "3D pixel bounds collapsed");

    let report = json!({
        "report_version": "wp0-golden-v1",
        "scene": "demo3d",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "drawable_names": names,
            "camera_position": [extracted.camera.position.x, extracted.camera.position.y, extracted.camera.position.z],
            "camera_target": [extracted.camera.target.x, extracted.camera.target.y, extracted.camera.target.z],
            "draw_calls": metrics.draw_calls,
            "instances": metrics.instances,
            "triangles": metrics.triangles,
            "non_background_pixels": changed,
            "center_non_background_pixels": center_changed,
            "content_bounds": [min_x, min_y, max_x, max_y]
        },
        "gpu_timestamp_supported": renderer.gpu_timestamp_supported()
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
                println!("Usage: wp0_3d_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
