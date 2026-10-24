//! P3-R02-3：法线/遮蔽贴图与切线基的**像素门**（`wp0-pbr-normal-golden-v1`）。
//!
//! 门钉住的观测：
//!
//! 1. **法线贴图被采样**：同一 UV 完整的四边形，`p3-r02-normal` 与只差
//!    `normal_texture` 的 `p3-r02-normal-off` 必须产生明显差异。
//! 2. **切线基真的参与**：`normal_scale=0` 把切线空间扰动压平，结果必须与
//!    "无法线贴图"基本一致（≤1 个色阶），证明扰动来自 TBN 而不是别的路径。
//! 3. **退化 UV 回退**：常量 UV 的四边形无法生成切线，法线贴图开/关必须逐字节一致；
//!    该四边形的常量 UV 正好采样到法线贴图右半（倾斜法线），若没有回退会明显变暗。
//! 4. **遮蔽贴图只压间接光**：`p3-r02-occlusion` 与只差遮蔽贴图/强度的材质对比，
//!    暗半侧必须变暗。
//! 5. **旧路径不受影响**：`pbr=off` 时法线贴图开/关逐字节一致。

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
const LIGHT: Vec3 = Vec3::new(0.8, -0.35, 0.2);
const CAMERA: Vec3 = Vec3::new(0.0, 3.5, 6.5);
const QUAD: &str = "assets/models/p3-r02-quad.gltf";
const DEGENERATE: &str = "assets/models/p3-r02-quad-degenerate.gltf";

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new(
        "wp0-pbr-normal-golden",
        WIDTH,
        HEIGHT,
    ))
    .expect("offscreen fixture for wp0-pbr-normal-golden");
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

    let normal = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-normal.material"),
        &samples,
    );
    let normal_off = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-normal-off.material"),
        &samples,
    );
    let normal_scale_zero = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-normal-scale-zero.material"),
        &samples,
    );
    let degenerate_normal = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        DEGENERATE,
        Some("assets/materials/p3-r02-normal.material"),
        &samples,
    );
    let degenerate_off = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        DEGENERATE,
        Some("assets/materials/p3-r02-normal-off.material"),
        &samples,
    );
    let occlusion = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-occlusion.material"),
        &samples,
    );
    let occlusion_off = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-occlusion-off.material"),
        &samples,
    );

    // 旧路径的负向门：法线贴图不得漏进 `pbr=off` 的着色。
    renderer.set_render_settings(RenderSettings::default());
    let legacy_normal = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-normal.material"),
        &samples,
    );
    let legacy_normal_off = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        QUAD,
        Some("assets/materials/p3-r02-normal-off.material"),
        &samples,
    );

    let (normal_delta, normal_at) = max_channel_delta(&normal, &normal_off);
    let (scale_zero_delta, scale_zero_at) = max_channel_delta(&normal_scale_zero, &normal_off);
    let (degenerate_delta, degenerate_at) = max_channel_delta(&degenerate_normal, &degenerate_off);
    let (occlusion_delta, occlusion_at) = max_channel_delta(&occlusion, &occlusion_off);
    let (legacy_delta, legacy_at) = max_channel_delta(&legacy_normal, &legacy_normal_off);
    // 无贴图时两个四边形（只差 UV）必须一致，证明退化场景本身正常渲染。
    let (baseline_delta, baseline_at) = max_channel_delta(&degenerate_off, &normal_off);

    assert!(
        normal_delta >= 8,
        "法线贴图开/关只差 {normal_delta}（点 {normal_at}）：法线贴图没有被采样"
    );
    assert!(
        scale_zero_delta <= 1,
        "normal_scale=0 与无法线贴图差 {scale_zero_delta}（点 {scale_zero_at}）：缩放轴没有生效"
    );
    assert_eq!(
        (degenerate_delta, degenerate_at),
        (0, 0),
        "退化 UV 必须回退到几何法线；实测差 {degenerate_delta}（点 {degenerate_at}）"
    );
    assert!(
        occlusion_delta >= 8,
        "遮蔽贴图开/关只差 {occlusion_delta}（点 {occlusion_at}）：遮蔽没有被采样"
    );
    assert_eq!(
        (legacy_delta, legacy_at),
        (0, 0),
        "pbr=off 时法线贴图不应影响像素；实测差 {legacy_delta}（点 {legacy_at}）"
    );
    assert_eq!(
        (baseline_delta, baseline_at),
        (0, 0),
        "无法线贴图时两个四边形应一致；实测差 {baseline_delta}（点 {baseline_at}）"
    );

    println!(
        "wp0_pbr_normal_samples normal={normal_delta}@{normal_at} \
         scale_zero={scale_zero_delta}@{scale_zero_at} degenerate={degenerate_delta}@{degenerate_at} \
         occlusion={occlusion_delta}@{occlusion_at} legacy_normal={legacy_delta} \
         baseline={baseline_delta}"
    );

    let report = json!({
        "report_version": "wp0-pbr-normal-golden-v1",
        "scene": "UV quad + normal/occlusion .material overrides",
        "width": WIDTH,
        "height": HEIGHT,
        "light_direction": [LIGHT.x, LIGHT.y, LIGHT.z],
        "camera": [CAMERA.x, CAMERA.y, CAMERA.z],
        "sample_points": samples
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect::<Vec<_>>(),
        "normal": normal,
        "normal_off": normal_off,
        "normal_scale_zero": normal_scale_zero,
        "degenerate_normal": degenerate_normal,
        "degenerate_off": degenerate_off,
        "occlusion": occlusion,
        "occlusion_off": occlusion_off,
        "max_channel_delta": {
            "normal_on_vs_off": normal_delta,
            "normal_scale_zero_vs_off": scale_zero_delta,
            "degenerate_normal_vs_off": degenerate_delta,
            "occlusion_on_vs_off": occlusion_delta,
            "legacy_normal_on_vs_off": legacy_delta,
            "no_map_uv_baseline": baseline_delta,
        },
        "checks": {
            "normal_map_is_sampled": normal_delta >= 8,
            "normal_scale_is_applied": scale_zero_delta <= 1,
            "degenerate_tangents_fall_back": degenerate_delta == 0,
            "occlusion_is_sampled": occlusion_delta >= 8,
            "legacy_shader_ignores_normal_map": legacy_delta == 0,
            "uv_only_baseline_matches": baseline_delta == 0,
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
    println!("wp0_pbr_normal_golden_ok");
}

fn sample_points() -> Vec<Vec3> {
    let mut points = Vec::new();
    for x in [-1.5_f32, 1.5] {
        for z in [-0.5_f32, 0.5] {
            points.push(Vec3::new(x, 0.0, z));
        }
    }
    points
}

#[allow(clippy::too_many_arguments)]
fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    mesh_path: &str,
    material: Option<&str>,
    local_points: &[Vec3],
) -> Vec<[u8; 3]> {
    let mut scene = Scene::new("pbr-normal-probe");
    let light = SceneEntityId(1);
    let quad = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (quad, "Quad"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        format!(
            "direction={},{},{};color=1,1,1;intensity=1",
            LIGHT.x, LIGHT.y, LIGHT.z
        ),
    );
    scene.entities.get_mut(&quad).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    let mesh_renderer = match material {
        Some(path) => format!("mesh={mesh_path};material={path}"),
        None => format!("mesh={mesh_path}"),
    };
    scene
        .entities
        .get_mut(&quad)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), mesh_renderer);
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        format!(
            "projection=perspective;position={},{},{};target=0,0,0;fov=60;near=0.1;far=100",
            CAMERA.x, CAMERA.y, CAMERA.z
        ),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("normal runtime");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("normal 3D extraction");
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("normal 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("normal readback");
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

fn max_channel_delta(left: &[[u8; 3]], right: &[[u8; 3]]) -> (u32, usize) {
    assert_eq!(left.len(), right.len());
    let mut best = (0, 0);
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        for channel in 0..3 {
            let delta = u32::from(left[channel].abs_diff(right[channel]));
            if delta > best.0 {
                best = (delta, index);
            }
        }
    }
    best
}

fn sample_pixel(pixels: &[u8], point: rustlua_ecs::transform::Vec2) -> [u8; 3] {
    let x = (point.x.round().max(0.0) as u32).min(WIDTH - 1);
    let y = (point.y.round().max(0.0) as u32).min(HEIGHT - 1);
    let index = ((y * WIDTH + x) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
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
                println!("Usage: wp0_pbr_normal_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
