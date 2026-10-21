//! M74-02 P0 度量闸门：帧调度分档实测（`jhyt_*` 与 50k sprite 压测）。
//!
//! 为什么要有这个例子：`--frame-profile` 是给**交互宿主**用的（跑一局游戏、退出时落盘），
//! 而闸门判据必须能在这台机器上重复跑、留读数。本例子用同一个计时口径
//! （[`engine::host::FrameProfile`]）在无窗口条件下跑三个目标：
//!
//! - `jhyt_title` / `jhyt_manor`：真实工程（`examples/game_jhyt`）的标题屏与山庄，
//!   走 `RuntimeScene::new` → 真实脚本、真实场景。
//! - `sprite_10000` / `sprite_50000`：任务里点名的 50k 精灵压测（10k 档作对照，
//!   避免把结论压在单点读数上）。
//!
//! 帧的定义与运行时一致（同 `wp0_scene_frame`）：`run_script_steps` + `vertices` +
//! `scene_3d`，并把析构计入——渲染器消费完这一帧的产物才算帧结束。因此
//! "最慢 stage 占帧比"的分母是玩家真正看到的那一帧，不是调度器自己的一小段。
//!
//! 运行：
//! ```powershell
//! cd engine
//! cargo run --release -p engine --example m74_ecs_frame_profile -- --frames 300 `
//!   --output ..\metrics\baselines\m74-frame-profile.json
//! ```

use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use engine::host::{FrameProfile, FrameProfileSummary, RuntimeScene, PARALLELISM_GATE_SHARE_PCT};
use rustlua_camera::camera::Viewport;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::{Scene, SceneEntityId};

const REPORT_VERSION: &str = "m74-frame-profile-v1";
const VIEWPORT: (u32, u32) = (1280, 720);
const DEFAULT_FRAMES: usize = 300;
/// 预热帧不计入样本：首帧含场景实例化、字体图集与 Lua 模块加载。
const WARMUP_FRAMES: usize = 20;
/// 50k 是任务点名的压测规模；10k 档作对照。
const SPRITE_STAIRS: [usize; 2] = [10_000, 50_000];
const DT: f32 = 1.0 / 60.0;
/// 进山庄的最多尝试帧数（`on_click('BtnStart')` 之后由 `scene_transition` 系统落地）。
const ENTER_MANOR_FRAMES: usize = 10;

struct Config {
    project: Option<PathBuf>,
    frames: usize,
    output: Option<PathBuf>,
}

struct TargetResult {
    name: String,
    summary: FrameProfileSummary,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_config();
    let project = resolve_project(&config)?;
    let examples = rustlua_infra::games_dir().join("examples");
    let mut targets = Vec::new();

    let mut jhyt = RuntimeScene::new(Some(project.as_path()))?;
    // 标题屏：真实工程启动后的第一屏（Lua `update` 与 UI 都在跑）。
    targets.push(measure("jhyt_title", &mut jhyt, config.frames)?);
    enter_manor(&mut jhyt)?;
    // 山庄：玩法主场景（主角、NPC、地块、粒子与 UI 都在这一屏里）。
    targets.push(measure("jhyt_manor", &mut jhyt, config.frames)?);
    drop(jhyt);

    let game_api = examples.join("game_api");
    for sprites in SPRITE_STAIRS {
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(game_api.as_path()), sprite_scene(sprites))?;
        targets.push(measure(
            &format!("sprite_{sprites}"),
            &mut runtime,
            config.frames,
        )?);
    }

    let report = format_report(&config, &targets);
    let path = config
        .output
        .clone()
        .unwrap_or_else(|| rustlua_infra::games_dir().join("metrics/baselines/m74-frame-profile.json"));
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, report)?;

    for target in &targets {
        println!("{}: {}", target.name, target.summary.summary_line());
    }
    let worst = worst_target(&targets);
    println!(
        "m74_frame_profile_ok report_version={REPORT_VERSION} targets={} frames={} \
         gate_threshold_pct={PARALLELISM_GATE_SHARE_PCT:.1} \
         gate_share_pct={:.3} worst_target={} verdict={} report={}",
        targets.len(),
        config.frames,
        worst.map_or(0.0, |target| target.summary.gate_share_pct()),
        worst.map_or("none", |target| target.name.as_str()),
        if worst.is_some_and(|target| target.summary.passes_gate()) {
            "skip"
        } else {
            "proceed"
        },
        path.display()
    );
    Ok(())
}

fn parse_config() -> Config {
    let mut config = Config {
        project: None,
        frames: DEFAULT_FRAMES,
        output: None,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--project" => {
                config.project = Some(PathBuf::from(
                    args.next().expect("--project requires a path"),
                ));
            }
            "--frames" => {
                config.frames = args
                    .next()
                    .expect("--frames requires a value")
                    .parse()
                    .expect("--frames must be an integer");
            }
            "--output" => {
                config.output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: m74_ecs_frame_profile [--project PATH] [--frames N] [--output PATH]"
                );
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    assert!(config.frames > 0, "--frames must be greater than zero");
    config
}

fn resolve_project(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let project = config.project.clone().unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    Ok(project)
}

/// 跑一档：预热 `WARMUP_FRAMES` 帧后连续采 `frames` 帧。
///
/// `last_schedule_report` 只在 `run_script_steps` 里写入，`vertices`/`scene_3d` 不会改动它，
/// 因此帧结束后再取到的仍是这一帧的那一份——不必为读报告克隆整张表（克隆会进入计时窗口）。
fn measure(
    name: &str,
    runtime: &mut RuntimeScene,
    frames: usize,
) -> Result<TargetResult, Box<dyn std::error::Error>> {
    let input = InputSnapshot::default();
    let viewport = viewport()?;
    for _ in 0..WARMUP_FRAMES {
        run_frame(runtime, &input, viewport)?;
    }
    let mut profile = FrameProfile::new();
    for _ in 0..frames {
        let elapsed = run_frame(runtime, &input, viewport)?;
        let report = runtime
            .last_schedule_report()
            .ok_or("run_script_steps 没有留下调度报告")?;
        profile.record(report, elapsed);
    }
    Ok(TargetResult {
        name: name.to_owned(),
        summary: profile.summary(),
    })
}

/// 一帧：固定步 → 2D 提取 → 3D 提取，含析构。计时口径与 `wp0_scene_frame` 一致。
fn run_frame(
    runtime: &mut RuntimeScene,
    input: &InputSnapshot,
    viewport: Viewport,
) -> Result<Duration, Box<dyn std::error::Error>> {
    let started = Instant::now();
    runtime.run_script_steps(1, DT, input)?;
    let mesh = runtime.vertices(VIEWPORT.0, VIEWPORT.1)?;
    let scene3d = runtime.scene_3d(viewport);
    // 渲染器消费完这一帧的产物才算帧结束。
    drop(scene3d);
    drop(mesh);
    Ok(started.elapsed())
}

/// 从标题屏进入山庄（与宿主/Lua 的真实路径一致：`BtnStart` → 固定步里的 `scene_transition`）。
fn enter_manor(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime
        .with_script(|script| script.execute_console("on_click('BtnStart')"))
        .map_err(|error| error.to_string())?;
    let input = InputSnapshot::default();
    let viewport = viewport()?;
    for _ in 0..ENTER_MANOR_FRAMES {
        run_frame(runtime, &input, viewport)?;
        let scene = runtime
            .with_script(|script| script.execute_console("return scene.reload()"))
            .map_err(|error| error.to_string())?;
        if scene.trim() == "manor" {
            return Ok(());
        }
    }
    Err(format!(
        "{} 帧内没有进入 manor（BtnStart 的场景切换没落地？）",
        ENTER_MANOR_FRAMES
    )
    .into())
}

fn viewport() -> Result<Viewport, Box<dyn std::error::Error>> {
    Ok(Viewport::new(VIEWPORT.0 as f32, VIEWPORT.1 as f32)?)
}

fn worst_target(targets: &[TargetResult]) -> Option<&TargetResult> {
    targets.iter().max_by(|left, right| {
        left.summary
            .gate_share_pct()
            .total_cmp(&right.summary.gate_share_pct())
    })
}

fn format_report(config: &Config, targets: &[TargetResult]) -> String {
    let entries = targets
        .iter()
        .map(|target| format!("{{\"name\":\"{}\",\"profile\":{}}}", target.name, target.summary.to_json()))
        .collect::<Vec<_>>()
        .join(",\n    ");
    let worst = worst_target(targets);
    let verdict = if worst.is_some_and(|target| target.summary.passes_gate()) {
        "skip"
    } else {
        "proceed"
    };
    format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \
         \"target\": {{\"frames\": {}, \"warmup_frames\": {}, \"viewport\": [{}, {}]}},\n  \
         \"targets\": [\n    {entries}\n  ],\n  \
         \"gate\": {{\"share_pct\": {:.3}, \"threshold_pct\": {PARALLELISM_GATE_SHARE_PCT:.1}, \
         \"worst_target\": \"{}\", \"verdict\": \"{verdict}\"}}\n}}\n",
        config.frames,
        WARMUP_FRAMES,
        VIEWPORT.0,
        VIEWPORT.1,
        worst.map_or(0.0, |target| target.summary.gate_share_pct()),
        worst.map_or("none", |target| target.name.as_str()),
    )
}

/// 合成 `sprites` 个 Sprite 实体（布局与 `sprite_stress`/`wp0_scene_frame` 同族）。
fn sprite_scene(sprites: usize) -> Scene {
    let mut scene = Scene::new(format!("m74-sprites-{sprites}"));
    for index in 0..sprites {
        let id = SceneEntityId(index as u64 + 1);
        scene
            .add_entity(id, format!("Sprite{index}"), None)
            .expect("add sprite entity");
        let x = (index % 40) as f32 * 34.0 - 600.0;
        let y = (index / 40) as f32 * 34.0 - 300.0;
        let entity = scene.entities.get_mut(&id).expect("sprite entity");
        entity.components.insert(
            "Transform".into(),
            format!("position={x},{y},0;rotation=0,0,0,1;scale=1,1,1"),
        );
        entity.components.insert(
            "Sprite".into(),
            format!(
                "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;\
                 tint=1.000,1.000,1.000,1.000;sorting_layer=default;order={}",
                index / 500
            ),
        );
    }
    scene
}
