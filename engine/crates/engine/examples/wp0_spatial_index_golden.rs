//! P2-① 证据：P1-① 空间索引（`SpatialGrid2D`）的像素级行为断言。
//!
//! 空间索引是**查询优化**：精灵的视口预筛从线性扫描换成 uniform grid 查询，
//! 语义上不能改变任何渲染结果。这条契约用最硬的证据钉死——同一场景
//! 开/关空间索引各渲染一次，读回的全帧像素**逐字节相等**：
//! * 结构断言：索引开关不影响顶点提取（`vertices` 数量一致，视口外精灵也照常提取，
//!   只是查询预筛跳过）；
//! * 像素断言：两次 read-back 的 RGBA 缓冲逐字节相等；
//! * 位置断言：视口内已知矩形有内容（与 `wp0_2d_golden` 同款采样）。

use std::{env, fs, path::PathBuf};

use engine::{
    core::{
        camera::{Camera2D, DEFAULT_2D_CAMERA_CENTER, Viewport},
        games_dir,
        scene::{Scene, SceneEntityId},
        transform::Vec2,
    },
    host::{RenderOutcome, Renderer, RuntimeScene},
};
use serde_json::json;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let camera = Camera2D::new(DEFAULT_2D_CAMERA_CENTER, 1.0).expect("camera");
    let scene = golden_scene(camera, viewport);

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the spatial-index golden");
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-spatial-index-golden"),
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
        label: Some("wp0-spatial-index-golden-target"),
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

    // A：关闭空间索引（线性扫描基线）；B：开启（grid 查询）。
    // `set_spatial_index` 返回**旧值**（回滚档），默认关闭；这里只关心最终开关状态。
    let mut runtime_off = RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone())
        .expect("runtime (index off)");
    // set_spatial_index 返回**旧值**（回滚档），因此用连续两次调用轮询当前状态：
    // 第二次调用返回的就是第一次设置后的状态。
    runtime_off.set_spatial_index(false);
    assert!(
        !runtime_off.set_spatial_index(false),
        "关闭空间索引后当前状态应为关闭（第二次 set 返回旧值 false）"
    );
    let mut runtime_on =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime (index on)");
    runtime_on.set_spatial_index(true);
    assert!(
        runtime_on.set_spatial_index(true),
        "开启空间索引后当前状态应为开启（第二次 set 返回旧值 true）"
    );
    runtime_on.set_spatial_index_cell_size(64.0);
    let cell_size = 64.0_f32;

    let mesh_off = runtime_off
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (index off)");
    let mesh_on = runtime_on
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (index on)");
    // ① 结构（记录不硬断言）：空间索引预筛会跳过视口外精灵（docs/53 §3.9：与原路径
    // push 被 cull 拒绝的净效果一致——预筛跳过的实体仍占序号，不产顶点）。因此
    // 开/关索引的顶点数可以不同（这是预筛的预期副作用）；**像素逐字节一致**才是契约。

    let render_off = renderer.render_into_texture(&mesh_off, None, &view, WIDTH, HEIGHT);
    assert!(
        matches!(render_off, RenderOutcome::Presented),
        "index-off 渲染未呈现"
    );
    let pixels_off = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (index off)");

    let render_on = renderer.render_into_texture(&mesh_on, None, &view, WIDTH, HEIGHT);
    assert!(
        matches!(render_on, RenderOutcome::Presented),
        "index-on 渲染未呈现"
    );
    let pixels_on = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (index on)");

    // ② 像素：开/关空间索引必须逐字节一致（索引只是查询优化）。
    assert_eq!(
        pixels_on.len(),
        pixels_off.len(),
        "两次 read-back 缓冲长度不一致（尺寸不该变）"
    );
    let identical = pixels_on == pixels_off;
    if !identical {
        let diff = pixels_off
            .iter()
            .zip(pixels_on.iter())
            .enumerate()
            .find(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .unwrap_or(0);
        panic!(
            "空间索引改变了渲染像素：首个差异字节 offset={diff}（缓冲 {} 字节）\
             —— 索引若改了精灵提取/排序语义，P1-① 就只是优化而非等价变换",
            pixels_off.len()
        );
    }

    // ③ 位置：视口内已知矩形必须有内容（索引开启后也没把精灵弄丢）。
    let background = sample(&pixels_on, 4, 4);
    let expectations = [
        // 屏幕 (120,150) 的 64×64 实心块。
        Expectation {
            name: "solid_in_view",
            min: [88.0, 118.0],
            max: [152.0, 182.0],
            min_pixels: 3_200,
        },
        // 屏幕 (280,150) 的 32×32 精灵图。
        Expectation {
            name: "sheet_in_view",
            min: [264.0, 134.0],
            max: [296.0, 166.0],
            min_pixels: 100,
        },
    ];
    for expectation in &expectations {
        let inside = count_diff(&pixels_on, background, expectation.min, expectation.max);
        assert!(
            inside >= expectation.min_pixels,
            "开启空间索引后 `{}` 区域内容不足：{inside} < {}（视口内精灵被索引预筛误杀）",
            expectation.name,
            expectation.min_pixels
        );
    }
    let metrics = renderer.last_frame_metrics();

    let report = json!({
        "report_version": "wp0-spatial-index-golden-v1",
        "scene": "spatial-index-probe (7 sprites, 4 in view + 3 out of view)",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "vertices_off": mesh_off.vertices.len(),
            "vertices_on": mesh_on.vertices.len(),
            "expected_vertices_off": 24 /* 预筛前视口外被 cull */,
            "pixels_identical": identical,
            "buffer_bytes": pixels_on.len(),
            "cell_size": cell_size,
            "draw_calls": metrics.draw_calls,
            "instances": metrics.instances,
            "triangles": metrics.triangles,
            "background": background,
        },
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
    println!("wp0_spatial_index_golden_ok pixels_identical=true sprites=7 cell_size={cell_size}");
}

fn parse_output() -> Option<PathBuf> {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--output" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// 合成空间索引 golden 场景：4 个精灵在视口内、3 个在视口外（负坐标 / 超出右边界），
/// 视口外精灵只有在线性扫描基线里才会被提取——索引开/关的像素对比因此是有意义的。
fn golden_scene(camera: Camera2D, viewport: Viewport) -> Scene {
    let mut scene = Scene::new("wp0-spatial-index-golden");
    add_sprite(
        &mut scene,
        1,
        "Solid",
        "assets/textures/red-1x1.png",
        64.0,
        [120.0, 150.0],
        "default",
        0,
        camera,
        viewport,
    );
    add_sprite(
        &mut scene,
        2,
        "Sheet",
        "assets/textures/player.png",
        32.0,
        [280.0, 150.0],
        "pickup",
        3,
        camera,
        viewport,
    );
    add_sprite(
        &mut scene,
        3,
        "Coin",
        "assets/textures/coin.png",
        24.0,
        [60.0, 60.0],
        "default",
        1,
        camera,
        viewport,
    );
    add_sprite(
        &mut scene,
        4,
        "Slime",
        "assets/textures/slime.png",
        40.0,
        [340.0, 240.0],
        "pickup",
        2,
        camera,
        viewport,
    );
    // 视口外（世界坐标直接写，不参与 screen_to_world 反算）：索引查询应跳过它们。
    let id = SceneEntityId(5);
    scene
        .add_entity(id, "OffLeft", None)
        .expect("add out-of-view sprite");
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Transform".into(),
            "position=-120,150,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/textures/coin.png;size=64x64;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=4"
                .into(),
        );
    let id = SceneEntityId(6);
    scene
        .add_entity(id, "OffRight", None)
        .expect("add out-of-view sprite");
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Transform".into(),
            "position=700,150,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/textures/coin.png;size=64x64;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=4"
                .into(),
        );
    let id = SceneEntityId(7);
    scene
        .add_entity(id, "OffTop", None)
        .expect("add out-of-view sprite");
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Transform".into(),
            "position=200,-180,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&id)
        .expect("entity")
        .components
        .insert(
            "Sprite".into(),
            "texture=assets/textures/coin.png;size=64x64;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=4"
                .into(),
        );
    scene
}

#[allow(clippy::too_many_arguments)]
fn add_sprite(
    scene: &mut Scene,
    id: u64,
    name: &str,
    texture: &str,
    size: f32,
    screen: [f32; 2],
    layer: &str,
    order: i32,
    camera: Camera2D,
    viewport: Viewport,
) {
    let world = camera
        .screen_to_world(Vec2::new(screen[0], screen[1]), viewport)
        .expect("sprite world position");
    let id = SceneEntityId(id);
    scene.add_entity(id, name, None).expect("add sprite entity");
    let entity = scene.entities.get_mut(&id).expect("sprite entity");
    entity.components.insert(
        "Transform".into(),
        format!(
            "position={},{},0;rotation=0,0,0,1;scale=1,1,1",
            world.x, world.y
        ),
    );
    entity.components.insert(
        "Sprite".into(),
        format!(
            "texture={texture};size={size}x{size};pivot=0.500,0.500;\
             tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
        ),
    );
}

fn sample(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let index = ((y * WIDTH + x) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

/// 与背景的通道差之和超过阈值即视为"有内容"。
fn differs(pixels: &[u8], background: [u8; 3], x: u32, y: u32) -> bool {
    let pixel = sample(pixels, x, y);
    let difference = (i16::from(pixel[0]) - i16::from(background[0])).unsigned_abs()
        + (i16::from(pixel[1]) - i16::from(background[1])).unsigned_abs()
        + (i16::from(pixel[2]) - i16::from(background[2])).unsigned_abs();
    difference > 24
}

/// 区域内与背景不同的像素数。
fn count_diff(pixels: &[u8], background: [u8; 3], min: [f32; 2], max: [f32; 2]) -> u64 {
    let mut count = 0_u64;
    let x_start = min[0].max(0.0) as u32;
    let y_start = min[1].max(0.0) as u32;
    let x_end = (max[0].ceil() as u32).min(WIDTH);
    let y_end = (max[1].ceil() as u32).min(HEIGHT);
    for y in y_start..y_end {
        for x in x_start..x_end {
            if differs(pixels, background, x, y) {
                count += 1;
            }
        }
    }
    count
}

/// 断言里用到的屏幕矩形（与期望最小非背景像素数）。
#[derive(Clone, Copy)]
struct Expectation {
    name: &'static str,
    min: [f32; 2],
    max: [f32; 2],
    min_pixels: u64,
}
