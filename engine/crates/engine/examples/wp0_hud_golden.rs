//! docs/54 §8-⑤ 语义 golden：**ScreenUI 必须在世界内容与 Overlay2D 之上**（HUD 隔离）。
//!
//! ⑤ 的判据是"Legacy2DOverlay 和 ScreenUI 仍可保持旧项目和 UI 行为"。旧的 overlay 路径已经被
//! `wp0_2d_golden` 逐像素钉住；缺的是**层与层之间的关系**：HUD 是屏幕空间的东西，它必须盖住
//! 世界内容与调试绘制，而不是被它们盖住或与之混合。这条只能靠像素证明，因为"谁盖谁"取决于
//! 绘制顺序，而绘制顺序在类型里看不出来。
//!
//! 做法是一次场景、两次渲染的 A/B：
//!
//! ```text
//! 同一个像素（碰撞体调试线经过的位置）
//!   ① ScreenUI 关闭：必须是调试线的青色  ⇒ 证明那一段 Overlay2D 确实画在这里
//!   ② ScreenUI 打开：必须变成 HUD 的品红 ⇒ 证明 ScreenUI 画在 Overlay2D 之后
//! ```
//!
//! 另外两个采样点保证这个结论不是空转：纯世界区域必须是精灵的红，HUD 内部（不在调试线上）
//! 在两次渲染里都必须是 HUD 的品红。任何一处没画出来/顺序错了，都会让其中一条断言失败。

use std::{env, fs, path::PathBuf};

use engine::{
    core::{
        camera::{Camera2D, Viewport},
        games_dir,
        scene::{Scene, SceneEntityId},
        transform::Vec2,
    },
    host::{RenderOutcome, Renderer, RuntimeScene},
};
use serde_json::json;

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

/// 世界精灵（白色贴图 + 红色 tint）的屏幕矩形：中心 (640,360)，边长 256。
const SPRITE_RECT: [f32; 4] = [512.0, 232.0, 768.0, 488.0];

/// HUD 面板（`Image texture=__white__` + `UiLayout`）的屏幕矩形：中心 (640,360)，边长 128。
const PANEL_RECT: [f32; 4] = [576.0, 296.0, 704.0, 424.0];

/// 静态碰撞体的半边长：它的调试轮廓盒子在 HUD 面板**内部**（中心 (640,360)）。
const COLLIDER_HALF: f32 = 32.0;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let camera = Camera2D::new(Vec2::new(640.0, 360.0), 1.0).expect("camera");
    let scene = hud_scene(camera, viewport);
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("instantiate HUD golden");

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the HUD golden");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-hud-golden"),
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
        label: Some("wp0-hud-golden-target"),
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

    // 调试绘制打开：这样 Overlay2D 层里一定有一段几何**压在 HUD 面板的位置上**。
    runtime.set_collider_debug_draw(true);

    // ① ScreenUI 关闭（编辑器场景视图就是这样）：HUD 不画。
    runtime.set_render_ui_overlay(false);
    let mesh_without_ui = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("extraction without screen UI");
    let layers = |mesh: &engine::host::SceneMesh| -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for draw in &mesh.draws {
            match draw.render_layer {
                engine::core::layer::RenderLayer::World2D => counts.0 += 1,
                engine::core::layer::RenderLayer::Overlay2D => counts.1 += 1,
                engine::core::layer::RenderLayer::ScreenUI => counts.2 += 1,
            }
        }
        counts
    };
    let (world_draws, overlay_draws, screen_ui_draws) = layers(&mesh_without_ui);
    assert!(
        world_draws >= 1 && overlay_draws >= 1,
        "世界层与叠加层都必须有内容（world={world_draws} overlay={overlay_draws}）"
    );
    assert_eq!(
        screen_ui_draws, 0,
        "关掉屏幕 UI 后不该还有 ScreenUI 层的 draw"
    );
    assert!(matches!(
        renderer.render_into_texture(&mesh_without_ui, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_without_ui = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("readback without screen UI");

    // ② ScreenUI 打开（默认）：同一批世界/叠加内容，HUD 画在它们之上。
    runtime.set_render_ui_overlay(true);
    let mesh_with_ui = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("extraction with screen UI");
    let (world_draws_ui, overlay_draws_ui, screen_ui_draws_ui) = layers(&mesh_with_ui);
    assert!(
        screen_ui_draws_ui >= 1,
        "打开屏幕 UI 后必须出现 ScreenUI 层的 draw"
    );
    assert_eq!(
        (world_draws_ui, overlay_draws_ui),
        (world_draws, overlay_draws),
        "开关屏幕 UI 不该改变世界层与叠加层的 draw 数"
    );
    assert!(matches!(
        renderer.render_into_texture(&mesh_with_ui, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels_with_ui = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("readback with screen UI");

    // 采样点：碰撞体轮廓的左边缘（Overlay2D 经过 HUD 面板内部）、纯世界区域、HUD 内部。
    let outline_x = (640.0 - COLLIDER_HALF) as u32;
    let outline = sample(&pixels_without_ui, outline_x, 360);
    let outline_covered = sample(&pixels_with_ui, outline_x, 360);
    let world_only = sample(&pixels_with_ui, 530, 250);
    let hud_inside_without = sample(&pixels_without_ui, 640, 360);
    let hud_inside_with = sample(&pixels_with_ui, 640, 360);

    // ① 关掉 UI 时：轮廓像素是**固定体**的青色（docs/54 §8-⑥ 的配色），说明这一段叠加内容
    //    确实画在 HUD 面板的位置上——否则下面那条"被盖住"就没有意义。
    assert!(
        outline[1] > 150 && outline[2] > 180 && outline[0] + 60 < outline[1],
        "关闭屏幕 UI 时该像素应为静态碰撞体轮廓的青色，实际 {outline:?}"
    );
    // ② 打开 UI 时：同一像素变成 HUD 的品红 ⇒ ScreenUI 画在 Overlay2D 之后。
    assert!(
        outline_covered[0] > 180 && outline_covered[2] > 180 && outline_covered[1] < 90,
        "HUD 必须盖住 Overlay2D 的调试线，实际 {outline_covered:?}"
    );
    // ③ 纯世界区域永远是精灵的红：屏幕 UI 只该盖住它自己矩形内的像素。
    assert!(
        world_only[0] > 180 && world_only[1] < 70 && world_only[2] < 70,
        "HUD 矩形之外的世界内容必须保持原色，实际 {world_only:?}"
    );
    // ④ HUD 内部：关掉 UI 时是精灵的红，打开后是 HUD 的品红。
    assert!(
        hud_inside_without[0] > 180 && hud_inside_without[1] < 70,
        "关闭屏幕 UI 时 HUD 区域应露出下面的世界精灵，实际 {hud_inside_without:?}"
    );
    assert!(
        hud_inside_with[0] > 180 && hud_inside_with[2] > 180 && hud_inside_with[1] < 90,
        "HUD 内部必须是 HUD 自己的颜色，实际 {hud_inside_with:?}"
    );

    let report = json!({
        "report_version": "wp0-hud-golden-v1",
        "viewport": [WIDTH, HEIGHT],
        "sprite_rect": SPRITE_RECT,
        "panel_rect": PANEL_RECT,
        "collider_half": COLLIDER_HALF,
        "layers": {
            "world2d": world_draws,
            "overlay2d": overlay_draws,
            "screen_ui": screen_ui_draws_ui,
        },
        "samples": {
            "overlay_pixel_without_ui": outline,
            "overlay_pixel_with_ui": outline_covered,
            "world_only_with_ui": world_only,
            "hud_inside_without_ui": hud_inside_without,
            "hud_inside_with_ui": hud_inside_with,
        },
        "hud_isolated": true,
        "layer_order": ["world2d", "overlay2d", "screen_ui"],
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = output {
        fs::write(&path, text.as_bytes()).expect("write report");
        println!("report: {}", path.display());
    }
    println!(
        "wp0_hud_golden_ok hud_isolated=true overlay_pixel_without_ui={outline:?} overlay_pixel_with_ui={outline_covered:?}"
    );
}

fn parse_output() -> Option<PathBuf> {
    let mut output = None;
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        if value == "--output" {
            output = values.next().map(PathBuf::from);
        }
    }
    output
}

/// 场景：红色世界精灵（覆盖整块画面中央） + 品红 HUD 面板（盖在精灵中央） +
/// 静态碰撞体（它的调试轮廓完全落在 HUD 面板内部）。
///
/// 三层刻意互相重叠，因此"谁在最上面"一定会在像素上留下痕迹。
fn hud_scene(camera: Camera2D, viewport: Viewport) -> Scene {
    let mut scene = Scene::new("hud-golden");
    let sprite = SceneEntityId(1);
    let panel = SceneEntityId(2);
    let probe = SceneEntityId(3);

    let world = camera
        .screen_to_world(Vec2::new(640.0, 360.0), viewport)
        .expect("sprite world position");
    scene.add_entity(sprite, "WorldSprite", None).unwrap();
    let entity = scene.entities.get_mut(&sprite).unwrap();
    entity.components.insert(
        "Transform".into(),
        format!("x={};y={};rotation=0;scale_x=1;scale_y=1", world.x, world.y),
    );
    entity.components.insert(
        "Sprite".into(),
        "texture=__white__;size=256x256;pivot=0.500,0.500;\
         tint=1.000,0.000,0.000,1.000;sorting_layer=default;order=0"
            .into(),
    );

    scene.add_entity(panel, "HudPanel", None).unwrap();
    let entity = scene.entities.get_mut(&panel).unwrap();
    entity.components.insert(
        "Transform".into(),
        "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
    );
    entity.components.insert(
        "Image".into(),
        "texture=__white__;color=0.900,0.100,0.900".into(),
    );
    entity.components.insert(
        "UiLayout".into(),
        "anchors=0.5,0.5,0.5,0.5;offsets=-64.00,-64.00,64.00,64.00;\
         min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000"
            .into(),
    );

    scene.add_entity(probe, "ColliderProbe", None).unwrap();
    let entity = scene.entities.get_mut(&probe).unwrap();
    entity.components.insert(
        "Transform".into(),
        format!("x={};y={};rotation=0;scale_x=1;scale_y=1", world.x, world.y),
    );
    entity.components.insert(
        "Collider".into(),
        format!(
            "shape=cuboid;half_width={COLLIDER_HALF};half_height={COLLIDER_HALF};dynamic=false"
        ),
    );
    scene
}

fn sample(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let index = ((y * WIDTH + x) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}
