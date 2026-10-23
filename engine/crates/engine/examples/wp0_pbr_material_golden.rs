//! P3-R02-2：`.material` 覆盖 + glTF ORM/emissive 的**像素门**（`wp0-pbr-material-golden-v1`）。
//!
//! 门钉住的观测：
//!
//! 1. **覆盖生效**：同一块地面挂 `.material` 后，像素必须明显不同于内嵌缺省材质。
//! 2. **ORM 真的被采样**：`p3-r02-orm`（左半粗糙电介质 / 右半粗糙金属）与字段完全相同的
//!    `p3-r02-flat`（无贴图）对比。同一相机、同一点上的视线项相同，差异只可能来自
//!    metallic-roughness 贴图。
//! 3. **自发光被加上**：`p3-r02-emissive` 与只差 emissive 的 `p3-r02-emissive-off` 对比。
//! 4. **失败回退不换画面**：语法错误的 `.material` 与不存在的文件都必须回退到内嵌材质，
//!    像素与"不挂覆盖"逐字节一致（同时渲染器已留下告警）。那份坏材质叫
//!    `assets/materials/_p3-r02-invalid.material`——`_` 前缀是"开发期文件"约定
//!    （`assetc::is_dev_artifact`），它不进扫描/导入/发行包，因此**故意的坏资产**不会被
//!    `MaterialImporter` 判成示例项目导入失败，而这条门照样读得到它。
//! 5. **旧路径不受影响**：`pbr=off` 时 emissive 开/关必须逐字节一致——自发光只属于 PBR
//!    着色器，旧管线一个字节都没改。

use std::{env, fs, path::PathBuf};

use engine::host::{RenderOutcome, Renderer, RuntimeScene};
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_math::math3d::Vec3;
use rustlua_render::picking::screen_from_world;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const LIGHT: Vec3 = Vec3::new(0.8, -0.35, 0.2);
const CAMERA: Vec3 = Vec3::new(0.0, 3.0, 6.0);

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the material golden");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-pbr-material-golden"),
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
        label: Some("wp0-pbr-material-golden-target"),
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
    let samples = sample_points();

    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready(), "PBR 变体没有编译");

    let default = render_scene(&mut renderer, &root, &texture, &view, None, &samples);
    let orm = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-orm.material"),
        &samples,
    );
    let flat = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-flat.material"),
        &samples,
    );
    let emissive = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-emissive.material"),
        &samples,
    );
    let emissive_off = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-emissive-off.material"),
        &samples,
    );
    let invalid = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/_p3-r02-invalid.material"),
        &samples,
    );
    let missing = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-does-not-exist.material"),
        &samples,
    );

    // 旧路径的负向门：自发光不得漏进 `pbr=off` 的着色。
    renderer.set_render_settings(RenderSettings::default());
    let legacy_emissive = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-emissive.material"),
        &samples,
    );
    let legacy_emissive_off = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Some("assets/materials/p3-r02-emissive-off.material"),
        &samples,
    );

    let (override_delta, override_at) = max_channel_delta(&orm, &default);
    let (texture_delta, texture_at) = max_channel_delta(&orm, &flat);
    let (emissive_delta, emissive_at) = max_channel_delta(&emissive, &emissive_off);
    let (invalid_delta, invalid_at) = max_channel_delta(&invalid, &default);
    let (missing_delta, missing_at) = max_channel_delta(&missing, &default);
    let (legacy_emissive_delta, legacy_emissive_at) =
        max_channel_delta(&legacy_emissive, &legacy_emissive_off);

    assert!(
        override_delta >= 8,
        "挂上 .material 后画面几乎没变（最大差 {override_delta}，点 {override_at}）：覆盖没有生效"
    );
    assert!(
        texture_delta >= 8,
        "ORM 与同字段无贴图材质只差 {texture_delta}（点 {texture_at}）：metallic-roughness 贴图没有被采样"
    );
    assert!(
        emissive_delta >= 20,
        "自发光开/关只差 {emissive_delta}（点 {emissive_at}）：emissive 没有进输出"
    );
    assert_eq!(
        (invalid_delta, invalid_at),
        (0, 0),
        "非法 .material 必须回退到内嵌材质；实测差 {invalid_delta}（点 {invalid_at}）"
    );
    assert_eq!(
        (missing_delta, missing_at),
        (0, 0),
        "缺失 .material 必须回退到内嵌材质；实测差 {missing_delta}（点 {missing_at}）"
    );
    assert_eq!(
        (legacy_emissive_delta, legacy_emissive_at),
        (0, 0),
        "pbr=off 时自发光不应影响像素；实测差 {legacy_emissive_delta}（点 {legacy_emissive_at}）"
    );

    println!(
        "wp0_pbr_material_samples override={override_delta}@{override_at} \
         orm_vs_flat={texture_delta}@{texture_at} emissive={emissive_delta}@{emissive_at} \
         invalid_fallback={invalid_delta} missing_fallback={missing_delta} \
         legacy_emissive={legacy_emissive_delta}"
    );

    let report = json!({
        "report_version": "wp0-pbr-material-golden-v1",
        "scene": "ground plane + .material overrides (built in memory)",
        "width": WIDTH,
        "height": HEIGHT,
        "light_direction": [LIGHT.x, LIGHT.y, LIGHT.z],
        "camera": [CAMERA.x, CAMERA.y, CAMERA.z],
        "sample_points": samples
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect::<Vec<_>>(),
        "default": default,
        "orm_override": orm,
        "flat_override": flat,
        "emissive": emissive,
        "emissive_off": emissive_off,
        "max_channel_delta": {
            "override_vs_default": override_delta,
            "orm_vs_flat": texture_delta,
            "emissive_on_vs_off": emissive_delta,
            "invalid_fallback": invalid_delta,
            "missing_fallback": missing_delta,
            "legacy_emissive_on_vs_off": legacy_emissive_delta,
        },
        "checks": {
            "material_override_changes_shading": override_delta >= 8,
            "orm_texture_is_sampled": texture_delta >= 8,
            "emissive_is_added": emissive_delta >= 20,
            "invalid_material_falls_back": invalid_delta == 0,
            "missing_material_falls_back": missing_delta == 0,
            "legacy_shader_ignores_emissive": legacy_emissive_delta == 0,
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
    println!("wp0_pbr_material_golden_ok");
}

fn sample_points() -> Vec<Vec3> {
    let mut points = Vec::new();
    for x in [-2.0_f32, -1.0, 1.0, 2.0] {
        for z in [0.5_f32, 1.5] {
            points.push(Vec3::new(x, 0.0, z));
        }
    }
    points
}

fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    material: Option<&str>,
    local_points: &[Vec3],
) -> Vec<[u8; 3]> {
    let mut scene = Scene::new("pbr-material-probe");
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
    let mesh_renderer = match material {
        Some(path) => format!("mesh=__ground_plane__;material={path}"),
        None => "mesh=__ground_plane__".to_owned(),
    };
    scene
        .entities
        .get_mut(&ground)
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
                println!("Usage: wp0_pbr_material_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
