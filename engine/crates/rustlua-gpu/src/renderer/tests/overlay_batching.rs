//! 2D 叠加通道的**保序合并**（本片新增）：相邻、同键、顶点首尾相接的 draw 合成一次提交。
//!
//! 为什么这个文件必须存在：这是一次"为了性能而动渲染路径"的改动，而它的**唯一**可接受结果是
//! "提交次数变少、像素一个字节都不变"。所以这里两条都断言：`draw_calls` 必须降下来，
//! 采样点与解析面积必须和逐 draw 画时完全一致——包括"不该合并的地方不许合并"。

use super::*;
use rustlua_camera::camera::{FrameViews, Viewport};

const SIZE: u32 = 64;

/// 一块四边形（两个三角形、6 个顶点）的顶点，NDC 坐标。
fn quad(x0: f32, x1: f32, y0: f32, y1: f32) -> [OverlayVertex; 6] {
    let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color: [1.0; 4],
        sdf: 0,
    };
    [
        corner(x0, y0, 0.0, 0.0),
        corner(x1, y0, 1.0, 0.0),
        corner(x1, y1, 1.0, 1.0),
        corner(x0, y0, 0.0, 0.0),
        corner(x1, y1, 1.0, 1.0),
        corner(x0, y1, 0.0, 1.0),
    ]
}

/// 四块 2×2 排布的四边形，全部内建白贴图（`texture == 0`）。
fn four_quads(rects: [[f32; 4]; 4]) -> Vec<OverlayVertex> {
    rects
        .into_iter()
        .flat_map(|r| quad(r[0], r[1], r[2], r[3]))
        .collect()
}

const RECTS: [[f32; 4]; 4] = [
    [-0.9, -0.1, 0.1, 0.9],
    [-0.1, 0.7, 0.1, 0.9],
    [-0.9, -0.1, -0.9, -0.1],
    [-0.1, 0.7, -0.9, -0.1],
];

fn request_gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    Some(
        pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("rustlua-overlay-batch-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device"),
    )
}

fn target(device: &wgpu::Device, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn set_views(renderer: &mut Renderer) {
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera =
        Camera2D::new(Vec2::new(SIZE as f32 * 0.5, SIZE as f32 * 0.5), 1.0).expect("camera2d");
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_2d(camera, viewport, [0.0, 0.0, 0.0, 1.0]),
        RenderView::overlay_2d(camera, viewport),
    ));
}

fn mesh(
    vertices: Vec<OverlayVertex>,
    draws: Vec<rustlua_render::frame::SceneDraw>,
) -> rustlua_render::frame::SceneMesh {
    rustlua_render::frame::SceneMesh {
        vertices,
        draws,
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

fn sample_centers(pixels: &[u8]) -> Vec<[u8; 4]> {
    RECTS
        .iter()
        .map(|r| {
            let ndc_x = (r[0] + r[1]) * 0.5;
            let ndc_y = (r[2] + r[3]) * 0.5;
            let px = (((ndc_x + 1.0) * 0.5) * SIZE as f32).floor() as u32;
            let py = (((1.0 - ndc_y) * 0.5) * SIZE as f32).floor() as u32;
            let offset = ((py * SIZE + px) * 4) as usize;
            [
                pixels[offset],
                pixels[offset + 1],
                pixels[offset + 2],
                pixels[offset + 3],
            ]
        })
        .collect()
}

/// 四块相邻同贴图的四边形：必须**只剩一次** draw call，且像素与逐 draw 画时完全一致。
#[test]
fn adjacent_same_texture_draws_collapse_into_one_draw_call() {
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    set_views(&mut renderer);
    let vertices = four_quads(RECTS);
    let draws = (0..4)
        .map(|index| rustlua_render::frame::SceneDraw::screen_ui(0, index * 6, 6))
        .collect();
    let target = target(&device, "rustlua-batch-merged");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_into_texture(&mesh(vertices, draws), None, &view, SIZE, SIZE);

    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.draw_calls, 1,
        "四块相邻同贴图四边形必须合成一次 draw call（合并没生效，或者键比实际严格）"
    );
    assert_eq!(metrics.instances, 4, "实例数（四边形数）总和不变");
    assert_eq!(metrics.triangles, 8, "三角形数总和不变");
    assert_eq!(metrics.dropped_draws, 0);

    let pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback");
    for (index, center) in sample_centers(&pixels).into_iter().enumerate() {
        assert_eq!(
            center,
            [255, 255, 255, 255],
            "第 {index} 块四边形中心应为白（合并改变了像素？）"
        );
    }
    // 结构检查：白色像素面积 = 四块的解析面积之和。合并若把两段之间的空隙也画了，
    // 或者把某段漏掉，这条会立刻露馅——单点采样做不到这一点。
    let white = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200)
        .count();
    let expected: usize = RECTS
        .iter()
        .map(|r| {
            let w = f64::from(r[1] - r[0]) * 0.5 * f64::from(SIZE);
            let h = f64::from(r[3] - r[2]) * 0.5 * f64::from(SIZE);
            (w.round() * h.round()) as usize
        })
        .sum();
    assert!(
        (white as f64 - expected as f64).abs() / expected as f64 <= 0.02,
        "白色像素 {white} 与解析面积 {expected} 不符（合并画多了或画少了）"
    );
}

/// 贴图一换，批次必须断开：T0,T0,T1,T0（顶点仍首尾相接）→ 3 次提交。
///
/// 这里用**真贴图**（内存上传的 1×1 纯蓝）而不是解析不到的贴图 1：后者会走 `dropped_draws`
/// 分支，断批的原因就变成了"不连续"而不是"键不同"，测不到本用例要测的那条键。
#[test]
fn different_texture_breaks_the_batch() {
    const BLUE_KEY: &str = "probe://solid-blue";
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    set_views(&mut renderer);
    renderer
        .upload_texture_rgba8(BLUE_KEY, 1, 1, &[0, 0, 255, 255])
        .expect("上传 1×1 纯蓝");
    let vertices = four_quads(RECTS);
    let draws = vec![
        rustlua_render::frame::SceneDraw::screen_ui(0, 0, 6),
        rustlua_render::frame::SceneDraw::screen_ui(0, 6, 6),
        rustlua_render::frame::SceneDraw::screen_ui(1, 12, 6),
        rustlua_render::frame::SceneDraw::screen_ui(0, 18, 6),
    ];
    let target = target(&device, "rustlua-batch-split");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut scene = mesh(vertices, draws);
    scene.paths = vec![BLUE_KEY.to_owned()];
    renderer.render_into_texture(&scene, None, &view, SIZE, SIZE);
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.draw_calls, 3,
        "T0,T0,T1,T0 应分成三批（前两块一批、蓝贴图一块、最后一块）"
    );
    assert_eq!(metrics.triangles, 8);
    assert_eq!(metrics.dropped_draws, 0, "三块贴图都解析得到，不该有丢弃");
    let pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback");
    let centers = sample_centers(&pixels);
    assert_eq!(centers[0], [255, 255, 255, 255]);
    assert_eq!(centers[1], [255, 255, 255, 255]);
    assert_eq!(centers[2], [0, 0, 255, 255], "第三块应是上传的纯蓝");
    assert_eq!(centers[3], [255, 255, 255, 255]);
}

/// 规模效应：1000 块**同贴图首尾相接**的四边形，改造前是 1000 次 `pass.draw`，现在必须只剩 1 次。
///
/// 这条是本片的收益证据，而且它是**设备无关**的：提交次数不需要真机 GPU 就能断言，
/// 所以"软件光栅上不许报性能"这条纪律不会挡住它。（像素在这里不重要，块块重叠也无所谓。）
#[test]
fn a_thousand_adjacent_quads_become_one_draw_call() {
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    set_views(&mut renderer);
    const QUADS: u32 = 1000;
    let mut vertices = Vec::new();
    for _ in 0..QUADS {
        vertices.extend(quad(-0.5, 0.5, -0.5, 0.5));
    }
    let draws = (0..QUADS)
        .map(|index| rustlua_render::frame::SceneDraw::screen_ui(0, index * 6, 6))
        .collect();
    let target = target(&device, "rustlua-batch-scale");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_into_texture(&mesh(vertices, draws), None, &view, SIZE, SIZE);
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.draw_calls, 1,
        "{QUADS} 块同贴图相邻四边形必须合成 1 次提交（改造前是 {QUADS} 次）"
    );
    assert_eq!(metrics.instances, u64::from(QUADS));
    assert_eq!(metrics.triangles, u64::from(QUADS) * 2);
    assert_eq!(metrics.dropped_draws, 0);
}

/// **局限也要钉住**：两张贴图交替出现（T0,T1,T0,T1,…）时一次都合不了——保序合并只吃"同键连续段"。
///
/// 为什么值得写成测试：这条限制决定了下一步该做什么。要真正把绘制次数压下来，得让**产场景的一端**
/// 尽量按图集把同键的 draw 排在一起（Tilemap/图集那一片的活），而不是在这里按贴图重排——
/// 2D 半透明叠加靠的就是顺序，重排等于改画面。
#[test]
fn alternating_textures_never_merge() {
    const ALT_KEY: &str = "probe://solid-magenta";
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    set_views(&mut renderer);
    renderer
        .upload_texture_rgba8(ALT_KEY, 1, 1, &[255, 0, 255, 255])
        .expect("上传 1×1 品红");
    const QUADS: u32 = 200;
    let mut vertices = Vec::new();
    for _ in 0..QUADS {
        vertices.extend(quad(-0.5, 0.5, -0.5, 0.5));
    }
    let draws = (0..QUADS)
        .map(|index| rustlua_render::frame::SceneDraw::screen_ui(index % 2, index * 6, 6))
        .collect();
    let mut scene = mesh(vertices, draws);
    scene.paths = vec![ALT_KEY.to_owned()];
    let target = target(&device, "rustlua-batch-alternating");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_into_texture(&scene, None, &view, SIZE, SIZE);
    let metrics = renderer.last_frame_metrics();
    assert_eq!(
        metrics.draw_calls,
        u64::from(QUADS),
        "交替贴图一次都合不了：{QUADS} 块就是 {QUADS} 次提交"
    );
    assert_eq!(metrics.dropped_draws, 0);
}

/// 顶点区间不连续就**不许**合并：中间那段没被任何 draw 覆盖的顶点一旦合进去就会被画出来。
#[test]
fn non_contiguous_ranges_never_merge() {
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    set_views(&mut renderer);
    // 前两块放在顶点 0..12，另一块放在 18..24：第 12..18 的顶点（一块四边形）谁都不引用。
    let mut vertices = four_quads([RECTS[0], RECTS[1], RECTS[2], RECTS[3]]);
    let leaked = vertices[12..18].to_vec();
    vertices.truncate(12);
    vertices.extend(leaked);
    vertices.extend(quad(RECTS[3][0], RECTS[3][1], RECTS[3][2], RECTS[3][3]));
    let draws = vec![
        rustlua_render::frame::SceneDraw::screen_ui(0, 0, 6),
        rustlua_render::frame::SceneDraw::screen_ui(0, 6, 6),
        rustlua_render::frame::SceneDraw::screen_ui(0, 18, 6),
    ];
    let target = target(&device, "rustlua-batch-gap");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_into_texture(&mesh(vertices, draws), None, &view, SIZE, SIZE);
    assert_eq!(
        renderer.last_frame_metrics().draw_calls,
        2,
        "0..6 与 6..12 一样、但第三段在 18..24：前两块合并成一次，第三块必须单独提交"
    );
    let pixels = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback");
    let centers = sample_centers(&pixels);
    assert_eq!(centers[0], [255, 255, 255, 255]);
    assert_eq!(centers[1], [255, 255, 255, 255]);
    assert_eq!(
        centers[2],
        [0, 0, 0, 255],
        "第 12..18 的顶点没有被任何 draw 引用；若被合进批次，第三块四边形就会出现在这里"
    );
    // 第四块（顶点 18..24）应该正常画出。
    assert_eq!(centers[3], [255, 255, 255, 255], "第三段自己那块应正常绘制");
}
