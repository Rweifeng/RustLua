//! P3-R02-1：PBR 的**像素门**（报告 `wp0-pbr-golden-v1`）。
//!
//! 这条门不靠"看起来像 PBR"，而是钉住三个只有修好法线/视线才会成立的观测：
//!
//! 1. **刚体旋转不变**：地面、方向光、相机一起绕 Z 轴转 60°（光相对模型不变）。
//!    PBR 顶点阶段用 CPU 传上来的逆转置法线矩阵 ⇒ 对应世界点的颜色不变；
//!    旧着色器把**本地法线**直接当世界法线用 ⇒ 光转了、法线没转，颜色必须明显漂移。
//!    这里同时断言 PBR 的漂移在容差内、旧路径的漂移显著更大（负向门）。
//! 2. **高光随相机**：模型与光不动，把相机绕物体转到四个方位、外加一个低位机位。
//!    旧着色器的高光视线写死 `(0,0,1)`，所以它的像素**完全不变**；PBR 的
//!    `V = normalize(camera.position - world)` 会让镜面项变化。断言 PBR 至少有一个
//!    采样点变化、旧路径变化为 0。
//! 3. **开关真的换了着色**：同一帧、同一场景，`pbr=off` 与 `pbr=on` 的读回必须不同。
//!
//! 采样点是**世界点投影**而不是固定像素：旋转后光栅化覆盖会动，固定像素会把"边缘采样到
//! 背景"误判成着色漂移。地面用内建 `__ground_plane__`（无外部资产），材质为仓库默认的
//! 非金属/粗糙 0.9。0.9 的 GGX 波瓣很宽，所以方向光特意压平（水平分量占主导）：
//! 相机绕点时半程向量才会真正扫过波瓣，本机实测最大差 8/255（旧路径 0）。

use std::{env, fs, path::PathBuf};

use engine::host::{RenderOutcome, Renderer, RuntimeScene};
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_math::math3d::{Quat, Vec3};
use rustlua_render::picking::screen_from_world;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// 光传播方向（表面→光源取反）。刻意让水平分量占主导：这样相机绕点转时半程向量
/// `H = normalize(L + V)` 会真正扫过 GGX 波瓣；近似垂直的光会让 H 永远贴着 +Y，
/// 相机怎么转高光都几乎不变（本轮实测过低角度不敏感，所以探针把光压平）。
const BASE_LIGHT: Vec3 = Vec3::new(0.8, -0.35, 0.2);
/// 基准相机（地面上方 3，前方 6）。
const BASE_CAMERA: Vec3 = Vec3::new(0.0, 3.0, 6.0);
/// 相机绕点：四个方位 + 一个低位。旧着色器的高光视线写死 (0,0,1)，这五个机位的像素
/// 必须**完全一致**；PBR 的镜面项随 `V` 变化，任何一处出现差异都说明视线接上了。
const ORBIT_CAMERAS: [Vec3; 5] = [
    Vec3::new(6.0, 3.0, 0.0),
    Vec3::new(0.0, 3.0, 6.0),
    Vec3::new(-6.0, 3.0, 0.0),
    Vec3::new(0.0, 3.0, -6.0),
    Vec3::new(0.0, 1.2, 6.0),
];
/// 刚体旋转角：绕 Z 转 60°，旧法线（恒 +Y）与光的夹角会从 ≈0.93 掉到 ≈0.22。
const ROTATION_DEGREES: f32 = 60.0;
/// 单通道差异容差（u8）。旋转后光栅化不变，理论上应当接近 0；留 2 级吸收定点舍入。
const ROTATION_TOLERANCE: u32 = 2;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the PBR golden");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-pbr-golden"),
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
        label: Some("wp0-pbr-golden-target"),
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

    // ---------- 旧路径（pbr=off）：基准、刚体旋转、相机绕点 ----------
    renderer.set_render_settings(RenderSettings::default());
    let legacy_base = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Quat::identity(),
        BASE_CAMERA,
        &samples,
    );
    let legacy_rotated = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        rotation_z(),
        BASE_CAMERA,
        &samples,
    );
    let legacy_orbits: Vec<Vec<[u8; 3]>> = ORBIT_CAMERAS
        .iter()
        .map(|camera| {
            render_scene(
                &mut renderer,
                &root,
                &texture,
                &view,
                Quat::identity(),
                *camera,
                &samples,
            )
        })
        .collect();

    // ---------- PBR（pbr=on）：同三组观测 ----------
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready(), "PBR 变体没有编译");
    let pbr_base = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        Quat::identity(),
        BASE_CAMERA,
        &samples,
    );
    let pbr_rotated = render_scene(
        &mut renderer,
        &root,
        &texture,
        &view,
        rotation_z(),
        BASE_CAMERA,
        &samples,
    );
    let pbr_orbits: Vec<Vec<[u8; 3]>> = ORBIT_CAMERAS
        .iter()
        .map(|camera| {
            render_scene(
                &mut renderer,
                &root,
                &texture,
                &view,
                Quat::identity(),
                *camera,
                &samples,
            )
        })
        .collect();

    let (switch_delta, switch_at) = max_channel_delta(&pbr_base, &legacy_base);
    let (pbr_rotation_delta, pbr_rotation_at) = max_channel_delta(&pbr_rotated, &pbr_base);
    let (legacy_rotation_delta, legacy_rotation_at) =
        max_channel_delta(&legacy_rotated, &legacy_base);
    let orbit_delta = |frames: &[Vec<[u8; 3]>], base: &[[u8; 3]]| -> (u32, usize) {
        frames
            .iter()
            .map(|frame| max_channel_delta(frame, base))
            .max()
            .unwrap_or((0, 0))
    };
    let (pbr_orbit_delta, pbr_orbit_at) = orbit_delta(&pbr_orbits, &pbr_base);
    let (legacy_orbit_delta, legacy_orbit_at) = orbit_delta(&legacy_orbits, &legacy_base);

    assert!(
        switch_delta >= 4,
        "打开 PBR 后画面没有变化（最大差 {switch_delta}）：开关没有换着色路径"
    );
    assert!(
        pbr_rotation_delta <= ROTATION_TOLERANCE,
        "PBR 在刚体旋转后漂移 {pbr_rotation_delta}（点 {pbr_rotation_at}）\
         —— 法线矩阵没有把法线旋到世界空间"
    );
    assert!(
        legacy_rotation_delta >= 10,
        "旧着色器在刚体旋转后只漂移 {legacy_rotation_delta}（点 {legacy_rotation_at}）：\
         探针没有区分力，无法证明法线 bug 被修复"
    );
    assert!(
        pbr_rotation_delta * 4 < legacy_rotation_delta,
        "PBR 漂移 {pbr_rotation_delta} 没有显著低于旧路径 {legacy_rotation_delta}"
    );
    assert!(
        pbr_orbit_delta >= 2,
        "相机移动后 PBR 高光没有变化（最大差 {pbr_orbit_delta}，点 {pbr_orbit_at}）：\
         视线仍被写死"
    );
    assert_eq!(
        legacy_orbit_delta, 0,
        "旧着色器的高光视线写死 (0,0,1)，相机移动本应零变化；实测 {legacy_orbit_delta}（点 {legacy_orbit_at}）"
    );

    println!(
        "wp0_pbr_samples switch={switch_delta}@{switch_at} pbr_rotate={pbr_rotation_delta}@{pbr_rotation_at} \
         legacy_rotate={legacy_rotation_delta}@{legacy_rotation_at} pbr_orbit={pbr_orbit_delta}@{pbr_orbit_at} \
         legacy_orbit={legacy_orbit_delta}@{legacy_orbit_at}"
    );

    let report = json!({
        "report_version": "wp0-pbr-golden-v1",
        "scene": "pbr-probe (built in memory)",
        "width": WIDTH,
        "height": HEIGHT,
        "light_direction": [BASE_LIGHT.x, BASE_LIGHT.y, BASE_LIGHT.z],
        "camera_base": [BASE_CAMERA.x, BASE_CAMERA.y, BASE_CAMERA.z],
        "orbit_cameras": ORBIT_CAMERAS
            .iter()
            .map(|camera| [camera.x, camera.y, camera.z])
            .collect::<Vec<_>>(),
        "rotation_degrees_z": ROTATION_DEGREES,
        "sample_points": samples
            .iter()
            .map(|point| [point.x, point.y, point.z])
            .collect::<Vec<_>>(),
        "legacy_base": legacy_base,
        "pbr_base": pbr_base,
        "legacy_rotated": legacy_rotated,
        "pbr_rotated": pbr_rotated,
        "legacy_orbit_frames": legacy_orbits,
        "pbr_orbit_frames": pbr_orbits,
        "max_channel_delta": {
            "pbr_vs_legacy": switch_delta,
            "pbr_rotation": pbr_rotation_delta,
            "legacy_rotation": legacy_rotation_delta,
            "pbr_camera_orbit": pbr_orbit_delta,
            "legacy_camera_orbit": legacy_orbit_delta,
        },
        "checks": {
            "pbr_switch_changes_shading": switch_delta >= 4,
            "pbr_rotation_invariant": pbr_rotation_delta <= ROTATION_TOLERANCE,
            "legacy_rotation_drifts": legacy_rotation_delta >= 10,
            "pbr_specular_follows_camera": pbr_orbit_delta >= 2,
            "legacy_specular_is_camera_blind": legacy_orbit_delta == 0,
        },
        "draw_calls": renderer.last_frame_metrics().draw_calls,
        "rotation_tolerance": ROTATION_TOLERANCE,
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
    println!("wp0_pbr_golden_ok");
}

fn rotation_z() -> Quat {
    Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), ROTATION_DEGREES.to_radians())
        .expect("unit Z axis")
}

/// 地面局部坐标里的采样点：避开原点足够远，旋转前后都落在地面内部。
fn sample_points() -> Vec<Vec3> {
    let mut points = Vec::new();
    for x in [-2.0_f32, -1.0, 0.0, 1.0, 2.0] {
        for z in [-2.0_f32, 0.0, 2.0] {
            points.push(Vec3::new(x, 0.0, z));
        }
    }
    points
}

/// 渲染一次：地面 + 方向光 + 相机。`rotation` 同时作用于地面、光与相机（刚体变换）。
fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    rotation: Quat,
    camera_position: Vec3,
    local_points: &[Vec3],
) -> Vec<[u8; 3]> {
    let mut scene = Scene::new("pbr-probe");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (ground, "Ground"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    let light_direction = rotation.rotate(BASE_LIGHT).expect("rotate light");
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        format!(
            "direction={},{},{};color=1,1,1;intensity=1",
            light_direction.x, light_direction.y, light_direction.z
        ),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        format!(
            "position=0,0,0;rotation={},{},{},{};scale=1,1,1",
            rotation.x, rotation.y, rotation.z, rotation.w
        ),
    );
    scene
        .entities
        .get_mut(&ground)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    let camera_world = rotation.rotate(camera_position).expect("rotate camera");
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        format!(
            "projection=perspective;position={},{},{};target=0,0,0;fov=60;near=0.1;far=100",
            camera_world.x, camera_world.y, camera_world.z
        ),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("pbr runtime");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("PBR 3D extraction");
    let mesh = runtime.vertices(WIDTH, HEIGHT).expect("PBR 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("PBR readback");
    local_points
        .iter()
        .map(|local| {
            let world = rotation.rotate(*local).expect("rotate sample");
            let point = screen_from_world(extracted.camera, world, viewport)
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
                println!("Usage: wp0_pbr_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
