//! M82-B2：程序化贴图的渲染侧门（生成像素 → 上传 → 采样 + 确定性 + 原地替换）。
//!
//! 生成核心（`rustlua-render::texture_gen`）已在 CPU 侧证明「同 spec 同字节」。这里证明
//! 那串字节**真的进了 GPU 并被采样到**——消费路径与文件贴图完全相同：
//! `upload_texture_rgba8(key,…)` 把像素注册进 `texture_paths`，`SceneMesh.paths=[key]`
//! 的四边形经 `bind_group_for` 命中缓存。键故意取哨兵 `__texture__?…`（**不是文件路径**）：
//! 解析不到就只能落白兜底，于是「采样点等于生成色」唯一解释就是「上传进了缓存且接对了线」。
//!
//! 四件事：
//! * **采样即生成**：checker 四象限中心逐通道等于 CPU 生成的调色板色（端点 0/255，sRGB 精确）。
//! * **确定性**：同一 spec 渲染两帧逐字节相同（GPU 往返不引入抖动）。
//! * **seed 真的到 GPU**：两个 seed 的 noise 回读不同。
//! * **原地替换防泄漏**：同 key 重生成复用槽位（`loaded_texture_count` 不增长）且采样更新。
//!
//! 计数判据（`draw_calls>=1`、`dropped_draws==0`）落在回读采样**之前**：零 draw 帧会落白兜底，
//! 若先采样后计数，「白」可能被误读成「画了但颜色不对」。反面控制：把 `upload_texture_rgba8`
//! 那行删掉 ⇒ 哨兵键解析不到、`dropped_draws>0` 或采样全白，本门当场红。

use super::*;
use rustlua_camera::camera::{FrameViews, Viewport};
use rustlua_render::frame::{SceneDraw, SceneMesh};
use rustlua_render::texture::TextureAsset;
use rustlua_render::texture_gen::{TextureKind, TextureSpec};

const SIZE: u32 = 64;
/// 哨兵键：故意不是文件路径，解析不到只能落白兜底。
const KEY: &str = "__texture__?gate";

/// 上传一张生成贴图、铺满全屏四边形、回读像素。计数判据在回读之前。
fn render_generated(
    renderer: &mut Renderer,
    device: &wgpu::Device,
    key: &str,
    asset: &TextureAsset,
) -> Vec<u8> {
    renderer
        .upload_texture_rgba8(key, asset.width(), asset.height(), asset.pixels())
        .expect("上传生成贴图");

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-texture-gen-target"),
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

    // 整屏四边形，左下 = uv(0,0)，与 `texture_upload.rs` 同一朝向。
    let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color: [1.0; 4],
        sdf: 0,
    };
    let mesh = SceneMesh {
        vertices: vec![
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, -1.0, 1.0, 0.0),
            corner(1.0, 1.0, 1.0, 1.0),
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, 1.0, 1.0, 1.0),
            corner(-1.0, 1.0, 0.0, 1.0),
        ],
        draws: vec![SceneDraw::screen_ui(1, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![key.to_owned()],
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
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_2d(camera, viewport, [0.0, 0.0, 0.0, 1.0]),
        RenderView::overlay_2d(camera, viewport),
    ));
    let outcome = renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE, true);
    assert!(matches!(outcome, RenderOutcome::Presented));

    // 计数判据先于采样：零 draw / 被丢弃都必须当场红，不能被白兜底冒充。
    let metrics = renderer.last_frame_metrics();
    assert!(metrics.draw_calls >= 1, "生成贴图必须产生 draw");
    assert_eq!(
        metrics.dropped_draws, 0,
        "生成贴图必须能解析，不许进 dropped_draws"
    );

    renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback")
}

/// uv → 全屏四边形像素 → 逐通道颜色（屏幕 y 向下，py 用 `1-ndc.y`）。
fn sample(read: &[u8], u: f32, v: f32) -> [u8; 4] {
    let ndc_x = -1.0 + 2.0 * u;
    let ndc_y = -1.0 + 2.0 * v;
    let px = (((ndc_x + 1.0) * 0.5) * SIZE as f32).round() as u32;
    let py = (((1.0 - ndc_y) * 0.5) * SIZE as f32).round() as u32;
    let offset = ((py * SIZE + px) * 4) as usize;
    [
        read[offset],
        read[offset + 1],
        read[offset + 2],
        read[offset + 3],
    ]
}

/// 4×4、scale 2 的棋盘：每象限是 2×2 同色块，象限中心双线性采样返回该块色。
fn checker_spec(colors: Vec<[u8; 4]>) -> TextureSpec {
    let mut spec = TextureSpec::default_for(TextureKind::Checker);
    spec.size = 4;
    spec.scale = 2.0;
    spec.colors = colors;
    spec
}

fn noise_spec(seed: u32) -> TextureSpec {
    let mut spec = TextureSpec::default_for(TextureKind::Noise);
    spec.size = 16;
    spec.scale = 4.0;
    spec.seed = seed;
    spec.colors = vec![[0, 0, 0, 255], [255, 255, 255, 255]];
    spec
}

#[test]
fn generated_checker_is_sampled_as_generated() {
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let asset = checker_spec(vec![[255, 0, 0, 255], [0, 0, 255, 255]])
        .generate()
        .expect("generate");
    let read = render_generated(&mut renderer, &device, KEY, &asset);
    assert_eq!(renderer.loaded_texture_count(), 1, "生成贴图必须进 2D 缓存");
    // 象限中心 = 棋盘四格色（cell(0,0)=红、(1,0)=蓝、(0,1)=蓝、(1,1)=红）。
    assert_eq!(sample(&read, 0.25, 0.25), [255, 0, 0, 255]);
    assert_eq!(sample(&read, 0.75, 0.25), [0, 0, 255, 255]);
    assert_eq!(sample(&read, 0.25, 0.75), [0, 0, 255, 255]);
    assert_eq!(sample(&read, 0.75, 0.75), [255, 0, 0, 255]);
}

#[test]
fn generated_texture_round_trip_is_deterministic() {
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let asset = noise_spec(7).generate().expect("generate");
    let first = render_generated(&mut renderer, &device, KEY, &asset);
    let second = render_generated(&mut renderer, &device, KEY, &asset);
    assert_eq!(first, second, "同 spec 的 GPU 往返必须逐字节相同");
}

#[test]
fn generated_noise_differs_by_seed() {
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let a = render_generated(
        &mut renderer,
        &device,
        KEY,
        &noise_spec(1).generate().unwrap(),
    );
    let b = render_generated(
        &mut renderer,
        &device,
        KEY,
        &noise_spec(2).generate().unwrap(),
    );
    assert_ne!(a, b, "不同 seed 的 noise 必须在画面上不同");
}

#[test]
fn regenerated_key_replaces_in_place() {
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let red_blue = checker_spec(vec![[255, 0, 0, 255], [0, 0, 255, 255]])
        .generate()
        .unwrap();
    let first = render_generated(&mut renderer, &device, KEY, &red_blue);
    assert_eq!(renderer.loaded_texture_count(), 1);
    assert_eq!(sample(&first, 0.25, 0.25), [255, 0, 0, 255]);

    // 同 key 重生成（绿/黄）：槽位不增长（防泄漏），且采样必须是新像素。
    let green_yellow = checker_spec(vec![[0, 255, 0, 255], [255, 255, 0, 255]])
        .generate()
        .unwrap();
    let second = render_generated(&mut renderer, &device, KEY, &green_yellow);
    assert_eq!(
        renderer.loaded_texture_count(),
        1,
        "同 key 重生成不许新增槽位（每帧重生成不泄漏）"
    );
    assert_eq!(
        sample(&second, 0.25, 0.25),
        [0, 255, 0, 255],
        "原地替换后采样必须是新像素"
    );
}
