//! docs/54 §8-⑩ 的**材质与阴影像素门**（报告 `wp0-lighting-golden-v1`）。
//!
//! 为什么需要它：3D golden 只断言"有内容、几何量对"，材质颜色错、`Light` 方向反了、阴影贴图没被
//! 采样都能蒙过去——本文件正是为了这条门而写的探针，并且**当场就量出了真缺陷**：`Light::direction`
//! 的文档与着色器约定是"光传播方向"（着色器取反得表面→光源向量），而 `renderer::light_view_projection`
//! 把阴影相机放在 `+direction` 一侧，符号相反；示例场景又都写成 `direction=0.4,0.8,0.5`（y>0＝光往
//! 上照）。结果是朝上的表面 `diffuse = 0`、3D 场景实际只有 0.15 的环境光、阴影完全不可见：
//! demo3d 的地面像素恒为环境色 `(55,60,67)`，阴影落点与受光点分别是 `(55,60,67)` 与 `(55,60,68)`。
//!
//! 现在这条门把三件事逐条钉住（都在**像素**上，不是"有没有画东西"）：
//!
//! 1. **漫反射真的随光照方向变化**：同一个地面绕 X 翻转 180°（法线从 +Y 变 -Y）后必须明显变暗。
//!    只断言"地面不是背景色"是不够的——环境光项会让任何表面都有颜色。
//! 2. **阴影真的把地面压暗**：遮挡物正下方的地面必须比远处受光地面暗超过阈值。
//! 3. **受光地面确实被照亮**：受光点必须显著亮于"只有环境光"的水平（用第 1 条的翻转结果作参照）。
//!
//! 光方向 `(0.3,-1,0.2)`（指向传播方向，即自上而下）刻意**不**平行于 Y 轴：完全垂直的光会让阴影
//! 相机的 `look_at` 退化（视线与 up 共线），那样测出来的是"相机坏了"而不是"阴影坏了"。

use std::{env, fs, path::PathBuf};

use engine::{
    core::{
        camera::Viewport,
        games_dir,
        math3d::Vec3,
        picking::screen_from_world,
        scene::{Scene, SceneEntityId},
    },
    host::{RenderOutcome, Renderer, RuntimeScene},
};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// 光传播方向：自上而下（y 为负），略微偏斜以免阴影相机退化。
const LIGHT_DIRECTION: [f32; 3] = [0.3, -1.0, 0.2];
/// 遮挡物高度：阴影落点 = 高度 / |dir.y| × (dir.x, 0, dir.z) 的反向偏移。
const CASTER_HEIGHT: f32 = 1.0;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the lighting golden");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-lighting-golden"),
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
        label: Some("wp0-lighting-golden-target"),
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

    // ---------- 受光那一次：光自上而下 ----------
    let lit = render_scene(&mut renderer, &root, &texture, &view, false);
    // ---------- 光照反向那一次：光自下而上（地面法线背离光源），漫反射应当消失 ----------
    //
    // 刻意用"翻转光"而不是"翻转地面"：地面绕 X 转 180° 会把面的绕序一起翻过来，被
    // `cull_mode: Back` 剔掉，测到的就成了背景色而不是漫反射（本轮实测踩到过）。
    let flipped = render_scene(&mut renderer, &root, &texture, &view, true);

    let luminance = |pixel: [u8; 3]| -> f32 {
        0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2])
    };
    let lit_luminance = luminance(lit.lit_pixel);
    let shadow_luminance = luminance(lit.shadow_pixel);
    let flipped_luminance = luminance(flipped.lit_pixel);

    // ① 漫反射随光照方向变化：地面翻面后必须明显更暗（环境光项让它不会全黑，但会明显掉下来）。
    assert!(
        lit_luminance > flipped_luminance + 20.0,
        "地面翻转 180°（法线背离光源）后没有明显变暗：受光={lit_luminance:.1} 翻面={flipped_luminance:.1} \
         —— 这说明漫反射根本没参与着色（`Light::direction` 符号或光照 uniform 有问题）"
    );
    // ② 阴影真的压暗地面。
    assert!(
        shadow_luminance + 8.0 < lit_luminance,
        "阴影落点没有比受光地面更暗：阴影={:.1}（{:?}）受光={:.1}（{:?}）\
         —— shadow map 没被采样，或阴影相机与光照方向符号不一致",
        shadow_luminance,
        lit.shadow_pixel,
        lit_luminance,
        lit.lit_pixel
    );
    // ③ 受光地面必须显著高于"只有环境光"的水平（用翻面那一份作参照）。
    assert!(
        lit_luminance > flipped_luminance + 20.0,
        "受光地面只比环境光高一点：受光={lit_luminance:.1} 仅环境光={flipped_luminance:.1}"
    );

    println!(
        "wp0_lighting_samples lit={:?} shadow={:?} flipped={:?} lit_lum={lit_luminance:.1} shadow_lum={shadow_luminance:.1} flipped_lum={flipped_luminance:.1}",
        lit.lit_pixel, lit.shadow_pixel, flipped.lit_pixel
    );

    let report = json!({
        "report_version": "wp0-lighting-golden-v1",
        "scene": "lighting-probe (built in memory)",
        "light_direction": LIGHT_DIRECTION,
        "caster_height": CASTER_HEIGHT,
        "samples": {
            "lit": lit.lit_pixel,
            "shadow": lit.shadow_pixel,
            "opposite_light": flipped.lit_pixel,
        },
        "uv": {
            "lit": lit.lit_uv,
            "shadow": lit.shadow_uv,
        },
        "luminance": {
            "lit": lit_luminance,
            "shadow": shadow_luminance,
            "opposite_light": flipped_luminance,
        },
        "diffuse_responds_to_light_direction": lit_luminance > flipped_luminance + 20.0,
        "shadow_darkens_ground": shadow_luminance + 8.0 < lit_luminance,
        "shadow_world_bbox": lit.shadow_world_bbox,
        "shadow_samples": lit.shadow_samples,
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
        "wp0_lighting_golden_ok shadow_darkens=true diffuse_responds=true lit_lum={lit_luminance:.1} shadow_lum={shadow_luminance:.1}"
    );
}

/// 把断言写成小函数只是为了让失败信息里的两个数都出现在消息里（`assert!` 里不好放格式化参数）。
struct LightingSamples {
    lit_pixel: [u8; 3],
    shadow_pixel: [u8; 3],
    lit_uv: [f32; 2],
    shadow_uv: [f32; 2],
    /// "只剩环境光"（=被遮暗）的地面在世界坐标里的足迹：`[min_x, min_z, max_x, max_z]`。
    shadow_world_bbox: [f32; 4],
    shadow_samples: u32,
    draw_calls: u64,
}

/// 渲染一次探针场景：地面 + 遮挡物 + 相机在地面上方俯视。
///
/// `flip_light` 为真时把光的传播方向取反（光自下而上）：地面法线朝上、仍然朝向相机（不会被背面剔除），
/// 但背对光源——于是"漫反射有没有真的参与着色"就只剩这一个变量。
fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    flip_light: bool,
) -> LightingSamples {
    let mut scene = Scene::new("lighting-probe");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let caster = SceneEntityId(3);
    let camera = SceneEntityId(4);
    for (id, name) in [
        (light, "Sun"),
        (ground, "Ground"),
        (caster, "Caster"),
        (camera, "Camera"),
    ] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    let sign = if flip_light { -1.0 } else { 1.0 };
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        format!(
            "direction={},{},{};color=1,1,1;intensity=1",
            LIGHT_DIRECTION[0] * sign,
            LIGHT_DIRECTION[1] * sign,
            LIGHT_DIRECTION[2] * sign
        ),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform3D".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene
        .entities
        .get_mut(&ground)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    // 遮挡物用**同一份内建几何**缩小后的水平面（2×2，悬在 y=1）：
    // 不引入任何外部资产，几何/绕序/法线都已知，阴影落点也确定（见下）。
    // 注意三个轴都要缩：`__ground_plane__` 是 ±4 的 XZ 面，只缩 x/y 会留下 ±4 的 z 向长条
    // （本轮踩到过：遮挡物变成 2×8 的带子，它的影子被自己挡住一大半，量到的"阴影足迹"完全对不上）。
    scene.entities.get_mut(&caster).unwrap().components.insert(
        "Transform3D".into(),
        format!("position=0,{CASTER_HEIGHT},0;rotation=0,0,0,1;scale=0.25,0.25,0.25"),
    );
    scene
        .entities
        .get_mut(&caster)
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    // 相机在地面上方俯视：demo3d 的相机与地面同高，地面在画面里是**边缘朝向**的，
    // 因此那个场景本来就看不到"地面被照亮/被遮暗"。
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera3D".into(),
        "position=0,3,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("lighting runtime");
    let extracted = runtime
        .scene_3d(WIDTH as f32 / HEIGHT as f32)
        .expect("lighting 3D extraction");
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("lighting 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("lighting readback");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let project = |world: Vec3| -> (engine::core::transform::Vec2, [u8; 3]) {
        let point = screen_from_world(extracted.camera, world, viewport)
            .expect("camera projects the sample point")
            .0;
        (point, sample_pixel(&pixels, point))
    };

    // 阴影落点：遮挡物沿光传播方向投到地面上的位置（`-height / dir.y × (dir.x, 0, dir.z)`）。
    let length = (LIGHT_DIRECTION[0] * LIGHT_DIRECTION[0]
        + LIGHT_DIRECTION[1] * LIGHT_DIRECTION[1]
        + LIGHT_DIRECTION[2] * LIGHT_DIRECTION[2])
        .sqrt();
    let travel = Vec3::new(
        LIGHT_DIRECTION[0] / length,
        LIGHT_DIRECTION[1] / length,
        LIGHT_DIRECTION[2] / length,
    );
    assert!(
        travel.y.abs() > 1e-3,
        "探针的光不能与地面平行（阴影落点会跑到无穷远）"
    );
    let shadow_center = Vec3::new(
        -CASTER_HEIGHT / travel.y * travel.x,
        0.0,
        -CASTER_HEIGHT / travel.y * travel.z,
    );
    // 小网格取最暗像素：既避免"差一个像素就判错"，又不放松"确实被遮暗"。
    let mut shadow_pixel = [255_u8; 3];
    let mut shadow_uv = [0.0_f32; 2];
    let mut darkest = f32::MAX;
    for step in -3..=3 {
        for other in -3..=3 {
            let world = Vec3::new(
                shadow_center.x + step as f32 * 0.04,
                0.0,
                shadow_center.z + other as f32 * 0.04,
            );
            let (point, pixel) = project(world);
            let luminance = 0.2126 * f32::from(pixel[0])
                + 0.7152 * f32::from(pixel[1])
                + 0.0722 * f32::from(pixel[2]);
            if luminance < darkest {
                darkest = luminance;
                shadow_pixel = pixel;
                shadow_uv = [point.x, point.y];
            }
        }
    }
    // 受光点取远离遮挡物与阴影的一片地面。
    let (lit_point, lit_pixel) = project(Vec3::new(-2.5, 0.0, 0.0));
    // 证据：阴影在**世界坐标**里的足迹。这条比"某两个采样点的颜色"更能说明问题——落点算错、
    // 遮挡物尺寸写错（本轮把只缩 x/y 的 ±4 面当成 2×2 用，实测足迹 x∈(-0.62,1.25) z∈(-3.8,4.2)
    // 一眼就露出来了）都会在这里显形。判据：足迹应当≈遮挡物足迹 + 光照偏移。
    let ambient_luminance = 0.2126 * 55.0 + 0.7152 * 60.0 + 0.0722 * 67.0;
    let mut world_bbox = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut world_count = 0_u32;
    for step in 0..=64 {
        for other in 0..=64 {
            let x = -4.0 + step as f32 * 0.125;
            let z = -4.0 + other as f32 * 0.125;
            let (_, pixel) = project(Vec3::new(x, 0.0, z));
            let luminance = 0.2126 * f32::from(pixel[0])
                + 0.7152 * f32::from(pixel[1])
                + 0.0722 * f32::from(pixel[2]);
            if (luminance - ambient_luminance).abs() < 4.0 {
                world_count += 1;
                world_bbox = (
                    world_bbox.0.min(x),
                    world_bbox.1.min(z),
                    world_bbox.2.max(x),
                    world_bbox.3.max(z),
                );
            }
        }
    }
    if !flip_light {
        // 单侧光照时，只有"被遮挡"的地面才会落到环境光亮度：足迹必须≳遮挡物本身（2×2）且
        // 大致位于 (0,0)+光照偏移 (0.30,0.20) 附近。
        assert!(
            world_count >= 100,
            "被遮暗的地面太少（{world_count} 个采样点）：阴影几乎不存在"
        );
        assert!(
            (world_bbox.0 - -0.7).abs() < 0.3 && (world_bbox.2 - 1.3).abs() < 0.3,
            "阴影足迹的 x 范围不对：({:.2},{:.2})，期望≈(-0.7,1.3)",
            world_bbox.0,
            world_bbox.2
        );
        assert!(
            (world_bbox.1 - -0.8).abs() < 0.3 && (world_bbox.3 - 1.2).abs() < 0.3,
            "阴影足迹的 z 范围不对：({:.2},{:.2})，期望≈(-0.8,1.2)（遮挡物 2×2 + 0.20 的光照偏移）",
            world_bbox.1,
            world_bbox.3
        );
    }
    LightingSamples {
        lit_pixel,
        shadow_pixel,
        lit_uv: [lit_point.x, lit_point.y],
        shadow_uv,
        shadow_world_bbox: [world_bbox.0, world_bbox.1, world_bbox.2, world_bbox.3],
        shadow_samples: world_count,
        draw_calls: renderer.last_frame_metrics().draw_calls,
    }
}

fn sample_pixel(pixels: &[u8], point: engine::core::transform::Vec2) -> [u8; 3] {
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
                println!("Usage: wp0_lighting_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
