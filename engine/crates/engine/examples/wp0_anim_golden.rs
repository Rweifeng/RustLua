//! P3-E01-4：动画的场景级 golden。
//!
//! 同一个场景里放两路**由 `.anim` 驱动**的实体：2D `Transform.position` + `Sprite`，以及
//! 3D `Transform3D.position`（实体带 `Light`，按 3D 分类）。在 t=0 / 0.5 / 1.0 三个时刻
//! 分别读回世界变换与 2D 精灵的离屏像素；随后再推 0.5 s，证明非循环片段停在末键不再前进。
//!
//! 断言分三层：
//! 1. **语义**：2D x 按 100 px/s、3D x 按 4 unit/s 线性推进，0.5 s 处各取中值；
//! 2. **像素**：2D 精灵的包围盒中心随变换右移 100 px（屏幕坐标），且每帧都有内容；
//! 3. **非循环**：超过 duration 后再步进，变换不再变化（采样钳在末键）。
//!
//! 报告写到 `--output`；没有 GPU 适配器时以 exit 3 显式跳过（不是通过）。

use std::{env, fs, path::PathBuf};

use engine::host::RuntimeScene;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
/// 默认 2D 相机中心 (640,360)；400×300 视口下屏幕 (200,150) 对应世界 (640,360)。
const SPRITE_WORLD_START: f32 = 540.0;
const SPRITE_SCREEN_START: f32 = 100.0;
const SPRITE_STEP_PX: f32 = 100.0;
const SUN_STEP_UNITS: f32 = 4.0;

struct FrameReadback {
    t: f32,
    sprite_x: f32,
    sun_x: f32,
    pixels: u64,
    center: [f32; 2],
}

fn main() {
    let output = parse_output();
    let root = temp_root();
    write_clips(&root);
    copy_sprite_texture(&root);

    let scene = golden_scene();
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene)
        .expect("instantiate anim golden");

    let fixture =
        match OffscreenTarget::acquire(&OffscreenRequest::new("wp0-anim-golden", WIDTH, HEIGHT)) {
            Ok(fixture) => fixture,
            Err(error) => {
                eprintln!("wp0_anim_golden_skipped reason=no_gpu error={error} exit=3");
                std::process::exit(3);
            }
        };
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let info = fixture.adapter_info();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let texture = fixture.texture();
    let view = fixture.view();

    let mut frames = Vec::new();
    frames.push(read_frame(&mut runtime, &mut renderer, texture, view, 0.0));
    runtime
        .run_script_steps(1, 0.5, &InputSnapshot::default())
        .expect("step to 0.5s");
    frames.push(read_frame(&mut runtime, &mut renderer, texture, view, 0.5));
    runtime
        .run_script_steps(1, 0.5, &InputSnapshot::default())
        .expect("step to 1.0s");
    frames.push(read_frame(&mut runtime, &mut renderer, texture, view, 1.0));

    // 非循环片段：再推 0.5 s，两个实体的姿态都必须停在末键。
    let clamped_before = [
        runtime
            .world_transform_2d_of(SceneEntityId(1))
            .expect("sprite transform")
            .position
            .x,
        runtime
            .world_transform_3d_of(SceneEntityId(2))
            .expect("sun transform")
            .position
            .x,
    ];
    runtime
        .run_script_steps(1, 0.5, &InputSnapshot::default())
        .expect("step past duration");
    let clamped_after = [
        runtime
            .world_transform_2d_of(SceneEntityId(1))
            .expect("sprite transform")
            .position
            .x,
        runtime
            .world_transform_3d_of(SceneEntityId(2))
            .expect("sun transform")
            .position
            .x,
    ];

    let expected = [
        (0.0, SPRITE_WORLD_START, 0.0),
        (
            0.5,
            SPRITE_WORLD_START + SPRITE_STEP_PX,
            SUN_STEP_UNITS * 0.5,
        ),
        (
            1.0,
            SPRITE_WORLD_START + 2.0 * SPRITE_STEP_PX,
            SUN_STEP_UNITS,
        ),
    ];
    for (frame, (t, sprite_x, sun_x)) in frames.iter().zip(expected) {
        assert!(
            (frame.t - t).abs() < 1e-4,
            "时间戳 {t} 不对：实读 {}",
            frame.t
        );
        assert!(
            (frame.sprite_x - sprite_x).abs() < 0.05,
            "t={t} 2D x 期望 {sprite_x}，实读 {}",
            frame.sprite_x
        );
        assert!(
            (frame.sun_x - sun_x).abs() < 0.01,
            "t={t} 3D x 期望 {sun_x}，实读 {}",
            frame.sun_x
        );
        let expected_center = SPRITE_SCREEN_START + (sprite_x - SPRITE_WORLD_START);
        assert!(
            (frame.center[0] - expected_center).abs() < 3.0
                && (frame.center[1] - 150.0).abs() < 3.0,
            "t={t} 精灵包围盒中心 {:?}，期望约 ({expected_center}, 150)",
            frame.center
        );
        assert!(
            frame.pixels > 100,
            "t={t} 精灵几乎没有像素：{}",
            frame.pixels
        );
    }
    assert!(
        (clamped_before[0] - clamped_after[0]).abs() < 1e-4
            && (clamped_before[1] - clamped_after[1]).abs() < 1e-4,
        "非循环片段越过后仍在动：before={clamped_before:?} after={clamped_after:?}"
    );

    let report = json!({
        "report_version": "wp0-anim-golden-v1",
        "scene": "wp0-anim-golden",
        "target": {"width": WIDTH, "height": HEIGHT},
        "adapter": {"name": info.name, "backend": format!("{:?}", info.backend)},
        "clips": {
            "sprite": "assets/anim/sprite.anim",
            "sun": "assets/anim/sun.anim",
            "duration": 1.0,
            "looped": false,
        },
        "frames": frames.iter().map(|frame| json!({
            "t": frame.t,
            "sprite_world_x": frame.sprite_x,
            "sun_world_x": frame.sun_x,
            "sprite_screen_center": frame.center,
            "sprite_pixels": frame.pixels,
        })).collect::<Vec<_>>(),
        "clamped_after_duration": {
            "before": clamped_before,
            "after": clamped_after,
            "unchanged": true,
        },
        "checks": {
            "two_d_moves_100px_per_second": true,
            "three_d_moves_4units_per_second": true,
            "pixels_track_the_transform": true,
            "non_loop_stops_at_duration": true,
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
    let _ = fs::remove_dir_all(&root);
}

fn read_frame(
    runtime: &mut RuntimeScene,
    renderer: &mut Renderer,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    t: f32,
) -> FrameReadback {
    let sprite = runtime
        .world_transform_2d_of(SceneEntityId(1))
        .expect("sprite world transform");
    let sun = runtime
        .world_transform_3d_of(SceneEntityId(2))
        .expect("sun world transform");
    let mesh = runtime.vertices(WIDTH, HEIGHT).expect("2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, None, view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("golden readback");
    let background = sample(&pixels, 4, 4);
    let (count, center) = content_bbox(&pixels, background);
    FrameReadback {
        t,
        sprite_x: sprite.position.x,
        sun_x: sun.position.x,
        pixels: count,
        center,
    }
}

fn golden_scene() -> Scene {
    let mut scene = Scene::new("wp0-anim-golden");
    scene.add_entity(SceneEntityId(1), "Sprite", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={SPRITE_WORLD_START},360,0;rotation=0,0,0,1;scale=1,1,1"),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;\
             tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
                .into(),
        );
        entity.components.insert(
            "AnimationPlayer".into(),
            "clip=assets/anim/sprite.anim;autoplay=true;looped=false".into(),
        );
    }
    scene.add_entity(SceneEntityId(2), "Sun", None).unwrap();
    {
        let entity = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        // 带 `Light` 的实体按 3D 分类；本 golden 只读它的 `Transform3D`。
        entity.components.insert(
            "Light".into(),
            "direction=0.4,-0.8,0.5;color=1,1,1;intensity=1".into(),
        );
        entity.components.insert(
            "AnimationPlayer".into(),
            "clip=assets/anim/sun.anim;autoplay=true;looped=false".into(),
        );
    }
    scene
}

fn write_clips(root: &std::path::Path) {
    fs::create_dir_all(root.join("assets/anim")).expect("create anim directory");
    fs::write(
        root.join("assets/anim/sprite.anim"),
        "\
version=1
name=sprite_move
duration=1
looped=false
track=Transform.position;interp=linear;keys=0:540,360;1:740,360
",
    )
    .expect("write sprite clip");
    fs::write(
        root.join("assets/anim/sun.anim"),
        "\
version=1
name=sun_move
duration=1
looped=false
track=Transform3D.position;interp=linear;keys=0:0,0,0;1:4,0,0
",
    )
    .expect("write sun clip");
}

fn copy_sprite_texture(root: &std::path::Path) {
    let source = games_dir()
        .join("examples")
        .join("game_api")
        .join("assets/textures/player.png");
    let target = root.join("assets/textures");
    fs::create_dir_all(&target).expect("create texture directory");
    fs::copy(&source, target.join("player.png")).expect("copy sprite texture");
}

fn temp_root() -> PathBuf {
    let root = env::temp_dir().join(format!(
        "rustlua-anim-golden-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn sample(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let index = ((y * WIDTH + x) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

fn differs(pixels: &[u8], background: [u8; 3], x: u32, y: u32) -> bool {
    let pixel = sample(pixels, x, y);
    let difference = (i16::from(pixel[0]) - i16::from(background[0])).unsigned_abs()
        + (i16::from(pixel[1]) - i16::from(background[1])).unsigned_abs()
        + (i16::from(pixel[2]) - i16::from(background[2])).unsigned_abs();
    difference > 24
}

/// 返回非背景像素数与它们的包围盒中心（没有内容时中心为 NaN）。
fn content_bbox(pixels: &[u8], background: [u8; 3]) -> (u64, [f32; 2]) {
    let mut count = 0_u64;
    let mut min_x = WIDTH;
    let mut min_y = HEIGHT;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if !differs(pixels, background, x, y) {
                continue;
            }
            count += 1;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    if count == 0 {
        return (0, [f32::NAN, f32::NAN]);
    }
    (
        count,
        [
            (min_x as f32 + max_x as f32) * 0.5,
            (min_y as f32 + max_y as f32) * 0.5,
        ],
    )
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
                println!("Usage: wp0_anim_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
