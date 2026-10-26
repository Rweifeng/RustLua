//! 逐纹理采样设置**真的落到了 GPU 上**（M79 P1-3c 的门）。
//!
//! 为什么这条门不能省成"读盘读到了"：`resources.meta` → `TextureImportSettings` → 采样器这三段里，
//! 只有最后一段错了才是最阴的——设置读到了、面板显示对了、`texture_sampling_of` 也报了 linear，
//! 但绑定组还挂着全局那个 Nearest 采样器（`textured.wgsl` 用的是 `textureSample(tex, samp, uv)`，
//! `samp` 来自绑定组，不是来自全局字段）。那种状态下画面一个像素都不会变，而所有"读数"都是绿的。
//! 所以这里判的是**像素**：拿一张四角四色的 2×2 棋盘，linear 档必须混合出任何一个纹素里都
//! 没有的颜色，nearest 档必须逐字节等于某一个纹素。两者只可能由采样器区分。
//!
//! 键用的是**工程相对路径**（不是 `probe://`）：这条路径要顺带证明"键就是 meta 里的键"，
//! 而 `texture_upload.rs` 用非路径键证明的是另一件事（上传进了缓存）。

use super::*;
use rustlua_assets::import_settings::{
    ResourceMeta, TextureFilter, TextureImportSettings, TextureWrap,
};
use rustlua_camera::camera::{FrameViews, Viewport};
use std::path::{Path, PathBuf};

/// 2×2 棋盘，行主序（第 0 行是 v∈[0,0.5)）。四角四色，**没有白也没有灰**。
const CHECKER: [[u8; 4]; 4] = [
    [255, 0, 0, 255],   // texel (0,0)：红
    [0, 255, 0, 255],   // texel (1,0)：绿
    [0, 0, 255, 255],   // texel (0,1)：蓝
    [255, 255, 0, 255], // texel (1,1)：黄
];

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];

const LINEAR_KEY: &str = "assets/art/checker-linear.png";
const PLAIN_KEY: &str = "assets/art/checker-plain.png";
const CLAMP_KEY: &str = "assets/art/checker-clamp.png";
const REPEAT_KEY: &str = "assets/art/checker-repeat.png";
const MIRROR_KEY: &str = "assets/art/checker-mirror.png";

const SIZE: u32 = 64;

/// nearest 档在"棋盘正中"实际取到的那颗纹素 = **(列 1, 行 0) = 绿**。
///
/// 这里刻意把半格偏置算出来写死，而不是只断言"是某个纹素原样"：声称 uv=(0.5,0.5) 的那个
/// 取样点，真正被采到的是**像素中心** `32.5/64 = 0.5078`。x 轴过了 0.5 ⇒ 列 1；v 轴因为屏幕
/// y 向下而落在 `1 - 0.5078 = 0.4922` ⇒ 没过 0.5 ⇒ 行 0。两个轴各自越过半格的方向不同，
/// 所以"正中=黄"是想当然（第一遍就是这么写错的，红在 `left: [0,255,0]`）。
const CENTER_NEAREST: [u8; 4] = GREEN;

fn temp_root(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    dir.push(format!(
        "rustlua-texsamp-{tag}-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时工程根");
    dir
}

/// 走**生产端**写这份文件（`ResourceMeta` 自己），门因此同时覆盖"写出来的东西运行时读得回"。
///
/// 传空表就是"文件里没有这一张"：`open` 会把盘上已有条目读回来，所以必须先逐条写回默认
/// （`set_texture_settings(.., default)` 的语义就是删行）再保存——`save()` 在空表时是**删文件**。
fn write_meta(root: &Path, entries: &[(&str, TextureImportSettings)]) {
    let mut meta = ResourceMeta::open(root).expect("打开元数据段");
    let existing: Vec<String> = meta.entries().map(|(key, _)| key.to_owned()).collect();
    for key in existing {
        meta.set_texture_settings(Path::new(&key), TextureImportSettings::default())
            .expect("清空旧条目");
    }
    for (key, settings) in entries {
        meta.set_texture_settings(*key, *settings)
            .expect("写入采样设置");
    }
    meta.save().expect("保存元数据段");
}

fn settings(filter: TextureFilter, wrap: TextureWrap) -> TextureImportSettings {
    TextureImportSettings::new(filter, wrap)
}

fn upload_checker(renderer: &mut Renderer, key: &str) {
    let pixels: Vec<u8> = CHECKER.concat();
    renderer
        .upload_texture_rgba8(key, 2, 2, &pixels)
        .expect("上传 2×2 棋盘");
}

/// 整屏四边形：`uv_scale = 1` 是常规用法；`= 2` 让 uv 越出 `[0,1]`，寻址模式才有得可挑。
fn quad_mesh(key: &str, uv_scale: f32) -> rustlua_render::frame::SceneMesh {
    let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color: [1.0; 4],
        sdf: 0,
    };
    rustlua_render::frame::SceneMesh {
        vertices: vec![
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, -1.0, uv_scale, 0.0),
            corner(1.0, 1.0, uv_scale, 1.0),
            corner(-1.0, -1.0, 0.0, 0.0),
            corner(1.0, 1.0, uv_scale, 1.0),
            corner(-1.0, 1.0, 0.0, 1.0),
        ],
        draws: vec![rustlua_render::frame::SceneDraw::screen_ui(1, 0, 6)],
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: vec![key.to_owned()],
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

/// 渲染一次并读回整屏。每次新建目标纹理：门要的是"这一帧画出来是什么"，不是复用率。
fn paint(renderer: &mut Renderer, key: &str, uv_scale: f32) -> Vec<u8> {
    let device = renderer.device.clone();
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-texture-sampling-target"),
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
    let mesh = quad_mesh(key, uv_scale);

    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera =
        Camera2D::new(Vec2::new(SIZE as f32 * 0.5, SIZE as f32 * 0.5), 1.0).expect("camera2d");
    let clear_black = [0.0, 0.0, 0.0, 1.0];
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_2d(camera, viewport, clear_black),
        RenderView::overlay_2d(camera, viewport),
    ));
    let outcome = renderer.render_into_texture(&mesh, None, &view, SIZE, SIZE);
    assert!(
        matches!(outcome, RenderOutcome::Presented),
        "离屏渲染没有走通（key={key}）"
    );
    assert_eq!(
        renderer.last_frame_metrics().dropped_draws,
        0,
        "贴图必须解析到，进了 dropped_draws 就等于这条门在判白底"
    );
    renderer
        .copy_texture_to_cpu(&target, SIZE, SIZE)
        .expect("readback")
}

/// uv → 读回缓冲里的像素。
///
/// 取的是 uv 对应的那个**纹素区间**里的像素，但真正被采样的是像素中心 `(px+0.5)/SIZE`，
/// 它比声称的 uv 偏半格；所以下面每个取样点都刻意离纹素边界 ≥ 0.09 个纹素（注释里给了实算值），
/// 免得门变成"考各家 nearest 的取整口径"。
fn pixel(read: &[u8], uv: [f32; 2], uv_scale: f32) -> [u8; 4] {
    let px = ((uv[0] / uv_scale) * SIZE as f32).round() as usize;
    let py = ((1.0 - uv[1]) * SIZE as f32).round() as usize;
    let offset = (py * SIZE as usize + px) * 4;
    [
        read[offset],
        read[offset + 1],
        read[offset + 2],
        read[offset + 3],
    ]
}

/// nearest 只能"原样吐出某一个纹素"；linear 在棋盘正中心必然给出这四个里没有的灰。
fn is_verbatim_texel(color: [u8; 4]) -> bool {
    CHECKER.contains(&color)
}

fn channel_spread(color: [u8; 4]) -> i32 {
    let max = color[0].max(color[1]).max(color[2]) as i32;
    let min = color[0].min(color[1]).min(color[2]) as i32;
    max - min
}

/// 登记过 linear 的那张图：中心像素必须是**混合色**，任何一个纹素都给不出它。
#[test]
fn a_documented_linear_texture_is_filtered_on_the_gpu() {
    let root = temp_root("linear");
    write_meta(
        &root,
        &[(
            LINEAR_KEY,
            settings(TextureFilter::Linear, TextureWrap::ClampToEdge),
        )],
    );
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device, queue, Some(root.clone()), false, false)
        .expect("offscreen renderer");
    upload_checker(&mut renderer, LINEAR_KEY);
    assert_eq!(
        renderer.texture_sampling_of(LINEAR_KEY),
        Some(settings(TextureFilter::Linear, TextureWrap::ClampToEdge)),
        "槽位上记的采样设置必须是 meta 里那一档"
    );

    let read = paint(&mut renderer, LINEAR_KEY, 1.0);
    // 中心：uv=(0.5,0.5) → 像素 32，其中心在 x=0.5078、v=0.4922（半格偏置，推导见
    // `CENTER_NEAREST`）⇒ 四个纹素都分到权重：红 0.250 / 绿 0.266 / 蓝 0.235 / 黄 0.250。
    let center = pixel(&read, [0.5, 0.5], 1.0);
    assert!(
        !is_verbatim_texel(center),
        "linear 档却取到了单个纹素原样 {center:?}：绑定组还挂着全局 Nearest 采样器"
    );
    for (index, name) in [(0, "R"), (1, "G"), (2, "B")] {
        assert!(
            center[index] > 0 && center[index] < 255,
            "{name} 通道 = {} 是端点值，说明这一路没做插值（中心像素 {center:?}）",
            center[index]
        );
    }
    // 棋盘四色在每通道上都是 {255,255,0,0} 的同一多重集 ⇒ 混合结果必然接近中性灰。
    // 至于 0/255 之间按 sRGB 还是线性插值（取决于纹理格式的解码），这里**故意不判**，
    // 两种算法都落在下面这个上限里：判"接近灰"就够把 nearest 和 linear 分开了。
    assert!(
        channel_spread(center) <= 64,
        "中心像素 {center:?} 通道跨度太大，不像四色均分的结果"
    );
    assert_eq!(center[3], 255, "四个纹素的 alpha 都是 255，混合不该改变它");
    std::fs::remove_dir_all(&root).ok();
}

/// **没登记过**的键必须逐字节维持今天的样子（默认档 == 原全局采样器，逐参数一致）。
#[test]
fn the_undocumented_texture_path_keeps_todays_pixels() {
    let root = temp_root("undocumented");
    // 文件在、表里只有**别的**键：证明"按键查"而不是"有文件就整批换采样器"。
    write_meta(
        &root,
        &[(
            LINEAR_KEY,
            settings(TextureFilter::Linear, TextureWrap::ClampToEdge),
        )],
    );
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device, queue, Some(root.clone()), false, false)
        .expect("offscreen renderer");
    upload_checker(&mut renderer, PLAIN_KEY);
    assert_eq!(
        renderer.texture_sampling_of(PLAIN_KEY),
        Some(TextureImportSettings::default()),
        "没登记过的键必须落在默认档"
    );

    let read = paint(&mut renderer, PLAIN_KEY, 1.0);
    // 四角（纹素中心）逐通道等于原纹素——这条与 `texture_upload.rs` 的同名断言等价，
    // 差别只在这里多了 project_root，因此它还额外钉住"读元数据段没把默认档改掉"。
    for (u, v, expected) in [
        (0.25, 0.25, RED),
        (0.75, 0.25, GREEN),
        (0.25, 0.75, [0u8, 0, 255, 255]),
        (0.75, 0.75, YELLOW),
    ] {
        let actual = pixel(&read, [u, v], 1.0);
        assert_eq!(actual, expected, "uv=({u},{v}) 默认档必须取到纹素原样");
    }
    // 中心：默认档只能给出某个纹素原样（哪一颗见 `CENTER_NEAREST` 的推导）。
    let center = pixel(&read, [0.5, 0.5], 1.0);
    assert!(
        is_verbatim_texel(center),
        "默认档在棋盘正中取到了混合色 {center:?}：全局 Nearest 采样器被换掉了"
    );
    assert_eq!(
        center, CENTER_NEAREST,
        "正中取到的纹素与半格偏置的推导不一致"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// 运行中改设置 → `reload_texture_settings` 就地换绑定组，**不重传像素**也要立刻见效。
///
/// 这条是编辑器路径：面板改滤镜后不重启预览、不清缓存，下一帧就该是糊的。
/// 它同时是 `sampling_epoch` 的门——2D 保序合并按 `(texture, sdf, bitmap)` 缓存绑定组，
/// 槽位号没变，纪元里少了第四项就会继续用旧绑定组，症状正是"改了没反应"。
#[test]
fn reload_swaps_the_sampler_without_reuploading() {
    let root = temp_root("reload");
    write_meta(&root, &[]);
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device, queue, Some(root.clone()), false, false)
        .expect("offscreen renderer");
    upload_checker(&mut renderer, LINEAR_KEY);

    let before = pixel(&paint(&mut renderer, LINEAR_KEY, 1.0), [0.5, 0.5], 1.0);
    assert_eq!(before, CENTER_NEAREST, "起手没登记 ⇒ 默认档取纹素原样");
    let epoch_before = renderer.texture_epoch();

    // 作者改设置：写盘（生产端就是面板那一步）。
    write_meta(
        &root,
        &[(
            LINEAR_KEY,
            settings(TextureFilter::Linear, TextureWrap::ClampToEdge),
        )],
    );
    assert_eq!(
        renderer.reload_texture_settings(),
        1,
        "一张已上传的贴图变了就该报 1，报 0 说明重读盘没生效"
    );
    assert_eq!(
        renderer.loaded_texture_count(),
        1,
        "就地换绑定组不许新增槽位（重传等于把整批纹理作废）"
    );
    assert_eq!(
        renderer.texture_sampling_of(LINEAR_KEY),
        Some(settings(TextureFilter::Linear, TextureWrap::ClampToEdge)),
    );
    assert_ne!(
        renderer.texture_epoch(),
        epoch_before,
        "纪元必须变：合批缓存靠它作废旧绑定组"
    );

    let after = pixel(&paint(&mut renderer, LINEAR_KEY, 1.0), [0.5, 0.5], 1.0);
    assert!(
        !is_verbatim_texel(after),
        "换档后中心仍是单个纹素 {after:?}：绑定组没换，或合批缓存还挂着旧的"
    );
    assert!(
        channel_spread(after) <= 64,
        "换档后中心 {after:?} 不接近中性灰"
    );

    // 改回默认也要能立刻回去（面板上"取消勾选"是同一条路）。
    write_meta(&root, &[]);
    assert_eq!(
        renderer.reload_texture_settings(),
        1,
        "回到默认也是一次改变"
    );
    let back = pixel(&paint(&mut renderer, LINEAR_KEY, 1.0), [0.5, 0.5], 1.0);
    assert_eq!(
        back, CENTER_NEAREST,
        "改回默认后中心 {back:?} 应逐字节回到纹素原样"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// 寻址模式也真的进了采样器：uv 越出 `[0,1]` 时三档给出**不同**的纹素。
///
/// 取 `uv_scale = 2`（整屏 uv.x ∈ [0,2]）、nearest，两个取样点各自分开一对：
/// - uv=1.4375（像素 46，中心 1.4531）：repeat → 回绕到 0.4531（列 0，红）；
///   clamp / mirror → 列 1（绿）。
/// - uv=1.90625（像素 61，中心 1.9219）：mirror → 2-1.9219=0.0781（列 0，红）；
///   clamp / repeat → 列 1（绿）。
///
/// 两点合起来三档互不相同（clamp=绿绿、repeat=红绿、mirror=绿红）；
/// v 固定在 0.25（像素 48，中心 0.2422）⇒ 只看第 0 行（红/绿）。
#[test]
fn the_documented_wrap_mode_reaches_the_sampler() {
    let root = temp_root("wrap");
    write_meta(
        &root,
        &[
            (
                CLAMP_KEY,
                settings(TextureFilter::Nearest, TextureWrap::ClampToEdge),
            ),
            (
                REPEAT_KEY,
                settings(TextureFilter::Nearest, TextureWrap::Repeat),
            ),
            (
                MIRROR_KEY,
                settings(TextureFilter::Nearest, TextureWrap::MirroredRepeat),
            ),
        ],
    );
    let Some((device, queue)) = super::texture_upload::request_gpu() else {
        return gpu_skipped("request_adapter");
    };
    let mut renderer = Renderer::new_offscreen(device, queue, Some(root.clone()), false, false)
        .expect("offscreen renderer");

    let mut seen: Vec<([u8; 4], [u8; 4])> = Vec::new();
    for (key, label, expect_wrap) in [
        (CLAMP_KEY, "clamp", TextureWrap::ClampToEdge),
        (REPEAT_KEY, "repeat", TextureWrap::Repeat),
        (MIRROR_KEY, "mirror", TextureWrap::MirroredRepeat),
    ] {
        upload_checker(&mut renderer, key);
        assert_eq!(
            renderer.texture_sampling_of(key),
            Some(settings(TextureFilter::Nearest, expect_wrap)),
            "{label}：meta 里的寻址档没落到槽位上"
        );
        let read = paint(&mut renderer, key, 2.0);
        let near = pixel(&read, [1.4375, 0.25], 2.0);
        let far = pixel(&read, [1.90625, 0.25], 2.0);
        assert!(
            near == RED || near == GREEN,
            "{label} 近点 {near:?} 不是第 0 行的单个纹素"
        );
        assert!(
            far == RED || far == GREEN,
            "{label} 远点 {far:?} 不是第 0 行的单个纹素"
        );
        seen.push((near, far));
    }
    let (clamp, repeat, mirror) = (&seen[0], &seen[1], &seen[2]);
    assert_ne!(
        clamp, repeat,
        "clamp 与 repeat 在 uv=1.44 处取到了同一个纹素"
    );
    assert_ne!(
        clamp, mirror,
        "clamp 与 mirror 在 uv=1.89 处取到了同一个纹素"
    );
    assert_ne!(repeat, mirror, "repeat 与 mirror 的两点读数不该完全相同");
    std::fs::remove_dir_all(&root).ok();
}
