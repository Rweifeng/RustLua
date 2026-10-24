//! P3-E06-4 headless 证据：同一场景、同一输入跑两次，采集 → 重放 → 差分。
//!
//! 判据（计划 §6.13 / docs/68 §10）：两次采集的**场景指纹、输入日志、最终状态指纹**
//! 逐位相同；逐系统 p50 与帧 p50 在容差内。任何一项不同都判 `drift` 并以非零码退出，
//! 不把"重放不稳"包装成成功。
//!
//! 运行：
//! ```powershell
//! cd engine
//! $env:RUSTLUA_GIT_COMMIT = (git rev-parse HEAD)
//! cargo run --release -p engine --example wp0_capture_replay -- --frames 120 `
//!   --output target\reports\p3\capture-replay
//! ```
//!
//! 产物：`capture-a.json`、`capture-b.json`、`diff.json`。A/B 各自新建 `RuntimeScene`，
//! 输入由固定种子生成并写入日志，重放只读日志、不再调用生成器——证明的是"日志足以复现"，
//! 不是"同一个进程连续跑两次"。

use std::{env, fs, path::PathBuf, time::Instant};

use engine::host::{
    CaptureBundle, CaptureScenario, CapturedInput, DEFAULT_DRIFT_THRESHOLD_PCT, FingerprintBuilder,
    FrameStats, MemorySnapshot, RuntimeScene, TimingStat, fingerprint_bytes, now_unix_ms,
};
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_input::input::{InputLogEntry, InputSnapshot, KeyCode};
use rustlua_scene::scene::{Scene, SceneEntityId};

const VIEWPORT: (u32, u32) = (1280, 720);
const DEFAULT_FRAMES: usize = 120;
const WARMUP_FRAMES: usize = 5;
const DT: f32 = 1.0 / 60.0;
/// 5k 精灵让单帧稳定在毫秒量级，状态指纹覆盖 3 万个顶点。
const SPRITES: usize = 5_000;

struct Config {
    output: PathBuf,
    frames: usize,
    seed: u64,
    threshold_pct: f32,
}

struct Run {
    bundle: CaptureBundle,
}

fn main() {
    let config = parse_config();
    let examples = games_dir().join("examples");
    let root = examples.join("game_api");
    let scene_spec = format!("wp0-capture-replay:sprites={SPRITES}:grid=40:dt={DT}");
    let scene_hash = fingerprint_bytes(scene_spec.as_bytes());
    let commit = env::var("RUSTLUA_GIT_COMMIT").unwrap_or_else(|_| "unknown".to_owned());

    let first = run_capture(
        &root,
        &config,
        &scene_hash,
        &scene_spec,
        &commit,
        "capture-a",
        None,
    )
    .expect("capture A");
    // B 只读 A 的输入日志：重放能力必须由持久化数据支撑，而不是"同一个种子再生成一次"。
    let second = run_capture(
        &root,
        &config,
        &scene_hash,
        &scene_spec,
        &commit,
        "capture-b",
        Some(&first.bundle.scenario.input_log),
    )
    .expect("capture B");

    let diff = first.bundle.diff(&second.bundle, config.threshold_pct);
    fs::create_dir_all(&config.output).expect("create capture output directory");
    let path_a = config.output.join("capture-a.json");
    let path_b = config.output.join("capture-b.json");
    let path_diff = config.output.join("diff.json");
    fs::write(&path_a, first.bundle.to_json_pretty()).expect("write capture A");
    fs::write(&path_b, second.bundle.to_json_pretty()).expect("write capture B");
    let diff_json = serde_json::to_string_pretty(&diff).expect("diff is serializable");
    fs::write(&path_diff, &diff_json).expect("write diff");

    let state_equal = diff.state.equal;
    let input_equal = !diff
        .scenario_differences
        .iter()
        .any(|difference| difference.starts_with("input_log"));
    let systems = first.bundle.systems.len();
    let lua_heap = first
        .bundle
        .memory
        .lua_heap_bytes
        .map_or_else(|| "unavailable".to_owned(), |bytes| bytes.to_string());
    let frame_delta = diff
        .frames
        .delta_pct
        .map_or_else(|| "n/a".to_owned(), |pct| format!("{pct:+.3}"));
    println!(
        "wp0_capture_replay_ok report_version={} scenario={} frames={} warmup={WARMUP_FRAMES} \
         sprites={SPRITES} seed={} input_entries={} input_equal={} state_equal={} state={} \
         baseline_p50_ms={:.3} replayed_p50_ms={:.3} frame_delta_pct={} systems={} \
         slowest={} threshold_pct={:.1} lua_heap_bytes={} stable={} verdict={} \
         capture_a={} capture_b={} diff={}",
        engine::host::CAPTURE_REPORT_VERSION,
        first.bundle.scenario.name,
        config.frames,
        config.seed,
        first.bundle.scenario.input_log.len(),
        input_equal,
        state_equal,
        diff.state.baseline,
        diff.frames.baseline_p50_ms,
        diff.frames.candidate_p50_ms,
        frame_delta,
        systems,
        if diff.slowest.is_empty() {
            "none".to_owned()
        } else {
            diff.slowest.join(",")
        },
        config.threshold_pct,
        lua_heap,
        diff.stable,
        diff.verdict,
        path_a.display(),
        path_b.display(),
        path_diff.display()
    );
    if !diff.stable {
        eprintln!(
            "重放不稳定：scenario_differences={:?} state_equal={} frame_delta_pct={:?} \
             slowest={:?} added={:?} removed={:?}",
            diff.scenario_differences,
            diff.state.equal,
            diff.frames.delta_pct,
            diff.slowest,
            diff.added,
            diff.removed
        );
        std::process::exit(1);
    }
}

fn run_capture(
    root: &std::path::Path,
    config: &Config,
    scene_hash: &str,
    scene_spec: &str,
    commit: &str,
    label: &str,
    replay_log: Option<&[CapturedInput]>,
) -> Result<Run, String> {
    let mut runtime = RuntimeScene::from_scene_edit(Some(root), sprite_scene(SPRITES))
        .map_err(|error| format!("{label}: RuntimeScene::from_scene_edit: {error:#}"))?;
    let total_frames = WARMUP_FRAMES + config.frames;
    let (inputs, input_log) = match replay_log {
        Some(log) => {
            if log.len() != total_frames {
                return Err(format!(
                    "{label}: 输入日志 {} 条，期望 {total_frames} 条",
                    log.len()
                ));
            }
            let inputs = log.iter().map(input_from_log).collect::<Vec<_>>();
            (inputs, log.to_vec())
        }
        None => {
            let mut rng = config.seed;
            let inputs = (0..total_frames)
                .map(|frame| synthetic_input(&mut rng, frame))
                .collect::<Vec<_>>();
            let log = inputs
                .iter()
                .enumerate()
                .map(|(frame, input)| captured_input(frame as u32, input))
                .collect::<Vec<_>>();
            (inputs, log)
        }
    };

    let mut frame_samples = Vec::with_capacity(config.frames);
    let mut system_samples: std::collections::BTreeMap<String, Vec<f64>> =
        std::collections::BTreeMap::new();
    let mut fingerprint = FingerprintBuilder::new();
    for (frame, input) in inputs.iter().enumerate() {
        let started = Instant::now();
        runtime
            .run_script_steps(1, DT, input)
            .map_err(|error| format!("{label}: script step frame {frame}: {error:#}"))?;
        let mesh = runtime
            .vertices(VIEWPORT.0, VIEWPORT.1)
            .map_err(|error| format!("{label}: vertices frame {frame}: {error:#}"))?;
        let scene3d = runtime
            .scene_3d(Viewport::new(VIEWPORT.0 as f32, VIEWPORT.1 as f32).expect("viewport"));
        hash_frame(&mut fingerprint, frame, &mesh, scene3d.as_ref());
        if frame >= WARMUP_FRAMES {
            frame_samples.push(started.elapsed().as_secs_f64() * 1000.0);
            if let Some(report) = runtime.last_schedule_report() {
                for timing in &report.timings {
                    system_samples
                        .entry(timing.name.clone())
                        .or_default()
                        .push(timing.elapsed.as_secs_f64() * 1_000_000.0);
                }
            }
        }
        drop(scene3d);
        drop(mesh);
    }

    let systems = system_samples
        .into_iter()
        .filter_map(|(name, samples)| TimingStat::from_micros(name, &samples))
        .collect::<Vec<_>>();
    let bundle = CaptureBundle {
        report_version: engine::host::CAPTURE_REPORT_VERSION.to_owned(),
        engine_version: engine::host::ENGINE_VERSION.to_owned(),
        engine_commit: commit.to_owned(),
        captured_at_unix_ms: now_unix_ms(),
        scenario: CaptureScenario {
            name: format!("{label}:{scene_spec}"),
            scene_path: None,
            scene_hash: scene_hash.to_owned(),
            viewport: [VIEWPORT.0, VIEWPORT.1],
            fixed_dt_seconds: f64::from(DT),
            seed: config.seed,
            warmup_frames: WARMUP_FRAMES as u32,
            frames: config.frames as u32,
            input_log,
        },
        state_hash: fingerprint.finish_hex(),
        frames: FrameStats::from_millis(&frame_samples),
        memory: MemorySnapshot {
            process_memory_mib: None,
            lua_heap_bytes: Some(runtime.lua_memory_bytes()),
            texture_bytes: None,
            mesh_bytes: None,
            vertex_buffer_bytes: None,
            readback_cpu_bytes: None,
            asset_entries: Some(runtime.asset_count() as u64),
            scene_entities: Some(runtime.scene_entity_count() as u64),
            runtime_entities: Some(runtime.runtime_entity_count() as u64),
        },
        systems,
        gpu_passes: Vec::new(),
        gpu_available: false,
        gpu_note: Some(
            "headless 例子不创建渲染器；逐通道 GPU 时间由编辑器采集导出（E06-3）".to_owned(),
        ),
    };
    Ok(Run { bundle })
}

/// 用固定种子生成一帧输入；`frame` 参与相位，保证重放可重建。
///
/// 这里**不**把生成器实现当成重放依据：`scenario.input_log` 才是唯一输入来源，
/// 例子里的 B 也读这份日志（见 `main` 的 `input_log` 构造）。
fn synthetic_input(rng: &mut u64, frame: usize) -> InputSnapshot {
    let mut input = InputSnapshot::default();
    input.set_focused(true);
    let x = 960.0 + (next_u64(rng) % 200) as f64 - 100.0;
    let y = 540.0 + (next_u64(rng) % 200) as f64 - 100.0;
    input.cursor_moved(x, y);
    if frame.is_multiple_of(17) {
        input.scroll_event(0.0, 1.0);
    }
    let held = (frame / 13).is_multiple_of(2);
    input.key_event(KeyCode::KeyD, held);
    if frame.is_multiple_of(29) {
        input.mouse_event(rustlua_input::input::MouseButton::Left, true);
    } else if frame % 29 == 3 {
        input.mouse_event(rustlua_input::input::MouseButton::Left, false);
    }
    input
}

fn captured_input(frame: u32, input: &InputSnapshot) -> CapturedInput {
    let entry = input.log_entry(frame);
    CapturedInput {
        frame,
        cursor: [entry.cursor.0, entry.cursor.1],
        cursor_delta: [entry.cursor_delta.0, entry.cursor_delta.1],
        scroll: [entry.scroll.0, entry.scroll.1],
        focused: entry.focused,
        held_keys: entry.held_keys.into_iter().map(str::to_owned).collect(),
        pressed_keys: entry.pressed_keys.into_iter().map(str::to_owned).collect(),
        released_keys: entry.released_keys.into_iter().map(str::to_owned).collect(),
        virtual_keys: entry.virtual_keys.into_iter().map(str::to_owned).collect(),
        held_mouse: entry.held_mouse,
        pressed_mouse: entry.pressed_mouse,
        released_mouse: entry.released_mouse,
        held_gamepad: entry.held_gamepad,
        pressed_gamepad: entry.pressed_gamepad,
        released_gamepad: entry.released_gamepad,
        gamepad_axes: entry
            .gamepad_axes
            .into_iter()
            .map(|(axis, value)| [axis as f32, value])
            .collect(),
        press_cursor: [entry.press_cursor.0, entry.press_cursor.1],
        press_cursor_points: entry
            .press_cursor_points
            .into_iter()
            .map(|(x, y)| [x, y])
            .collect(),
    }
}

/// 持久化条目 → 输入快照；重放路径只走这一条，不调用种子生成器。
fn input_from_log(captured: &CapturedInput) -> InputSnapshot {
    let keys = |names: &[String]| {
        names
            .iter()
            .filter_map(|name| KeyCode::from_text(name).map(KeyCode::to_text))
            .collect::<Vec<_>>()
    };
    InputLogEntry {
        frame: captured.frame,
        cursor: (captured.cursor[0], captured.cursor[1]),
        cursor_delta: (captured.cursor_delta[0], captured.cursor_delta[1]),
        scroll: (captured.scroll[0], captured.scroll[1]),
        focused: captured.focused,
        held_keys: keys(&captured.held_keys),
        pressed_keys: keys(&captured.pressed_keys),
        released_keys: keys(&captured.released_keys),
        virtual_keys: keys(&captured.virtual_keys),
        held_mouse: captured.held_mouse.clone(),
        pressed_mouse: captured.pressed_mouse.clone(),
        released_mouse: captured.released_mouse.clone(),
        held_gamepad: captured.held_gamepad.clone(),
        pressed_gamepad: captured.pressed_gamepad.clone(),
        released_gamepad: captured.released_gamepad.clone(),
        gamepad_axes: captured
            .gamepad_axes
            .iter()
            .map(|[axis, value]| (*axis as u8, *value))
            .collect(),
        press_cursor: (captured.press_cursor[0], captured.press_cursor[1]),
        press_cursor_points: captured
            .press_cursor_points
            .iter()
            .map(|[x, y]| (*x, *y))
            .collect(),
    }
    .replay()
}

fn next_u64(state: &mut u64) -> u64 {
    // xorshift64*：小而确定，不依赖平台 RNG。
    let mut value = *state;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    *state = value;
    value.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

fn hash_frame(
    fingerprint: &mut FingerprintBuilder,
    frame: usize,
    mesh: &rustlua_render::frame::SceneMesh,
    scene3d: Option<&rustlua_render::frame::Scene3d>,
) {
    fingerprint.push_u64(frame as u64);
    fingerprint.push_u64(mesh.vertices.len() as u64);
    for vertex in &mesh.vertices {
        for value in vertex.position {
            fingerprint.push_f32(value);
        }
        for value in vertex.uv {
            fingerprint.push_f32(value);
        }
        for value in vertex.color {
            fingerprint.push_f32(value);
        }
        fingerprint.push_u64(u64::from(vertex.sdf));
    }
    fingerprint.push_u64(mesh.world_sprites.len() as u64);
    fingerprint.push_u64(mesh.paths.len() as u64);
    for path in &mesh.paths {
        fingerprint.push_bytes(path.as_bytes());
    }
    fingerprint.push_u64(scene3d.is_some() as u64);
    if let Some(scene3d) = scene3d {
        fingerprint.push_u64(scene3d.drawables.len() as u64);
        for drawable in &scene3d.drawables {
            fingerprint.push_u64(drawable.entity.0);
            for column in &drawable.model.columns {
                for value in column {
                    fingerprint.push_f32(*value);
                }
            }
        }
    }
}

fn parse_config() -> Config {
    let mut config = Config {
        output: PathBuf::from("target/reports/p3/capture-replay"),
        frames: DEFAULT_FRAMES,
        seed: 0xC0FF_EE01,
        threshold_pct: DEFAULT_DRIFT_THRESHOLD_PCT,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                config.output = PathBuf::from(args.next().expect("--output requires a directory"));
            }
            "--frames" => {
                config.frames = args
                    .next()
                    .expect("--frames requires an integer")
                    .parse()
                    .expect("--frames must be an integer");
            }
            "--seed" => {
                config.seed = args
                    .next()
                    .expect("--seed requires an integer")
                    .parse()
                    .expect("--seed must be an integer");
            }
            "--threshold" => {
                config.threshold_pct = args
                    .next()
                    .expect("--threshold requires a number")
                    .parse()
                    .expect("--threshold must be a number");
            }
            "--help" | "-h" => {
                println!(
                    "Usage: wp0_capture_replay [--frames N] [--seed N] [--threshold PCT] \
                     [--output DIR]"
                );
                std::process::exit(0);
            }
            other => panic!("unknown argument: {other}"),
        }
    }
    assert!(config.frames > 0, "--frames must be greater than zero");
    assert!(
        config.threshold_pct >= 0.0,
        "--threshold must not be negative"
    );
    config
}

/// 固定场景：`count` 个 Sprite 的网格排布（与 `wp0_profile_capture` 同族）。
fn sprite_scene(count: usize) -> Scene {
    let mut scene = Scene::new(format!("wp0-capture-replay-{count}"));
    for index in 0..count {
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
