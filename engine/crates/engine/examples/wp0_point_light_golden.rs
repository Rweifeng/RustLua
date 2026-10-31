//! 多点光源（`type=point`）的**像素门**（报告 `wp0-point-light-golden-v1`）。
//!
//! 为什么需要它：`Light` 组件新增了点光源后，CPU 侧的打包（`light_uniform_bytes`）、
//! 绑定尺寸、着色器里的 `array<PointLight, 8>` 与循环上限一共有四段接线，任何一段
//! 错位（偏移、layout、count 字段）都会让画面看起来"只是暗了一点"，光看有没有画出东西
//! 完全区分不出来。这里用**纯点光照明的场景**把每一段钉在像素上：
//!
//! - 方向光强度置 0 ⇒ 直射/阴影/环境项（`indirect = AMBIENT_FACTOR * radiance`）全为 0，
//!   画面里剩下的颜色只可能来自点光源；
//! - 两盏不同颜色的点光分别悬在 x=-1.5 / x=+1.5 上方 ⇒ 各自正下方的地面必须被染成
//!   自己的颜色，这同时证明数组里**不止第 0 槽**被读到；
//! - 第三次把 `range` 缩到 0.5（地面距离 1.2 > range）⇒ 同一位置必须几乎全黑，
//!   这证明衰减/截断真的按 range 生效，而不是"点光无条件加一个常量"。

use std::{env, fs, path::PathBuf};

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_math::math3d::Vec3;
use rustlua_render::picking::screen_from_world;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// 两盏点光的色温刻意拉开：任何混色/串槽都会让两侧的判据同时失败。
const RED: ([f32; 3], [f32; 3]) = ([1.0, 0.15, 0.1], [-1.5, 1.2, 0.0]);
const GREEN: ([f32; 3], [f32; 3]) = ([0.1, 1.0, 0.2], [1.5, 1.2, 0.0]);
const INTENSITY: f32 = 8.0;
const LIT_RANGE: f32 = 4.0;
/// 地面到光源的距离是 1.2；这个 range 让 falloff 恰好为 0。
const DARK_RANGE: f32 = 0.5;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new(
        "wp0-point-light-golden",
        WIDTH,
        HEIGHT,
    ))
    .expect("offscreen fixture for wp0-point-light-golden");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    // 点光只在 PBR 着色路径里参与（`mesh3d_pbr.wgsl`）；默认档是关的。
    renderer.set_render_settings(rustlua_render::render_settings::RenderSettings {
        pbr: true,
        ..Default::default()
    });
    let texture = fixture.texture();
    let view = fixture.view();

    let lit = render_scene(&mut renderer, &root, texture, view, true, LIT_RANGE);
    let dark = render_scene(&mut renderer, &root, texture, view, true, DARK_RANGE);
    // 默认（非 PBR）着色路径也必须吃到点光：老工程的 `RenderSettings` 不会开 PBR，
    // 点光在那条路径上静默失效是最容易漏的接线。
    let legacy = render_scene(&mut renderer, &root, texture, view, false, LIT_RANGE);

    let luminance = |pixel: [u8; 3]| -> f32 {
        0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2])
    };
    let red_lit = luminance(lit.red_pixel);
    let green_lit = luminance(lit.green_pixel);
    let center_lit = luminance(lit.center_pixel);
    let red_dark = luminance(dark.red_pixel);
    let green_dark = luminance(dark.green_pixel);

    assert!(
        red_lit > red_dark + 15.0,
        "range 覆盖时红灯正下方没有明显变亮：亮={red_lit:.1}（{:?}）暗={red_dark:.1}（{:?}）\
         —— 点光 count/偏移或衰减没有生效",
        lit.red_pixel,
        dark.red_pixel
    );
    assert!(
        lit.red_pixel[0] > lit.red_pixel[1] + 20 && lit.red_pixel[0] > lit.red_pixel[2] + 20,
        "红灯正下方没有偏红：{:?}（两盏灯串槽或颜色/位置打包错位）",
        lit.red_pixel
    );
    assert!(
        green_lit > green_dark + 15.0,
        "range 覆盖时绿灯正下方没有明显变亮：亮={green_lit:.1}（{:?}）暗={green_dark:.1}（{:?}）\
         —— 数组第 1 槽可能没被着色器读到",
        lit.green_pixel,
        dark.green_pixel
    );
    assert!(
        lit.green_pixel[1] > lit.green_pixel[0] + 20
            && lit.green_pixel[1] > lit.green_pixel[2] + 20,
        "绿灯正下方没有偏绿：{:?}",
        lit.green_pixel
    );
    assert!(
        center_lit > luminance(dark.center_pixel) + 10.0,
        "两盏灯中间的地面没有叠加受光：亮={center_lit:.1} 暗={:.1}",
        luminance(dark.center_pixel)
    );
    // 默认（非 PBR）着色路径：同样两盏灯必须同样上色。
    let legacy_red = luminance(legacy.red_pixel);
    let legacy_green = luminance(legacy.green_pixel);
    assert!(
        legacy.red_pixel[0] > legacy.red_pixel[1] + 20
            && legacy.red_pixel[0] > legacy.red_pixel[2] + 20
            && legacy_red > 15.0,
        "默认着色路径上红灯没有生效：{:?}（mesh3d.wgsl 的点光循环没接上）",
        legacy.red_pixel
    );
    assert!(
        legacy.green_pixel[1] > legacy.green_pixel[0] + 20
            && legacy.green_pixel[1] > legacy.green_pixel[2] + 20
            && legacy_green > 15.0,
        "默认着色路径上绿灯没有生效：{:?}",
        legacy.green_pixel
    );

    println!(
        "wp0_point_light_samples red={:?} green={:?} center={:?} dark_red={:?} legacy_red={:?} red_lum={red_lit:.1} green_lum={green_lit:.1}",
        lit.red_pixel, lit.green_pixel, lit.center_pixel, dark.red_pixel, legacy.red_pixel
    );

    let report = json!({
        "report_version": "wp0-point-light-golden-v1",
        "scene": "two point lights over a ground plane (built in memory)",
        "lights": [
            {"color": RED.0, "position": RED.1, "range": LIT_RANGE, "intensity": INTENSITY},
            {"color": GREEN.0, "position": GREEN.1, "range": LIT_RANGE, "intensity": INTENSITY},
        ],
        "samples": {
            "red_lit": lit.red_pixel,
            "green_lit": lit.green_pixel,
            "center_lit": lit.center_pixel,
            "red_dark": dark.red_pixel,
            "green_dark": dark.green_pixel,
            "legacy_red_lit": legacy.red_pixel,
            "legacy_green_lit": legacy.green_pixel,
        },
        "luminance": {
            "red_lit": red_lit,
            "green_lit": green_lit,
            "center_lit": center_lit,
            "red_dark": red_dark,
            "green_dark": green_dark,
        },
        "point_lights": lit.point_light_count,
        "draw_calls": lit.draw_calls,
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
    println!(
        "wp0_point_light_golden_ok two_slots=true range_cutoff=true red_lum={red_lit:.1} green_lum={green_lit:.1} dark_lum={red_dark:.1}"
    );
}

struct PointLightSamples {
    red_pixel: [u8; 3],
    green_pixel: [u8; 3],
    center_pixel: [u8; 3],
    point_light_count: usize,
    draw_calls: u64,
}

/// 渲染一次探针场景。`range` 同时作用于两盏点光：等于 `LIT_RANGE` 时地面被照亮，
/// 等于 `DARK_RANGE` 时地面在衰减半径之外。`pbr` 选着色路径（两条都必须吃点光）。
fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    pbr: bool,
    range: f32,
) -> PointLightSamples {
    renderer.set_render_settings(rustlua_render::render_settings::RenderSettings {
        pbr,
        ..Default::default()
    });
    let mut scene = Scene::new("point-light-probe");
    let sun = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let red = SceneEntityId(3);
    let green = SceneEntityId(4);
    let camera = SceneEntityId(5);
    for (id, name) in [
        (sun, "Sun"),
        (ground, "Ground"),
        (red, "RedLamp"),
        (green, "GreenLamp"),
        (camera, "Camera"),
    ] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    // 强度 0：直射光、阴影与环境项一起消失，读回的颜色只可能来自点光源。
    scene.entities.get_mut(&sun).unwrap().components.insert(
        "Light".into(),
        "direction=0,-1,0;color=1,1,1;intensity=0".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&ground)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    for (id, (color, position)) in [(red, RED), (green, GREEN)] {
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Light".into(),
            format!(
                "direction=0,-1,0;color={},{},{};intensity={INTENSITY};\
                 type=point;position={},{},{};range={range}",
                color[0], color[1], color[2], position[0], position[1], position[2]
            ),
        );
    }
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,3,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );

    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root), scene).expect("point light runtime");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime
        .scene_3d(viewport)
        .expect("point light 3D extraction");
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("point light 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("point light readback");
    let project = |world: Vec3| -> [u8; 3] {
        let point = screen_from_world(extracted.camera, world, viewport)
            .expect("camera projects the sample point")
            .0;
        sample_pixel(&pixels, point)
    };
    PointLightSamples {
        red_pixel: project(Vec3::new(RED.1[0], 0.0, RED.1[2])),
        green_pixel: project(Vec3::new(GREEN.1[0], 0.0, GREEN.1[2])),
        center_pixel: project(Vec3::ZERO),
        point_light_count: extracted.point_lights.len(),
        draw_calls: renderer.last_frame_metrics().draw_calls,
    }
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
                println!("Usage: wp0_point_light_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
