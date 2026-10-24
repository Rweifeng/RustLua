//! WP0 混合 2D/3D semantic golden（`wp0-mixed-golden-v1`）：**同一个世界视图**里
//! Sprite 与 Mesh 按深度策略互相遮挡。
//!
//! 为什么需要它：2D golden 只覆盖 overlay 顶点管线，3D golden 只覆盖网格 pass，两者都**看不到**
//! "精灵与网格在同一份深度缓冲上互相遮挡"这件事——而这正是 docs/54 §8-④ 的判据。收敛前
//! 世界 Sprite 有自己的深度附件、且永远在网格之后画，网格遮挡精灵在结构上不可能发生。
//!
//! 场景 `examples/game_api/scenes/mixed.scene`（一个透视相机 + 一面朝向相机的网格墙 + 三个精灵）：
//! - `FrontSprite`（z = 0，在墙前面）必须**可见**；
//! - `BehindSprite`（z = -2，在墙后面）必须被墙**完全遮住**（像素与墙面参考值一致）；
//! - `Veil`（tint alpha = 0.5）必须**叠加**出既非墙色也非不透明精灵色的第三种像素。
//!
//! 三条一起才说明"深度策略按不透明性下发"真的生效：只断言"精灵可见"会在精灵永远画在最上层时
//! 也通过；只断言"被遮挡"会在精灵根本没被画出时也通过。
//!
//! 运行：`cargo run --release -p engine --example wp0_mixed_golden -- --output <path>`

use std::{env, fs, path::PathBuf};

use engine::host::RuntimeScene;
use rustlua_camera::camera::{DEFAULT_3D_CLEAR, FrameViews, RenderView, Viewport};
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_scene::scene::Scene;
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

/// 采样点（像素坐标，左上原点）。数值由场景里的世界坐标经透视投影推得，见文件头注释。
const FRONT_SPRITE: (u32, u32) = (190, 180);
const BEHIND_SPRITE: (u32, u32) = (417, 180);
const VEIL: (u32, u32) = (320, 102);
/// 纯墙面的参考点：远离三个精灵的投影范围。
const WALL_REFERENCE: (u32, u32) = (320, 320);

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let scene_path = root.join("scenes/mixed.scene");
    let scene = Scene::load(&scene_path).expect("load mixed scene");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(&root), scene).expect("instantiate mixed scene");
    // 世界 Sprite 深度通道：这就是"混合 2D/3D"要打开的那个开关。
    // 视口剔除保持默认开启：世界通道在有 3D 内容时不得再按 2D 相机裁剪，这条 golden 正是
    // 抓出那次误丢的地方（见 `world_channel_does_not_cull_by_the_2d_camera_when_the_scene_has_3d_content`）。
    runtime.set_world_sprite_depth(true);

    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let mesh = runtime.vertices(WIDTH, HEIGHT).expect("2D extraction");
    let scene3d = runtime.scene_3d(viewport).expect("3D extraction");
    assert!(
        !mesh.world_sprites.is_empty(),
        "世界 Sprite 通道没有顶点：开关或提取路径不对"
    );
    assert_eq!(scene3d.drawables.len(), 1, "网格墙应恰好一个 drawable");
    assert_eq!(
        scene3d.drawables[0].name, "Wall",
        "网格墙的名字变了：采样点基于它覆盖全屏"
    );

    let fixture =
        OffscreenTarget::acquire(&OffscreenRequest::new("wp0-mixed-golden", WIDTH, HEIGHT))
            .expect("offscreen fixture for wp0-mixed-golden");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let info = fixture.adapter_info();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");

    // 与真实宿主（`game_app`）逐行同源的视图声明：主视图 = 3D 相机，overlay = 2D 相机。
    // 世界 Sprite 因此与网格共用相机、视口与深度附件（docs/54 §8-③/④）。
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let overlay = runtime.render_view_2d(WIDTH, HEIGHT);
    let main = RenderView::main_3d(scene3d.camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(main),
        overlay: Some(overlay),
    });
    // 回滚档路径也要喂：纯 2D 画面才会用到它（此处声明了主视图，会被当帧覆盖）。
    if let Some(view_projection) = runtime.sprite_view_projection(WIDTH, HEIGHT) {
        renderer.set_world_sprite_camera(view_projection);
    }

    let texture = fixture.texture();
    let view = fixture.view();
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&scene3d), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("mixed golden readback");
    let metrics = renderer.last_frame_metrics();

    let sample = |point: (u32, u32)| -> [u8; 4] {
        let index = ((point.1 * WIDTH + point.0) * 4) as usize;
        [
            pixels[index],
            pixels[index + 1],
            pixels[index + 2],
            pixels[index + 3],
        ]
    };
    let front = sample(FRONT_SPRITE);
    let behind = sample(BEHIND_SPRITE);
    let veil = sample(VEIL);
    let wall = sample(WALL_REFERENCE);

    // 红纹理 ⇒ 精灵像素必须是"红占优"。绿/蓝通道只可能来自墙面的混合。
    let is_sprite_red = |pixel: [u8; 4]| pixel[0] > 150 && pixel[1] < 96 && pixel[2] < 96;

    // 墙面必须真的在画面里（否则"被遮挡"会因为到处是清屏色而假通过）。
    assert!(
        wall[0].abs_diff(clear_red()) > 8
            || wall[1].abs_diff(clear_green()) > 8
            || wall[2].abs_diff(clear_blue()) > 8,
        "墙面参考点与清屏色无法区分：网格没画出来，遮挡断言不成立 {wall:?}"
    );
    assert!(
        is_sprite_red(front),
        "墙前面的不透明精灵必须可见（不透明精灵写深度，网格因此盖不住它）：{front:?}"
    );
    assert!(
        !is_sprite_red(behind),
        "墙后面的精灵必须被网格遮住，实际 {behind:?}"
    );
    let behind_close_to_wall = (0..3).all(|channel| behind[channel].abs_diff(wall[channel]) <= 4);
    assert!(
        behind_close_to_wall,
        "被遮挡的像素必须与墙面参考值一致（证明那里画的是墙）：behind={behind:?} wall={wall:?}"
    );
    // 半透明精灵：必须**既不等于墙面**（说明画上去了）**也不等于不透明精灵**（说明墙面透出来了）。
    // 只断言"红占优"是不够的——50% 的红叠在浅色墙面上同样红占优，那种断言抓不到"半透明被当成
    // 不透明画"这种回归。
    let veil_blended =
        veil != wall && veil[0] > wall[0] + 20 && veil[1] < wall[1] && veil[0] < front[0];
    assert!(
        veil_blended,
        "半透明精灵必须是叠加结果：veil={veil:?} wall={wall:?} front={front:?}"
    );
    assert!(
        metrics.draw_calls >= 4,
        "混合帧应同时包含阴影/网格与世界 Sprite 通道，实际 draw_calls={}",
        metrics.draw_calls
    );

    let report = json!({
        "report_version": "wp0-mixed-golden-v1",
        "scene": "mixed",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "semantic": {
            "meshes": scene3d.drawables.len(),
            "world_sprites": mesh.world_sprites.len() / 6,
            "draw_calls": metrics.draw_calls,
            "instances": metrics.instances,
            "triangles": metrics.triangles
        },
        "occlusion": {
            "front_sprite_visible": is_sprite_red(front),
            "behind_sprite_occluded": behind_close_to_wall && !is_sprite_red(behind),
            "translucent_blended": veil_blended,
            "samples": {
                "front": front,
                "behind": behind,
                "veil": veil,
                "wall": wall
            }
        },
        "gpu_timestamp_supported": renderer.gpu_timestamp_supported()
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

/// 清屏色（`DEFAULT_3D_CLEAR`）的 sRGB 字节，用于"墙面确实画出来了"的判定。
fn clear_red() -> u8 {
    srgb_byte(DEFAULT_3D_CLEAR[0])
}
fn clear_green() -> u8 {
    srgb_byte(DEFAULT_3D_CLEAR[1])
}
fn clear_blue() -> u8 {
    srgb_byte(DEFAULT_3D_CLEAR[2])
}

/// 线性颜色 → sRGB 字节（渲染目标是 `Rgba8UnormSrgb`，读回的是编码后的值）。
fn srgb_byte(linear: f32) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
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
                println!("Usage: wp0_mixed_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
