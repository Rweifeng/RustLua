//! P3-E02-4：材质四组场景级 golden（`wp0-material-golden-v1`）。
//!
//! 同一块地面、同一相机/灯光，只换 `.material` 的 metallic/roughness：
//! `0/0.1`、`0/0.9`、`1/0.1`、`1/0.9`。门钉住的观测：
//!
//! 1. **四组两两不同**：任意两组在采样点上的最大通道差都不为 0；
//! 2. **金属度消除漫反射**：`metallic=0` 的平均亮度高于同粗糙度的 `metallic=1`；
//! 3. **粗糙度改变高光分布**：同金属度下 `0.1` 的峰值亮度高于 `0.9`
//!    （高光更集中，镜面采样点出现更亮的像素）；
//! 4. 报告每组 `material_path` 与逐点像素，供人工复核/基线对比。
//!
//! 灯光方向取 `(0,-0.6,0.8)`：相机固定在 `(0,3,6)` 时，它关于地面法线的镜像
//! 反射点正好是 `(0,0,2)`——采样网格必须包含该点，否则 0.1 粗糙度的窄高光
//! 会整个落在采样点之间，只剩漫反射，粗糙度看似没生效。

use std::{env, fs, path::PathBuf};

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_math::math3d::Vec3;
use rustlua_render::picking::screen_from_world;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const LIGHT: Vec3 = Vec3::new(0.0, -0.6, 0.8);
const CAMERA: Vec3 = Vec3::new(0.0, 3.0, 6.0);

const GROUPS: [(&str, f32, f32); 4] = [
    ("assets/materials/p3-e02-m0-r01.material", 0.0, 0.1),
    ("assets/materials/p3-e02-m0-r09.material", 0.0, 0.9),
    ("assets/materials/p3-e02-m1-r01.material", 1.0, 0.1),
    ("assets/materials/p3-e02-m1-r09.material", 1.0, 0.9),
];

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let fixture = match OffscreenTarget::acquire(&OffscreenRequest::new(
        "wp0-material-golden",
        WIDTH,
        HEIGHT,
    )) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("material_golden_skipped reason=no_adapter error={error} exit=3");
            std::process::exit(3);
        }
    };
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let texture = fixture.texture();
    let view = fixture.view();
    let samples = sample_points();

    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready(), "PBR 变体没有编译");

    let mut results = Vec::new();
    for (path, metallic, roughness) in GROUPS {
        let pixels = render_scene(&mut renderer, &root, texture, view, path, &samples);
        let luma: Vec<f32> = pixels.iter().map(|pixel| luma(*pixel)).collect();
        let mean = luma.iter().sum::<f32>() / luma.len() as f32;
        let max = luma.iter().copied().fold(0.0_f32, f32::max);
        results.push((path, metallic, roughness, pixels, mean, max));
    }

    let mut min_pair_delta = u32::MAX;
    for left in 0..results.len() {
        for right in (left + 1)..results.len() {
            let delta = max_channel_delta(&results[left].3, &results[right].3);
            min_pair_delta = min_pair_delta.min(delta);
        }
    }
    let dielectric_rough = results[1].4;
    let metal_rough = results[3].4;
    let dielectric_smooth_peak = results[0].5;
    let dielectric_rough_peak = results[1].5;
    let metal_smooth_peak = results[2].5;
    let metal_rough_peak = results[3].5;

    for (path, metallic, roughness, _, mean, max) in &results {
        println!(
            "group {path} metallic={metallic} roughness={roughness} mean={mean:.2} max={max:.2}"
        );
    }
    assert!(
        min_pair_delta >= 4,
        "四组材质两两差异太小（最小最大通道差 {min_pair_delta}）：覆盖或参数没有生效"
    );
    assert!(
        dielectric_rough > metal_rough,
        "metallic=0 的平均亮度应高于 metallic=1（漫反射 vs 金属）：{dielectric_rough:.2} vs {metal_rough:.2}"
    );
    assert!(
        dielectric_smooth_peak > dielectric_rough_peak,
        "粗糙度 0.1 的峰值亮度应高于 0.9（高光更集中）：{dielectric_smooth_peak:.2} vs {dielectric_rough_peak:.2}"
    );
    assert!(
        metal_smooth_peak > metal_rough_peak,
        "金属材质粗糙度 0.1 的峰值亮度应高于 0.9：{metal_smooth_peak:.2} vs {metal_rough_peak:.2}"
    );

    println!(
        "wp0_material_samples min_pair_delta={min_pair_delta} \
         dielectric_mean={dielectric_rough:.2} metal_mean={metal_rough:.2} \
         dielectric_peak={dielectric_smooth_peak:.2}/{dielectric_rough_peak:.2} \
         metal_peak={metal_smooth_peak:.2}/{metal_rough_peak:.2}"
    );

    let groups: Vec<_> = results
        .iter()
        .map(|(path, metallic, roughness, pixels, mean, max)| {
            json!({
                "material_path": path,
                "metallic": metallic,
                "roughness": roughness,
                "mean_luma": mean,
                "max_luma": max,
                "samples": pixels,
            })
        })
        .collect();
    let report = json!({
        "report_version": "wp0-material-golden-v1",
        "scene": "ground plane + four .material groups (built in memory)",
        "width": WIDTH,
        "height": HEIGHT,
        "light_direction": [LIGHT.x, LIGHT.y, LIGHT.z],
        "camera": [CAMERA.x, CAMERA.y, CAMERA.z],
        "sample_points": samples
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect::<Vec<_>>(),
        "groups": groups,
        "min_pair_delta": min_pair_delta,
        "checks": {
            "groups_are_distinct": min_pair_delta >= 4,
            "metal_removes_diffuse": dielectric_rough > metal_rough,
            "smooth_highlight_peaks_dielectric": dielectric_smooth_peak > dielectric_rough_peak,
            "smooth_highlight_peaks_metal": metal_smooth_peak > metal_rough_peak,
        },
        "draw_calls": renderer.last_frame_metrics().draw_calls,
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).expect("create output directory");
        }
        fs::write(&path, text.as_bytes()).expect("write report");
        println!("report: {}", path.display());
    }
    println!("wp0_material_golden_ok");
}

fn sample_points() -> Vec<Vec3> {
    let mut points = Vec::new();
    for x in [-2.5_f32, -1.5, -0.5, 0.5, 1.5, 2.5] {
        for z in [0.5_f32, 1.5, 2.5] {
            points.push(Vec3::new(x, 0.0, z));
        }
    }
    // 镜面反射点（见模块注释），高光峰值只在这里稳定出现。
    points.push(Vec3::new(0.0, 0.0, 2.0));
    points
}

fn luma(pixel: [u8; 3]) -> f32 {
    0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2])
}

fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    material: &str,
    local_points: &[Vec3],
) -> Vec<[u8; 3]> {
    let mut scene = Scene::new("material-edge-probe");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (ground, "Ground"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        format!(
            "direction={},{},{};color=1,1,1;intensity=1",
            LIGHT.x, LIGHT.y, LIGHT.z
        ),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "MeshRenderer".into(),
        format!("mesh=__ground_plane__;material={material}"),
    );
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        format!(
            "projection=perspective;position={},{},{};target=0,0,0;fov=60;near=0.1;far=100",
            CAMERA.x, CAMERA.y, CAMERA.z
        ),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("material runtime");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("material 3D extraction");
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("material 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("material readback");
    local_points
        .iter()
        .map(|world| {
            let point = screen_from_world(extracted.camera, *world, viewport)
                .unwrap_or_else(|| panic!("相机没有覆盖采样点 {world:?}"))
                .0;
            sample_pixel(&pixels, point)
        })
        .collect()
}

fn max_channel_delta(left: &[[u8; 3]], right: &[[u8; 3]]) -> u32 {
    assert_eq!(left.len(), right.len());
    let mut best = 0;
    for (left, right) in left.iter().zip(right) {
        for channel in 0..3 {
            best = best.max(u32::from(left[channel].abs_diff(right[channel])));
        }
    }
    best
}

fn sample_pixel(pixels: &[u8], point: rustlua_ecs::transform::Vec2) -> [u8; 3] {
    // 0.1 粗糙度的高光可能只有一个像素宽，逐通道取 5×5 邻域最大值以免错位漏采。
    let center_x = (point.x.round().max(0.0) as i64).clamp(0, i64::from(WIDTH) - 1);
    let center_y = (point.y.round().max(0.0) as i64).clamp(0, i64::from(HEIGHT) - 1);
    let mut best = [0_u8; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let x = (center_x + dx).clamp(0, i64::from(WIDTH) - 1) as u32;
            let y = (center_y + dy).clamp(0, i64::from(HEIGHT) - 1) as u32;
            let index = ((y * WIDTH + x) * 4) as usize;
            for channel in 0..3 {
                best[channel] = best[channel].max(pixels[index + channel]);
            }
        }
    }
    best
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
                println!("Usage: wp0_material_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
