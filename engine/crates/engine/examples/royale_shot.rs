//! ClashRoyale 的 3D 像素证据：离屏跑 900 个固定步（AI 自走 + nav 寻路），
//! 用真实 `Renderer` 把场景里的透视相机 + 竞技场网格 + HUD 合成成一张 PNG。
//!
//! 浏览器 WebGL2 宿主现在也喂 3D（`game_web/src/rust_scene.rs` 的 `scene_3d`，与桌面同一个
//! `RuntimeScene::scene_3d` 调用）；本示例仍走与 `wp0_3d_golden` 相同的离屏管线，给出
//! 不经浏览器的逐像素基线。把 `RUSTLUA_WEBGL2_LIMITS=1` 设在环境里，同一条命令就会用
//! WebGL2 的 downlevel 限额建设备，复现"没有存储缓冲"的那条 3D 路径。
//!
//!     cargo run -q -p engine --example royale_shot -- E:/Qoder/ClashRoyale
//!
//! 第二个参数可指定输出 PNG（默认 `<工程>/artifacts/royale-3d.png`），
//! 第三个参数可指定步数（默认 900）。

use std::path::PathBuf;

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

const WIDTH: u32 = 720;
const HEIGHT: u32 = 1280;
const STEPS: u32 = 900;

fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("E:/Qoder/ClashRoyale"));
    let output = std::env::args()
        .nth(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("artifacts").join("royale-3d.png"));
    // 第三个参数：步数，或 `menu`——只走一帧开始界面（战斗帧停在 0），用来给
    // 标题页出图（背景铺满 + 按钮九宫格）。
    let arg3 = std::env::args().nth(3);
    let menu_mode = arg3.as_deref() == Some("menu");
    let steps = arg3
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(STEPS);

    let scene = Scene::load(root.join("scenes").join("battle.scene"))
        .map_err(|error| format!("加载 battle.scene：{error}"))?;
    let mut runtime =
        RuntimeScene::from_scene(Some(&root), scene).map_err(|error| format!("实例化：{error}"))?;
    if !menu_mode {
        runtime
            .with_script(|script| script.eval_json("__royale_autoplay(true) or true"))
            .map_err(|error| format!("开启 AI 自走：{error}"))?;
    }
    let input = InputSnapshot::default();
    // 受击血条用 `camera3d.world_to_screen` 投影：它读的是**帧首 `vertices()` 回填**的
    // 相机快照，离屏路径不跑这一步就永远拿不到。相机是静态的，开跑前回填一次即可。
    runtime
        .vertices(WIDTH, HEIGHT)
        .map_err(|error| format!("相机回填：{error}"))?;
    if menu_mode {
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .map_err(|error| format!("开始界面帧：{error}"))?;
    } else {
        for step in 0..steps {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .unwrap_or_else(|error| panic!("第 {step} 步失败：{error}"));
        }
    }

    let stats = runtime
        .with_script(|script| script.eval_json("__royale_stats()"))
        .map_err(|error| format!("读取 __royale_stats()：{error}"))?;
    let viewport =
        Viewport::new(WIDTH as f32, HEIGHT as f32).map_err(|error| format!("viewport：{error}"))?;
    let extracted = runtime
        .scene_3d(viewport)
        .ok_or_else(|| "场景里没有 3D 视图".to_owned())?;
    let mut names = extracted
        .drawables
        .iter()
        .map(|drawable| drawable.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .map_err(|error| format!("HUD 提取：{error}"))?;

    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new("royale-shot", WIDTH, HEIGHT))
        .map_err(|error| format!("离屏设备：{error}"))?;
    let info = fixture.adapter_info();
    let mut renderer = Renderer::new_offscreen(
        fixture.device().clone(),
        fixture.queue().clone(),
        Some(root.clone()),
        false,
        true,
    )
    .map_err(|error| format!("renderer：{error}"))?;
    let texture = fixture.texture();
    if !matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), fixture.view(), WIDTH, HEIGHT),
        RenderOutcome::Presented
    ) {
        return Err("离屏渲染没有出帧".to_owned());
    }
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .map_err(|error| format!("读回像素：{error}"))?;
    let metrics = renderer.last_frame_metrics();

    let stats_pixels = analyze(&pixels);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir：{error}"))?;
    }
    let mut image = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        WIDTH,
        HEIGHT,
        image::Rgba([0, 0, 0, 0]),
    );
    for (index, pixel) in image.pixels_mut().enumerate() {
        let src = index * 4;
        *pixel = image::Rgba([
            pixels[src],
            pixels[src + 1],
            pixels[src + 2],
            pixels[src + 3],
        ]);
    }
    image
        .save(&output)
        .map_err(|error| format!("保存 PNG：{error}"))?;

    if menu_mode {
        // 开始界面没有战斗内容，只看"背景是否铺满"：整屏应几乎全是画面，
        // 且四边都摸到边（背景没盖住时，窗口上下会露出清屏色的横带，
        // 那时左上角本身就是清屏色，bounds 直接从中间开始）。
        let total = u64::from(WIDTH) * u64::from(HEIGHT);
        let (x0, y0, x1, y1) = stats_pixels.bounds;
        if stats_pixels.non_background * 100 < total * 85 {
            return Err(format!(
                "开始界面背景没铺满：只有 {} / {} 个非背景像素",
                stats_pixels.non_background, total
            ));
        }
        if x0 > 4 || y0 > 4 || x1 + 4 < WIDTH || y1 + 4 < HEIGHT {
            return Err(format!(
                "开始界面四边有露出：bounds={:?}",
                stats_pixels.bounds
            ));
        }
        let corner = |x: u32, y: u32| {
            let index = ((y * WIDTH + x) * 4) as usize;
            [
                pixels[index],
                pixels[index + 1],
                pixels[index + 2],
                pixels[index + 3],
            ]
        };
        println!(
            "menu ok: png={} frame={} nonbg={}/{} bounds={:?} corners={:?}/{:?}/{:?}/{:?} ui_draws={}",
            output.display(),
            stats["frame"],
            stats_pixels.non_background,
            total,
            stats_pixels.bounds,
            corner(2, 2),
            corner(WIDTH - 3, 2),
            corner(2, HEIGHT - 3),
            corner(WIDTH - 3, HEIGHT - 3),
            metrics.draw_calls,
        );
        return Ok(());
    }

    if stats_pixels.non_background < 20_000 {
        return Err(format!(
            "3D 画面几乎空白：只有 {} 个非背景像素",
            stats_pixels.non_background
        ));
    }
    if stats_pixels.bounds.0 >= stats_pixels.bounds.2
        || stats_pixels.bounds.1 >= stats_pixels.bounds.3
    {
        return Err("3D 内容的像素边界塌缩".to_owned());
    }
    for (label, count) in [
        ("草地", stats_pixels.green),
        ("蓝方", stats_pixels.blue),
        ("红方", stats_pixels.red),
    ] {
        if count < 500 {
            return Err(format!("{label} 在画面里只有 {count} 个像素"));
        }
    }
    println!(
        "shot ok: png={} adapter={:?}/{} frame={} units={} damaged={} bars={} tower_bars={} drawables={} instances={} triangles={} draw_calls={} nonbg={} green={} blue={} red={} bounds={:?}",
        output.display(),
        info.backend,
        info.name,
        stats["frame"],
        stats["units"],
        stats["damaged_towers"],
        stats["hp_bars"],
        stats["hp_bars_towers"],
        extracted.drawables.len(),
        metrics.instances,
        metrics.triangles,
        metrics.draw_calls,
        stats_pixels.non_background,
        stats_pixels.green,
        stats_pixels.blue,
        stats_pixels.red,
        stats_pixels.bounds,
    );
    println!("visible drawables: {}", names.join(", "));
    Ok(())
}

struct PixelStats {
    non_background: u64,
    green: u64,
    blue: u64,
    red: u64,
    bounds: (u32, u32, u32, u32),
}

fn analyze(pixels: &[u8]) -> PixelStats {
    let background = [pixels[0], pixels[1], pixels[2]];
    let mut non_background = 0;
    let mut green = 0;
    let mut blue = 0;
    let mut red = 0;
    let mut min_x = WIDTH;
    let mut min_y = HEIGHT;
    let mut max_x = 0;
    let mut max_y = 0;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let index = ((y * WIDTH + x) * 4) as usize;
            let r = pixels[index];
            let g = pixels[index + 1];
            let b = pixels[index + 2];
            let difference = (i16::from(r) - i16::from(background[0])).unsigned_abs()
                + (i16::from(g) - i16::from(background[1])).unsigned_abs()
                + (i16::from(b) - i16::from(background[2])).unsigned_abs();
            if difference <= 24 {
                continue;
            }
            non_background += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            if g > 70 && g > r.saturating_add(20) && g > b.saturating_add(20) {
                green += 1;
            } else if b > 90 && b > r.saturating_add(40) {
                blue += 1;
            } else if r > 90 && r > g.saturating_add(40) {
                red += 1;
            }
        }
    }
    PixelStats {
        non_background,
        green,
        blue,
        red,
        bounds: (min_x, min_y, max_x, max_y),
    }
}
