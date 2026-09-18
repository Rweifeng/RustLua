//! WP0 semantic 2D golden：2D overlay 的结构契约 + 宽容的离屏像素证据。
//!
//! 与 `wp0_3d_golden` 同族，但针对**2D overlay 顶点管线**（`SpriteBatch::build` →
//! 屏幕/NDC 变换 → `OverlayVertex`）。它存在的直接原因：WP6 要把这条路径改成单趟
//! 生成（去掉 `SpriteVertex` 中间缓冲），改动的风险是"画歪/少画/顺序错"，而这类错误
//! 不会被 3D golden 或 CPU 顶点计数发现——所以先立一个像素级的基线。
//!
//! 断言分三层：
//! 1. **结构**：顶点数必须恰好等于（精灵数 + 瓦片数）× 6；
//! 2. **位置**：每个已知矩形内的非背景像素数必须超过阈值；
//! 3. **不漏不歪**：**预期矩形之外必须一个非背景像素都没有**——这条最能把 NDC/偏移/
//!    缩放的错误钉死，而它不需要任何精确整图 hash（跨 GPU 不可比）。

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

/// 瓦片地图的行：第 0 行 = 贴图列 1，第 1 行 = 贴图列 2。两行**故意不同**，
/// 交换版用于证明纵向行序（见 `main`）。
const TILES_ROWS_IN_ORDER: &str = "11112222";

/// 同一张地图，两行内容对调。
const TILES_ROWS_SWAPPED: &str = "22221111";

/// 瓦片地图在屏幕上的矩形：4×2 格 ×32px。注意 `y` 取的是地图**下边缘**——
/// 2D 世界 Y 向上，tiles 的第 0 行落在最下方。
const TILEMAP_RECT: [f32; 4] = [40.0, 210.0, 168.0, 274.0];

/// 一个屏幕矩形与其期望的最小非背景像素数。
#[derive(Clone, Copy)]
struct Expectation {
    name: &'static str,
    min: [f32; 2],
    max: [f32; 2],
    min_pixels: u64,
}

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let camera = Camera2D::new(DEFAULT_2D_CAMERA_CENTER, 1.0).expect("camera");
    let scene = golden_scene(camera, viewport, TILES_ROWS_IN_ORDER);
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("instantiate 2D golden");

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the 2D golden");
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-2d-golden"),
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
        label: Some("wp0-2d-golden-target"),
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
    // 2D 场景没有 3D 提取物：这里必须能只画 overlay 而不依赖 3D 快照。
    assert!(matches!(
        renderer.render_into_texture(&mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback");
    let metrics = renderer.last_frame_metrics();

    // ① 结构：2 个精灵 + 8 个瓦片，每块 6 个顶点。
    let expected_vertices = (2 + 8) * 6;
    assert_eq!(
        mesh.vertices.len(),
        expected_vertices,
        "2D overlay 顶点数变化：说明精灵或瓦片没被提取进去"
    );
    assert!(
        mesh.draws.len() >= 3,
        "draw range 数量异常：{}（精灵层与瓦片层应当分开）",
        mesh.draws.len()
    );
    assert!(
        metrics.draw_calls >= 3,
        "overlay draw call 消失：{}",
        metrics.draw_calls
    );

    // ②③ 像素层：矩形内必须有内容，矩形外必须一个非背景像素都没有。
    let background = sample(&pixels, 4, 4);
    let expectations = [
        // 红块：64×64 实心贴图，矩形 [88,152]×[118,182]。
        Expectation {
            name: "sprite_solid",
            min: [88.0, 118.0],
            max: [152.0, 182.0],
            min_pixels: 3_600,
        },
        // 精灵图：32×32，带透明边缘，矩形 [264,296]×[134,166]。
        Expectation {
            name: "sprite_sheet",
            min: [264.0, 134.0],
            max: [296.0, 166.0],
            min_pixels: 120,
        },
        // 瓦片图：4×2 格 ×32px，矩形 [40,168]×[210,274]。
        Expectation {
            name: "tilemap",
            min: [TILEMAP_RECT[0], TILEMAP_RECT[1]],
            max: [TILEMAP_RECT[2], TILEMAP_RECT[3]],
            min_pixels: 200,
        },
    ];
    let mut report_regions = Vec::new();
    for expectation in expectations {
        let inside = count_diff(&pixels, background, expectation.min, expectation.max);
        assert!(
            inside >= expectation.min_pixels,
            "{} 区域内非背景像素只有 {inside} 个（阈值 {}）：内容没画出来或被裁掉",
            expectation.name,
            expectation.min_pixels
        );
        report_regions.push(json!({
            "name": expectation.name,
            "min": expectation.min,
            "max": expectation.max,
            "non_background_pixels": inside,
        }));
    }
    // 矩形外泄漏：把每个预期矩形外扩 2px 当作容差，任何像素落在外扩矩形之外都算泄漏。
    let leaked = count_leaked(&pixels, background, &expectations);
    assert_eq!(
        leaked, 0,
        "预期矩形之外出现 {leaked} 个非背景像素：投影或批处理顺序错了"
    );

    let center = sample(&pixels, 120, 150);
    assert!(
        center[0] > 140 && center[1] < 110 && center[2] < 110,
        "实心红块的像素颜色异常：{center:?}（贴图没绑上或 UV 错了）"
    );

    // WP7 ②：collider 调试绘制。默认关闭时上面那次渲染必须**没有任何**轮廓像素（已经由
    // leaked == 0 与红块区域像素数 = 4096 同时证明）；开启后轮廓要正好压在精灵矩形边界上。
    assert!(!runtime.collider_debug_draw(), "调试绘制默认必须是关闭的");
    runtime.set_collider_debug_draw(true);
    let debug_mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (debug)");
    // 实心块 1 个 box（4 段）× 6 顶点 = 24，其余内容不变。
    assert_eq!(
        debug_mesh.vertices.len(),
        expected_vertices + 24,
        "collider 调试轮廓的顶点数不对（box 4 段 × 6 顶点）"
    );
    assert!(matches!(
        renderer.render_into_texture(&debug_mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let debug_pixels = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (debug)");
    let debug_metrics = renderer.last_frame_metrics();
    assert!(
        debug_metrics.draw_calls > metrics.draw_calls,
        "开启调试绘制后应多出一个 draw call（白色轮廓）：{} vs {}",
        debug_metrics.draw_calls,
        metrics.draw_calls
    );
    // 矩形左边界在屏幕 x = 88：线宽 1.5px 居中在边界上，所以 x = 87 处应出现绿色轮廓。
    // 用"绿占优"而不是固定 RGB：轮廓是按 sRGB 混合上去的，具体数值会随混合精度变化，
    // 但"明显偏绿而不是偏红"这件事必须成立。
    let outline = sample(&debug_pixels, 87, 150);
    assert!(
        outline[1] > outline[0] + 60 && outline[1] > outline[2] + 40,
        "collider 轮廓像素不是实心碰撞体的绿色：{outline:?}（轮廓没画或没落在碰撞体位置）"
    );
    let debug_center = sample(&debug_pixels, 120, 150);
    assert!(
        debug_center[0] > 140 && debug_center[1] < 110,
        "开启调试绘制后实心块内部被盖住了：{debug_center:?}"
    );
    assert!(
        count_leaked(&debug_pixels, background, &expectations) == 0,
        "collider 轮廓画到了预期矩形之外（说明世界→屏幕投影与碰撞体位置不一致）"
    );

    // 瓦片行的**纵向顺序**：2D 世界是 Y 向上（`Camera2D::world_to_screen` 的 y 取负），
    // 而 `build_cached_tilemap_layer` 按 `(y + 0.5) * tile_size` 排布，因此 **tiles 数组的
    // 第 0 行画在地图最下方**。这不是文档里写过的约定，只能靠像素证明，所以这里用"把两行
    // 交换再画一次"来钉死：交换后上下两行的颜色必须整体对调。
    let mut swapped_runtime = RuntimeScene::from_scene_edit(
        Some(root.as_path()),
        golden_scene(camera, viewport, TILES_ROWS_SWAPPED),
    )
    .expect("instantiate swapped 2D golden");
    let swapped_mesh = swapped_runtime
        .vertices(WIDTH, HEIGHT)
        .expect("2D extraction (swapped)");
    assert_eq!(
        swapped_mesh.vertices.len(),
        expected_vertices,
        "交换瓦片行不该改变顶点数"
    );
    assert!(matches!(
        renderer.render_into_texture(&swapped_mesh, None, &view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let swapped = renderer
        .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
        .expect("golden readback (swapped)");
    let lower = region_signature(&pixels, background, [40.0, 242.0], [168.0, 274.0]);
    let upper = region_signature(&pixels, background, [40.0, 210.0], [168.0, 242.0]);
    let lower_swapped = region_signature(&swapped, background, [40.0, 242.0], [168.0, 274.0]);
    let upper_swapped = region_signature(&swapped, background, [40.0, 210.0], [168.0, 242.0]);
    assert_ne!(
        lower, upper,
        "两行瓦片颜色相同，无法用它证明行序（tileset 的列 1/2 是否真的不同？）"
    );
    assert_eq!(
        lower, upper_swapped,
        "交换 tiles 行后下半区颜色未跟着换：瓦片行序不是「第 0 行在下」"
    );
    assert_eq!(
        upper, lower_swapped,
        "交换 tiles 行后上半区颜色未跟着换：瓦片行序不是「第 0 行在下」"
    );

    // WP4 ④：视口外瓦片剔除的像素等价证明（跨出视口的地图，关/开剔除各渲一次）。
    let mut culling_runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), culling_scene(camera, viewport))
            .expect("instantiate culling scene");
    let (culling_full_vertices, culling_culled_vertices, culling_both_vertices) =
        prove_culling_keeps_pixels(&mut culling_runtime, &mut renderer, &view, &texture);

    let report = json!({
        "report_version": "wp0-2d-golden-v1",
        "scene": "wp0-2d-golden",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "vertices": mesh.vertices.len(),
            "draw_ranges": mesh.draws.len(),
            "draw_calls": metrics.draw_calls,
            "instances": metrics.instances,
            "triangles": metrics.triangles,
            "background": background,
            "center_pixel": center,
            "leaked_pixels": leaked,
            "regions": report_regions,
            "collider_debug": {
                "default_enabled": false,
                "extra_vertices": debug_mesh.vertices.len() - expected_vertices,
                "total_vertices": debug_mesh.vertices.len(),
                "outline_pixel": outline,
                "center_pixel": debug_center,
                "draw_calls": debug_metrics.draw_calls,
                "leaked_pixels": count_leaked(&debug_pixels, background, &expectations),
            },
            "tilemap_culling": {
                "default_enabled": true,
                "full_vertices": culling_full_vertices,
                "tiles_only_vertices": culling_culled_vertices,
                "both_culled_vertices": culling_both_vertices,
                "pixels_identical": true,
            },
            "tile_rows": {
                "in_order_lower": lower,
                "in_order_upper": upper,
                "swapped_lower": lower_swapped,
                "swapped_upper": upper_swapped,
                "row_zero_is_bottom": true,
            },
        }
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

/// 合成 2D golden 场景：每块内容都放在**由屏幕矩形反算出的世界坐标**上，因此断言可以用
/// 屏幕像素直接写。
fn golden_scene(camera: Camera2D, viewport: Viewport, tiles: &str) -> Scene {
    let mut scene = Scene::new("wp0-2d-golden");
    // 屏幕中心 (120,150) 放 64×64 实心贴图。
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
    // 屏幕中心 (280,150) 放 32×32 精灵图，层更高 ⇒ 必须后画。
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
    // 瓦片图：4×2 格 ×32px。`Transform` 是地图的**原点**，而 tiles 的第 0 行在这里落在
    // 最下方（世界 Y 向上），所以把原点放在地图的**下边缘** y=274，地图才占 [210,274]。
    let origin = camera
        .screen_to_world(Vec2::new(TILEMAP_RECT[0], TILEMAP_RECT[3]), viewport)
        .expect("tilemap origin");
    let id = SceneEntityId(3);
    scene
        .add_entity(id, "Tiles", None)
        .expect("add tilemap entity");
    let entity = scene.entities.get_mut(&id).expect("tilemap entity");
    entity.components.insert(
        "Transform".into(),
        format!(
            "x={};y={};rotation=0;scale_x=1;scale_y=1",
            origin.x, origin.y
        ),
    );
    entity.components.insert(
        "Tilemap".into(),
        format!(
            "tile_size=32x32;width=4;height=2;texture=assets/textures/tileset.png;\
             tiles={tiles};solid=00000000"
        ),
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
        format!("x={};y={};rotation=0;scale_x=1;scale_y=1", world.x, world.y),
    );
    entity.components.insert(
        "Sprite".into(),
        format!(
            "texture={texture};size={size}x{size};pivot=0.500,0.500;\
             tint=1.000,1.000,1.000,1.000;sorting_layer={layer};order={order}"
        ),
    );
    // 实心块上挂一个与精灵矩形**完全重合**的 box 碰撞体：这样开启 collider 调试绘制后，
    // 轮廓正好落在精灵矩形边界上（在泄漏检查的 ±2px 容差内），于是"轮廓画在哪"也能被
    // 像素断言钉住。WP7 ② 的调试绘制就是靠这个场景取得像素证据。
    if name == "Solid" {
        entity.components.insert(
            "Collider".into(),
            format!(
                "shape=cuboid;half_width={};half_height={}",
                size / 2.0,
                size / 2.0
            ),
        );
    }
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

/// 区域"颜色签名"：区域内所有非背景像素的 RGBA 分量之和。
///
/// 用和而不是某个像素，是为了对贴图里的少量透明/抗锯齿像素免疫；两行瓦片内容不同，
/// 签名就必然不同，交换行后签名必然对调。
fn region_signature(pixels: &[u8], background: [u8; 3], min: [f32; 2], max: [f32; 2]) -> [u64; 3] {
    let mut signature = [0_u64; 3];
    let x_start = min[0].max(0.0) as u32;
    let y_start = min[1].max(0.0) as u32;
    let x_end = (max[0].ceil() as u32).min(WIDTH);
    let y_end = (max[1].ceil() as u32).min(HEIGHT);
    for y in y_start..y_end {
        for x in x_start..x_end {
            if !differs(pixels, background, x, y) {
                continue;
            }
            let pixel = sample(pixels, x, y);
            for (channel, value) in pixel.iter().enumerate() {
                signature[channel] += u64::from(*value);
            }
        }
    }
    signature
}

/// 预期矩形（外扩 2px 容差）之外的"有内容"像素数。这是本 golden 最灵敏的一条：
/// 它不需要整图 hash，却能把任何平移/缩放/偏移错误钉死。
fn count_leaked(pixels: &[u8], background: [u8; 3], expectations: &[Expectation]) -> u64 {
    let mut leaked = 0_u64;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if !differs(pixels, background, x, y) {
                continue;
            }
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let inside_any = expectations.iter().any(|expectation| {
                px >= expectation.min[0] - 2.0
                    && px <= expectation.max[0] + 2.0
                    && py >= expectation.min[1] - 2.0
                    && py <= expectation.max[1] + 2.0
            });
            if !inside_any {
                leaked += 1;
            }
        }
    }
    leaked
}

fn count_diff(pixels: &[u8], background: [u8; 3], min: [f32; 2], max: [f32; 2]) -> u64 {
    let mut count = 0;
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

/// WP4 ④ / WP6 剔除的**像素等价证明**：同一张"跨出视口"的场景（瓦片地图 + 三个精灵），
/// 把瓦片剔除与精灵剔除的四种组合各渲染一次，四个像素缓冲必须**逐字节相同**。
///
/// 更强的两条隐含断言：① 每个开关各自都必须**确实**减少顶点，否则这个"证明"是空转；
/// ② 场景里那个**跨在视口边缘上**的精灵必须被保留——若剔除用了过紧的包围盒把它扔掉，
/// 边缘像素就会变，`full_pixels != culled_pixels` 立刻炸。这正是"保守外扩"的验收点。
fn prove_culling_keeps_pixels(
    runtime: &mut RuntimeScene,
    renderer: &mut Renderer,
    view: &wgpu::TextureView,
    texture: &wgpu::Texture,
) -> (u64, u64, u64) {
    let render = |runtime: &mut RuntimeScene,
                  renderer: &mut Renderer,
                  tiles: bool,
                  sprites: bool|
     -> (Vec<u8>, usize) {
        runtime.set_tilemap_culling(tiles);
        runtime.set_sprite_culling(sprites);
        let mesh = runtime
            .vertices(WIDTH, HEIGHT)
            .expect("2D extraction for the culling proof");
        assert!(matches!(
            renderer.render_into_texture(&mesh, None, view, WIDTH, HEIGHT),
            RenderOutcome::Presented
        ));
        let pixels = renderer
            .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
            .expect("readback for the culling proof");
        (pixels, mesh.vertices.len())
    };

    let (baseline_pixels, no_culling) = render(runtime, renderer, false, false);
    let (tiles_pixels, tiles_only) = render(runtime, renderer, true, false);
    let (sprites_pixels, sprites_only) = render(runtime, renderer, false, true);
    let (both_pixels, both) = render(runtime, renderer, true, true);

    assert!(runtime.tilemap_culling(), "瓦片剔除必须默认开启");
    assert!(runtime.sprite_culling(), "精灵剔除必须默认开启");
    assert!(
        tiles_only < no_culling,
        "瓦片剔除没有减少顶点（{tiles_only} vs {no_culling}）：这张地图没有跨出视口"
    );
    assert!(
        sprites_only < no_culling,
        "精灵剔除没有减少顶点（{sprites_only} vs {no_culling}）：场景里没有视口外精灵"
    );
    assert!(
        both <= tiles_only && both <= sprites_only,
        "两个剔除叠加后顶点反而更多：both={both} tiles={tiles_only} sprites={sprites_only}"
    );
    for (label, pixels) in [
        ("tiles", &tiles_pixels),
        ("sprites", &sprites_pixels),
        ("both", &both_pixels),
    ] {
        if &baseline_pixels != pixels {
            let differing = baseline_pixels
                .iter()
                .zip(pixels.iter())
                .filter(|(left, right)| left != right)
                .count();
            panic!(
                "剔除（{label}）改变了画面：{differing} 个字节不同——视口外内容不该影响任何像素，\
                 而跨在边缘上的内容必须保留"
            );
        }
    }
    (no_culling as u64, tiles_only as u64, both as u64)
}

/// 合成一张**跨出视口**的场景：16×10 格地图放在屏幕 (300,250) 起（左上区域在视口外），
/// 另加两个精灵——一个完全在视口外（必须被剔除），一个跨在右边缘上（**必须保留**）。
/// 行序仍是"第 0 行在最下方"。
fn culling_scene(camera: Camera2D, viewport: Viewport) -> Scene {
    let mut scene = Scene::new("wp0-2d-culling");
    let origin = camera
        .screen_to_world(Vec2::new(300.0, 250.0), viewport)
        .expect("tilemap origin");
    let id = SceneEntityId(1);
    scene.add_entity(id, "Wide", None).expect("add tilemap");
    let entity = scene.entities.get_mut(&id).expect("tilemap entity");
    entity.components.insert(
        "Transform".into(),
        format!(
            "x={};y={};rotation=0;scale_x=1;scale_y=1",
            origin.x, origin.y
        ),
    );
    entity.components.insert(
        "Tilemap".into(),
        format!(
            "tile_size=32x32;width=16;height=10;texture=assets/textures/tileset.png;\
             tiles={};solid={}",
            "1234".repeat(40),
            "0".repeat(160)
        ),
    );
    // 完全在视口外（中心在屏幕左侧外，半径也不足以够到视口）。
    add_sprite(
        &mut scene,
        2,
        "Offscreen",
        "assets/textures/red-1x1.png",
        64.0,
        [-80.0, 150.0],
        "default",
        0,
        camera,
        viewport,
    );
    // 跨在右边缘上：一半在视口内，必须保留（保守外扩的验收点）。
    add_sprite(
        &mut scene,
        3,
        "Straddle",
        "assets/textures/red-1x1.png",
        64.0,
        [WIDTH as f32 - 8.0, 150.0],
        "default",
        1,
        camera,
        viewport,
    );
    scene
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
                println!("Usage: wp0_2d_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
