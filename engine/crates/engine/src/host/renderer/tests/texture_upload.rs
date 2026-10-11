//! 公开的"内存贴图"上传入口（WP5 第二片的前置）。
//!
//! 为什么单开一个文件：这条路径的**失败形态**极易被当成成功——web/微信上没有文件系统，
//! 2D 贴图按路径解析必然落到白兜底，而"白"恰好也是"什么都没画"的样子。所以这里用一张
//! **四个角都不是白色**的 2×2 贴图：只要上传没进缓存、或者绑定/采样接错了线，采样点立刻
//! 变成白色或别的颜色，断言当场炸，而不是悄悄画出一片白。

use super::*;
use rustlua_camera::camera::{FrameViews, Viewport};

/// 上传用的键：**故意不是文件路径**。
///
/// 这是本文件的核心手法：`bind_group_for` 解析不到这条路径时只能落到白兜底，
/// 于是"采样点是对的"就等于"上传真的进了缓存并被采样到了"——没有第二条解释。
const CHECKER_KEY: &str = "probe://checker2x2";

/// 2×2 棋盘，行主序（第 0 行是 v∈[0,0.5) 的那一半）。四角四色，**没有白**。
const CHECKER: [[u8; 4]; 4] = [
    [255, 0, 0, 255],   // texel (0,0)：红
    [0, 255, 0, 255],   // texel (1,0)：绿
    [0, 0, 255, 255],   // texel (0,1)：蓝
    [255, 255, 0, 255], // texel (1,1)：黄
];

/// 期望：采样 uv 中心 → 颜色。uv 与 texel 的对应关系是"uv ∈ [i/2,(i+1)/2) → 第 i 列"，
/// 所以在纹素中心（0.25/0.75）取样时，双线性过滤返回的**就是**那个纹素本身。
const SAMPLES: [(f32, f32, [u8; 4]); 4] = [
    (0.25, 0.25, [255, 0, 0, 255]),
    (0.75, 0.25, [0, 255, 0, 255]),
    (0.25, 0.75, [0, 0, 255, 255]),
    (0.75, 0.75, [255, 255, 0, 255]),
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
            label: Some("rustlua-texture-upload-device"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("request test device"),
    )
}

/// 上传一张 2×2 棋盘并把整屏四边形贴上去：四个采样点必须**逐通道**等于原纹素。
#[test]
fn uploaded_rgba_texture_is_sampled_as_uploaded() {
    const SIZE: u32 = 64;
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");

    let pixels: Vec<u8> = CHECKER.concat();
    let slot = renderer
        .upload_texture_rgba8(CHECKER_KEY, 2, 2, &pixels)
        .expect("上传 2×2 棋盘");
    assert_eq!(slot, 0, "第一次上传应落在槽位 0");
    assert_eq!(renderer.loaded_texture_count(), 1, "上传必须进 2D 贴图缓存");

    // 同 key 覆盖是**原地替换**（流式图集靠这条）：槽位不变、缓存不增长。
    let again = renderer
        .upload_texture_rgba8(CHECKER_KEY, 2, 2, &pixels)
        .expect("同 key 覆盖上传");
    assert_eq!(again, 0, "同 key 覆盖必须复用槽位");
    assert_eq!(
        renderer.loaded_texture_count(),
        1,
        "同 key 覆盖不该新增槽位"
    );

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-texture-upload-target"),
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
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // 整屏四边形，uv 与 NDC 同向（ysize 上 NDC y 向上、uv v 向"纹素行号增大"方向），
    // 与宿主 `OverlayVertex` 的常规用法一致：左下 = uv(0,0)。
    let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color: [1.0; 4],
        sdf: 0,
    };
    let mesh = crate::host::frame::SceneMesh {
        vertices: vec![
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, -1.0, 1.0, 0.0),
            corner(1.0, 1.0, 1.0, 1.0),
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, 1.0, 1.0, 1.0),
            corner(-1.0, 1.0, 0.0, 1.0),
        ],
        draws: vec![crate::host::SceneDraw::screen_ui(1, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![CHECKER_KEY.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };

    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera =
        Camera2D::new(Vec2::new(SIZE as f32 * 0.5, SIZE as f32 * 0.5), 1.0).expect("camera2d");
    let clear_black = [0.0, 0.0, 0.0, 1.0];
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_2d(camera, viewport, clear_black),
        RenderView::overlay_2d(camera, viewport),
    ));
    let outcome = renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE);
    assert!(matches!(outcome, RenderOutcome::Presented));
    assert_eq!(
        renderer.last_frame_metrics().dropped_draws,
        0,
        "贴图必须能解析到，不能进 dropped_draws"
    );

    let read = renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback");
    assert_eq!(read.len(), (SIZE * SIZE * 4) as usize);

    for (u, v, expected) in SAMPLES {
        // uv → NDC（全屏四边形）→ 像素：屏幕 y 向下，所以 py 用 (1 - ndc.y)。
        let ndc_x = -1.0 + 2.0 * u;
        let ndc_y = -1.0 + 2.0 * v;
        let px = (((ndc_x + 1.0) * 0.5) * SIZE as f32).round() as u32;
        let py = (((1.0 - ndc_y) * 0.5) * SIZE as f32).round() as u32;
        let offset = ((py * SIZE + px) * 4) as usize;
        let actual = [
            read[offset],
            read[offset + 1],
            read[offset + 2],
            read[offset + 3],
        ];
        // 端点值（0/255）在 sRGB 往返上是精确的，所以这里按逐通道相等断言；
        // 真机上若出现 ±1 抖动，说明是取样点落在纹素边界而非本路径错了。
        assert_eq!(
            actual, expected,
            "uv=({u},{v}) 像素=({px},{py}) 期望 {expected:?} 实得 {actual:?}"
        );
    }
}

/// 不合法的输入必须**在进缓存之前**被拒：不能出现"注册了一个空槽位"。
#[test]
fn invalid_uploads_are_rejected_without_touching_the_cache() {
    let Some((device, queue)) = request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");

    // 尺寸与字节数不符（2×2 需要 16 字节）。
    assert!(
        renderer
            .upload_texture_rgba8("probe://short", 2, 2, &[0u8; 15])
            .is_err(),
        "字节数与尺寸不符必须报错"
    );
    // 0 尺寸。
    assert!(
        renderer
            .upload_texture_rgba8("probe://zero", 0, 0, &[])
            .is_err(),
        "0 尺寸必须报错"
    );
    // 不是 PNG 的字节。
    assert!(
        renderer
            .upload_texture_png("probe://garbage", b"not a png at all")
            .is_err(),
        "垃圾字节必须报错"
    );
    assert_eq!(renderer.loaded_texture_count(), 0, "被拒的上传不许留下槽位");
}
