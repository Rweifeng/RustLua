//! WP0 增量基准：场景装载、typed 编译与运行时帧分配（`wp0-v4`）。
//!
//! 为什么需要它：WP1（typed scene）与 WP6（提取快照）真正要替换的是**每帧的分配与
//! 克隆**，而不是字符串解析——运行帧早已不解析组件字符串（`from_component_str` 在
//! 帧内为 0）。因此本基准把**装载期成本**与**帧内分配**分开度量，作为这两个工作包
//! 以及 WP3（ECS 热存储）的前后对照量尺。
//!
//! 帧的定义与运行时一致：`run_script_steps` + `vertices` + `scene_3d`，并计入析构
//! 成本（渲染器与 Lua 看到的正是这一步的产物）。
//!
//! 运行：
//! ```powershell
//! cd engine
//! cargo run --release -p engine --example wp0_scene_frame -- --cycles 30 `
//!   --output ..\artifacts\baselines\wp0-scene-frame.json
//! ```

#[path = "support/wp0_alloc.rs"]
mod wp0_alloc;

use engine::core::{
    games_dir,
    input::InputSnapshot,
    math3d::Vec3,
    par::Parallelism,
    particle::ParticleEmitter,
    scene::{Scene, SceneEntityId},
    scene_extract::extract_serialized_scene_3d,
    spatial_transform::{LocalTransform, TransformTree},
    transform::Vec2 as SceneVec2,
    typed_scene::TypedScene,
};
use engine::host::RuntimeScene;
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const REPORT_VERSION: &str = "wp0-v4";
const VIEWPORT: (u32, u32) = (1280, 720);
const FRAME_WARMUP: usize = 3;
const PROJECTS: [&str; 2] = ["game_api", "game_jhyt"];
const SPRITE_STAIRS: [usize; 3] = [1_000, 10_000, 50_000];
/// Tilemap 阶梯（边长，单位格）：32×32 = 1,024 格、100×100 = 10,000 格。
const TILEMAP_STAIRS: [usize; 2] = [32, 100];
/// 粒子阶梯：docs/40 §8 的目标是 1 万 CPU 粒子 <4 ms。
///
/// 发射点取默认 2D 相机中心（`DEFAULT_2D_CAMERA_CENTER`）= 1280×720 视口的正中心，因此粒子
/// 全部落在视口内：粒子在批处理里就是精灵，也会走视口剔除，发在原点（屏幕角落）会让一部分
/// 被剔除，于是"顶点数 == 粒子数 × 6"这把量尺就不再量它声称要量的东西。
const PARTICLE_STAIRS: [usize; 2] = [1_000, 10_000];
const PARTICLE_ORIGIN: SceneVec2 = SceneVec2::new(640.0, 360.0);
const DEPTH_STAIRS: [usize; 3] = [1, 10, 100];
/// 精灵剔除 A/B 的规模：1 万档看顶点生成，5 万档看**收集阶段**（剔除前那 5 万次 push
/// 与随后对 5 万项的排序才是大头）。
const SPRITE_CULLING_AB_SPRITES: [usize; 2] = [10_000, 50_000];
const DEPTH_CYCLES: usize = 200;

#[derive(Debug, Clone, Copy)]
struct Stats {
    count: usize,
    min_us: u128,
    average_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    max_us: u128,
}

struct Config {
    output: Option<PathBuf>,
    cycles: usize,
    /// 每精灵顶点生成的并行度（WP8）。`off` = 强制串行（回滚档）。
    sprite_parallelism: Parallelism,
}

fn main() {
    let config = parse_config();
    let examples = games_dir().join("examples");
    let scene_entries = measure_scenes(&examples, config.cycles);
    let frame_stairs = SPRITE_STAIRS
        .iter()
        .map(|count| measure_frame(&examples, *count, config.cycles, config.sprite_parallelism))
        .collect::<Vec<_>>();
    let depth_stairs = DEPTH_STAIRS
        .iter()
        .map(|depth| measure_depth(*depth, DEPTH_CYCLES))
        .collect::<Vec<_>>();
    let tilemap_stairs = TILEMAP_STAIRS
        .iter()
        .map(|side| measure_tilemap_frame(&examples, *side, config.cycles))
        .collect::<Vec<_>>();
    let sprite_culling_ab = SPRITE_CULLING_AB_SPRITES
        .iter()
        .map(|sprites| measure_sprite_culling_ab(&examples, *sprites, config.cycles))
        .collect::<Vec<_>>()
        .join(",\n    ");
    let tilemap_edit_stairs = TILEMAP_STAIRS
        .iter()
        .map(|side| measure_tilemap_edit(&examples, *side, config.cycles))
        .collect::<Vec<_>>();
    let particle_stairs = PARTICLE_STAIRS
        .iter()
        .map(|count| measure_particle_frame(&examples, *count, config.cycles))
        .collect::<Vec<_>>();
    let json = format_report(
        &scene_entries,
        &frame_stairs,
        &depth_stairs,
        &tilemap_stairs,
        &particle_stairs,
        &tilemap_edit_stairs,
        &sprite_culling_ab,
        &config,
    );
    if let Some(path) = &config.output {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(path, &json).expect("write wp0-v4 report");
        println!("report: {}", path.display());
    } else {
        print!("{json}");
    }
    let scenes = scene_entries.len();
    println!(
        "wp0_scene_frame_ok report_version={REPORT_VERSION} scenes={scenes} stairs={} depth_stairs={}",
        frame_stairs.len(),
        depth_stairs.len()
    );
}

fn parse_config() -> Config {
    let mut output = None;
    let mut cycles = 30usize;
    let mut sprite_parallelism = Parallelism::detected();
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ));
            }
            "--sprite-parallelism" => {
                sprite_parallelism = match args
                    .next()
                    .expect("--sprite-parallelism requires a value")
                    .as_str()
                {
                    "off" => Parallelism::SEQUENTIAL,
                    "auto" => Parallelism::detected(),
                    other => panic!("unknown --sprite-parallelism value: {other}"),
                };
            }
            "--cycles" => {
                cycles = args
                    .next()
                    .expect("--cycles requires a value")
                    .parse()
                    .expect("--cycles must be an integer");
            }
            "--help" | "-h" => {
                println!("Usage: wp0_scene_frame [--cycles N] [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(cycles > 0, "--cycles must be greater than zero");
    Config {
        output,
        cycles,
        sprite_parallelism,
    }
}

/// 逐场景采样：`Scene::load`（磁盘 → 字符串组件）与 `TypedScene::compile_v2`（字符串
/// → typed）分开计时。两者都在加载边界，不在帧内。
///
/// `compile_v2` 严格模式在任何诊断下都会返回 `Err`，因此能走到这里即意味着
/// `diagnostics == 0`——报告里的该字段是这一事实的记录，不是独立统计。
fn measure_scenes(examples: &std::path::Path, cycles: usize) -> Vec<String> {
    let mut reports = Vec::new();
    for project in PROJECTS {
        let dir = examples.join(project).join("scenes");
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("读取场景目录失败 {}: {error}", dir.display()));
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("scene"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let mut load_samples = Vec::with_capacity(cycles);
            let mut compile_samples = Vec::with_capacity(cycles);
            let mut legacy_samples = Vec::with_capacity(cycles);
            let mut entities = 0usize;
            let mut typed_components = 0usize;
            let mut legacy_drawables = 0usize;
            for _ in 0..cycles {
                let started = Instant::now();
                let scene = Scene::load(&path)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                load_samples.push(started.elapsed());

                let started = Instant::now();
                let typed = TypedScene::compile_v2(&scene)
                    .unwrap_or_else(|errors| panic!("{} 编译诊断：{errors:?}", path.display()));
                compile_samples.push(started.elapsed());

                // 编辑器 3D 视图在 WP1-4 之前每次重绘都调用这条路径；同一份 Scene 上
                // 直接对比，作为"typed 编译是否比旧提取更便宜"的 A/B 证据。
                let started = Instant::now();
                let legacy = extract_serialized_scene_3d(&scene);
                legacy_samples.push(started.elapsed());

                entities = typed.entities.len();
                typed_components = typed
                    .entities
                    .iter()
                    .map(|entity| entity.components.len())
                    .sum();
                legacy_drawables = legacy.entities.len();
            }
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_owned();
            reports.push(format!(
                "{{\"project\":\"{project}\",\"scene\":\"{name}\",\"entities\":{entities},\
                 \"typed_components\":{typed_components},\"diagnostics\":0,\
                 \"legacy_drawables\":{legacy_drawables},\
                 \"load_us\":{},\"typed_compile_us\":{},\"legacy_extract_us\":{}}}",
                stats_json(&summarize(load_samples)),
                stats_json(&summarize(compile_samples)),
                stats_json(&summarize(legacy_samples))
            ));
        }
    }
    reports
}

/// 帧阶梯：合成 N 个 Sprite 实体的运行时场景，测稳态帧的时间与分配。
///
/// `sprite_parallelism` 只是把同一个运行时的顶点生成换成串行或并行（WP8 A/B）：
/// 两条路径输出逐字节相同，因此这条阶梯的差异就是并行带来的净效果。
fn measure_frame(
    examples: &std::path::Path,
    sprites: usize,
    cycles: usize,
    sprite_parallelism: Parallelism,
) -> String {
    let root = examples.join("game_api");
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), sprite_scene(sprites))
        .expect("runtime scene for the frame stair");
    runtime.set_sprite_parallelism(sprite_parallelism);
    let input = InputSnapshot::default();
    for _ in 0..FRAME_WARMUP {
        run_frame(&mut runtime, &input);
    }
    let mut frame_samples = Vec::with_capacity(cycles);
    let mut alloc_counts = Vec::with_capacity(cycles);
    let mut alloc_bytes = Vec::with_capacity(cycles);
    let mut script_alloc_counts = Vec::with_capacity(cycles);
    let mut vertices_alloc_counts = Vec::with_capacity(cycles);
    let mut scene3d_alloc_counts = Vec::with_capacity(cycles);
    let mut peak_delta = 0u64;
    let mut vertices = 0usize;
    let mut draws = 0usize;
    for _ in 0..cycles {
        let sample = run_frame(&mut runtime, &input);
        frame_samples.push(sample.elapsed);
        alloc_counts.push(sample.alloc_count);
        alloc_bytes.push(sample.alloc_bytes);
        script_alloc_counts.push(sample.script_alloc_count);
        vertices_alloc_counts.push(sample.vertices_alloc_count);
        scene3d_alloc_counts.push(sample.scene3d_alloc_count);
        peak_delta = peak_delta.max(sample.peak_delta);
        vertices = sample.vertices;
        draws = sample.draws;
    }
    let stats = summarize(frame_samples);
    format!(
        "{{\"sprites\":{sprites},\"samples\":{},\"frame_us\":{},\
         \"alloc_count_p50\":{},\"alloc_count_max\":{},\
         \"alloc_bytes_p50\":{},\"alloc_bytes_max\":{},\"alloc_peak_delta_bytes\":{},\
         \"script_alloc_count_p50\":{},\"vertex_alloc_count_p50\":{},\"scene3d_alloc_count_p50\":{},\
         \"extract_alloc_count_p50\":{},\
         \"vertices\":{vertices},\"draws\":{draws}}}",
        stats.count,
        stats_json(&stats),
        median_u64(&alloc_counts),
        alloc_counts.iter().copied().max().unwrap_or(0),
        median_u64(&alloc_bytes),
        alloc_bytes.iter().copied().max().unwrap_or(0),
        peak_delta,
        median_u64(&script_alloc_counts),
        median_u64(&vertices_alloc_counts).saturating_sub(median_u64(&script_alloc_counts)),
        median_u64(&scene3d_alloc_counts).saturating_sub(median_u64(&vertices_alloc_counts)),
        median_u64(&alloc_counts).saturating_sub(median_u64(&script_alloc_counts))
    )
}

struct FrameSample {
    elapsed: Duration,
    alloc_count: u64,
    alloc_bytes: u64,
    peak_delta: u64,
    /// 步骤管线（物理 / UI 命中 / Lua）阶段内的分配次数。
    script_alloc_count: u64,
    /// `vertices()` 阶段内的分配次数（含之前的步骤累计，用差值算）。
    vertices_alloc_count: u64,
    /// `scene_3d()` 结束时的累计分配次数。
    scene3d_alloc_count: u64,
    vertices: usize,
    draws: usize,
}

fn run_frame(runtime: &mut RuntimeScene, input: &InputSnapshot) -> FrameSample {
    wp0_alloc::reset_window();
    let started = Instant::now();
    runtime
        .run_script_steps(1, 1.0 / 60.0, input)
        .expect("script step");
    // 步骤管线（物理 / UI 命中 / Lua）与提取分开计数：只报帧总量无法判断该优化哪一段。
    let script_alloc_count = wp0_alloc::window_count();
    let mesh = runtime
        .vertices(VIEWPORT.0, VIEWPORT.1)
        .expect("2D extraction");
    let vertices_alloc_count = wp0_alloc::window_count();
    let scene3d = runtime.scene_3d(VIEWPORT.0 as f32 / VIEWPORT.1 as f32);
    let scene3d_alloc_count = wp0_alloc::window_count();
    // 渲染器消费完这一帧的产物才算帧结束，因此析构成本计入帧时间。
    let vertices = mesh.vertices.len();
    let draws = mesh.draws.len();
    drop(scene3d);
    drop(mesh);
    let elapsed = started.elapsed();
    FrameSample {
        elapsed,
        alloc_count: wp0_alloc::window_count(),
        alloc_bytes: wp0_alloc::window_bytes(),
        peak_delta: wp0_alloc::window_peak_delta_bytes(),
        script_alloc_count,
        vertices_alloc_count,
        scene3d_alloc_count,
        vertices,
        draws,
    }
}

/// 合成 `sprites` 个 Sprite 实体；布局与 `sprite_stress` 同族的网格排布。
fn sprite_scene(sprites: usize) -> Scene {
    let mut scene = Scene::new(format!("wp0-frame-{sprites}"));
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
            format!("x={x};y={y};rotation=0;scale_x=1;scale_y=1"),
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

/// Tilemap 帧阶梯：一张 `side × side` 的地图，测稳态帧的时间与分配。
///
/// 关注点：缓存命中时不应为整张地图付克隆代价（地图越大越明显）。
fn measure_tilemap_frame(examples: &std::path::Path, side: usize, cycles: usize) -> String {
    let root = examples.join("game_api");
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), tilemap_scene(side))
        .expect("runtime scene for the tilemap stair");
    let input = InputSnapshot::default();
    for _ in 0..FRAME_WARMUP {
        run_frame(&mut runtime, &input);
    }
    let mut frame_samples = Vec::with_capacity(cycles);
    let mut alloc_counts = Vec::with_capacity(cycles);
    let mut alloc_bytes = Vec::with_capacity(cycles);
    let mut vertices = 0usize;
    let mut draws = 0usize;
    for _ in 0..cycles {
        let sample = run_frame(&mut runtime, &input);
        frame_samples.push(sample.elapsed);
        alloc_counts.push(sample.alloc_count);
        alloc_bytes.push(sample.alloc_bytes);
        vertices = sample.vertices;
        draws = sample.draws;
    }
    format!(
        "{{\"side\":{side},\"tiles\":{},\"samples\":{},\"frame_us\":{},\
         \"alloc_count_p50\":{},\"alloc_bytes_p50\":{},\"vertices\":{vertices},\"draws\":{draws}}}",
        side * side,
        frame_samples.len(),
        stats_json(&summarize(frame_samples)),
        median_u64(&alloc_counts),
        median_u64(&alloc_bytes)
    )
}

/// 合成一张 `side × side` 的 Tilemap 场景（单一实体，带 Transform + Tilemap）。
fn tilemap_scene(side: usize) -> Scene {
    let mut scene = Scene::new(format!("wp0-tilemap-{side}"));
    let id = SceneEntityId(1);
    scene
        .add_entity(id, "Ground", None)
        .expect("add tilemap entity");
    let tiles: String = (0..side * side)
        .map(|index| char::from(b'1' + (index % 4) as u8))
        .collect();
    let solid: String = (0..side * side)
        .map(|index| if index % 7 == 0 { '1' } else { '0' })
        .collect();
    let entity = scene.entities.get_mut(&id).expect("tilemap entity");
    entity.components.insert(
        "Transform".into(),
        "x=0;y=0;rotation=0;scale_x=1;scale_y=1".into(),
    );
    entity.components.insert(
        "Tilemap".into(),
        format!(
            "tile_size=32x32;width={side};height={side};\
             texture=assets/textures/tileset.png;tiles={tiles};solid={solid}"
        ),
    );
    scene
}

/// 粒子帧阶梯：持续发射器（每帧补足到 `count` 个）下的稳态帧成本。
///
/// 每帧先补足再计时，因此补足本身不计入窗口——窗口里只有
/// `run_script_steps`（含粒子推进）+ `vertices`（含粒子四边形）。
fn measure_particle_frame(examples: &std::path::Path, count: usize, cycles: usize) -> String {
    let root = examples.join("game_api");
    let mut runtime =
        RuntimeScene::from_scene_edit(Some(root.as_path()), Scene::new("wp0-particles"))
            .expect("runtime scene for the particle stair");
    let input = InputSnapshot::default();
    let emitter = ParticleEmitter {
        count: 1,
        speed: 60.0,
        life: 1.5,
        size_start: 6.0,
        size_end: 0.0,
        color_start: [1.0, 0.9, 0.6, 1.0],
        color_end: [1.0, 0.5, 0.2, 0.0],
        ..ParticleEmitter::default()
    };
    let top_up = |runtime: &mut RuntimeScene| {
        let live = runtime.with_script(|script| script.particle_count());
        if live < count {
            let mut emitter = emitter;
            emitter.count = count - live;
            runtime.with_script(|script| {
                // 发在**视口中心**而不是世界原点：粒子在批处理里就是精灵，会走视口剔除，
                // 发在原点（屏幕角落）会让一部分粒子被剔除，于是"顶点数 == 粒子数 × 6"
                // 不再成立——那把量尺就不再量它声称要量的东西了。
                script.burst_particles(PARTICLE_ORIGIN, &emitter);
            });
        }
    };
    for _ in 0..FRAME_WARMUP {
        top_up(&mut runtime);
        run_frame(&mut runtime, &input);
    }
    let mut frame_samples = Vec::with_capacity(cycles);
    let mut alloc_counts = Vec::with_capacity(cycles);
    let mut alloc_bytes = Vec::with_capacity(cycles);
    let mut vertices = 0usize;
    let mut draws = 0usize;
    for _ in 0..cycles {
        top_up(&mut runtime);
        let sample = run_frame(&mut runtime, &input);
        frame_samples.push(sample.elapsed);
        alloc_counts.push(sample.alloc_count);
        alloc_bytes.push(sample.alloc_bytes);
        vertices = sample.vertices;
        draws = sample.draws;
    }
    let live = runtime.with_script(|script| script.particle_count());
    format!(
        "{{\"particles\":{count},\"live\":{live},\"samples\":{},\"frame_us\":{},\
         \"alloc_count_p50\":{},\"alloc_bytes_p50\":{},\"vertices\":{vertices},\"draws\":{draws}}}",
        frame_samples.len(),
        stats_json(&summarize(frame_samples)),
        median_u64(&alloc_counts),
        median_u64(&alloc_bytes)
    )
}

/// 单元格编辑阶梯：**改一格之后再渲染一帧**的成本（WP4 ④ 的量尺）。
///
/// docs/40 §5 给 Tilemap 的目标是"接口更新目标 <4ms"。稳态帧（缓存命中）早就有量尺了，缺的是
/// "编辑后那一帧"：缓存会失效并重建这一层网格，代价与**地图格数**成正比还是与**改动格数**
/// 成正比，是 chunk 化该不该做的唯一依据。因此这里每轮只改一格，再测这一帧。
fn measure_tilemap_edit(examples: &std::path::Path, side: usize, cycles: usize) -> String {
    let root = examples.join("game_api");
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), tilemap_scene(side))
        .expect("runtime scene for the tilemap edit stair");
    let input = InputSnapshot::default();
    for _ in 0..FRAME_WARMUP {
        run_frame(&mut runtime, &input);
    }
    let mut edit_samples = Vec::with_capacity(cycles);
    let mut steady_samples = Vec::with_capacity(cycles);
    let mut vertices = 0usize;
    for cycle in 0..cycles {
        // 稳态帧（无编辑）作为同轮对照：差值就是"这次编辑"的净代价。
        let steady = run_frame(&mut runtime, &input);
        steady_samples.push(steady.elapsed);
        // 每轮改不同的一格，避免命中任何"同一格重复改"的优化。
        let x = cycle % side;
        let y = (cycle / side) % side;
        let value = 1 + (cycle % 3) as u8;
        assert!(
            runtime
                .set_tilemap_cell(SceneEntityId(1), x, y, value)
                .expect("editing one tile cell"),
            "tilemap entity must exist"
        );
        let edited = run_frame(&mut runtime, &input);
        edit_samples.push(edited.elapsed);
        vertices = edited.vertices;
    }
    let steady = summarize(steady_samples);
    let edited = summarize(edit_samples);
    format!(
        "{{\"side\":{side},\"tiles\":{},\"samples\":{},\
         \"steady_frame_us\":{},\"edited_frame_us\":{},\
         \"edit_cost_p50_us\":{},\"vertices\":{vertices}}}",
        side * side,
        edited.count,
        stats_json(&steady),
        stats_json(&edited),
        edited.p50_us.saturating_sub(steady.p50_us)
    )
}

/// 精灵剔除的**同进程交替 A/B**（WP6 CPU 可见性）。
///
/// 跨运行的帧时间噪声约 ±15–25%（docs/53 §3.5 的阴性对照），因此这里在同一进程里逐轮交替
/// 关/开剔除并分别采样；同时记录两侧顶点数，保证"确实剔掉了东西"而不是空转。
fn measure_sprite_culling_ab(examples: &std::path::Path, sprites: usize, cycles: usize) -> String {
    let root = examples.join("game_api");
    let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), sprite_scene(sprites))
        .expect("runtime scene for the sprite culling A/B");
    let input = InputSnapshot::default();
    for _ in 0..FRAME_WARMUP {
        run_frame(&mut runtime, &input);
    }
    let mut full_samples = Vec::with_capacity(cycles);
    let mut culled_samples = Vec::with_capacity(cycles);
    let mut full_vertices = 0usize;
    let mut culled_vertices = 0usize;
    for _ in 0..cycles {
        runtime.set_sprite_culling(false);
        let full = run_frame(&mut runtime, &input);
        full_samples.push(full.elapsed);
        full_vertices = full.vertices;
        runtime.set_sprite_culling(true);
        let culled = run_frame(&mut runtime, &input);
        culled_samples.push(culled.elapsed);
        culled_vertices = culled.vertices;
    }
    format!(
        "{{\"sprites\":{sprites},\"samples\":{},\
         \"full_frame_us\":{},\"culled_frame_us\":{},\
         \"full_vertices\":{full_vertices},\"culled_vertices\":{culled_vertices}}}",
        culled_samples.len(),
        stats_json(&summarize(full_samples)),
        stats_json(&summarize(culled_samples))
    )
}

/// 报告字段逐段拼接。参数确实多（每段阶梯一个），但它们只是待格式化的字符串片段，
/// 为它们专门造一个结构体只会把同样的名字再抄一遍。
#[allow(clippy::too_many_arguments)]
fn format_report(
    scene_entries: &[String],
    frame_stairs: &[String],
    depth_stairs: &[String],
    tilemap_stairs: &[String],
    particle_stairs: &[String],
    tilemap_edit_stairs: &[String],
    sprite_culling_ab: &str,
    config: &Config,
) -> String {
    let mut json = String::new();
    json.push_str(&format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"benchmark\": \"scene_frame\",\n  \
         \"target\": {{\"cycles\": {}, \"viewport\": [{}, {}], \"frame_warmup\": {}, \"depth_cycles\": {DEPTH_CYCLES}}},\n",
        config.cycles, VIEWPORT.0, VIEWPORT.1, FRAME_WARMUP
    ));
    json.push_str(&format!(
        "  \"scene_load\": {{\"scenes\": {}, \"entries\": [\n    {}\n  ]}},\n",
        scene_entries.len(),
        scene_entries.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"frame_stairs\": [\n    {}\n  ],\n",
        frame_stairs.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"transform_depth\": [\n    {}\n  ],\n",
        depth_stairs.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"tilemap_stairs\": [\n    {}\n  ],\n",
        tilemap_stairs.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"particle_stairs\": [\n    {}\n  ],\n",
        particle_stairs.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"tilemap_edit_stairs\": [\n    {}\n  ],\n",
        tilemap_edit_stairs.join(",\n    ")
    ));
    json.push_str(&format!(
        "  \"sprite_culling_ab\": [{sprite_culling_ab}]\n}}\n"
    ));
    json
}

/// 深度阶梯：`depth` 长父链上对比"全量重算"与"增量脏传播"。
///
/// 叶子变动只需重算 1 个节点，根变动必须重算整条链——两者分开报告，避免用叶子
/// 数字冒充"增量总是 O(1)"。叶子改动放在计时窗口之外（窗口只量传播本身）。
fn measure_depth(depth: usize, cycles: usize) -> String {
    let tip = SceneEntityId(depth as u64);
    let mut tree = build_chain(depth);
    tree.recompute_all().expect("initial propagation");

    let mut full = Vec::with_capacity(cycles);
    let mut full_allocs = Vec::with_capacity(cycles);
    for _ in 0..cycles {
        wp0_alloc::reset_window();
        let started = Instant::now();
        tree.recompute_all().expect("full propagation");
        full.push(started.elapsed());
        full_allocs.push(wp0_alloc::window_count());
    }

    let mut leaf = Vec::with_capacity(cycles);
    let mut leaf_allocs = Vec::with_capacity(cycles);
    let mut leaf_recomputes = 0;
    for index in 0..cycles {
        tree.set_local(tip, translated(1.0 + index as f32));
        tree.reset_recomputes();
        wp0_alloc::reset_window();
        let started = Instant::now();
        tree.world_position(tip).expect("incremental propagation");
        leaf.push(started.elapsed());
        leaf_allocs.push(wp0_alloc::window_count());
        leaf_recomputes = tree.recomputes();
    }

    let mut root = Vec::with_capacity(cycles);
    let mut root_allocs = Vec::with_capacity(cycles);
    let mut root_recomputes = 0;
    for index in 0..cycles {
        tree.set_local(SceneEntityId(1), translated(1.0 + index as f32));
        tree.reset_recomputes();
        wp0_alloc::reset_window();
        let started = Instant::now();
        tree.world_position(tip).expect("root-change propagation");
        root.push(started.elapsed());
        root_allocs.push(wp0_alloc::window_count());
        root_recomputes = tree.recomputes();
    }

    format!(
        "{{\"depth\":{depth},\"samples\":{cycles},\
         \"full_ns\":{},\"incremental_leaf_ns\":{},\"incremental_root_ns\":{},\
         \"full_recomputes\":{depth},\"leaf_recomputes\":{leaf_recomputes},\
         \"root_recomputes\":{root_recomputes},\
         \"full_alloc_p50\":{},\"leaf_alloc_p50\":{},\"root_alloc_p50\":{}}}",
        stats_ns_json(&summarize_ns(full)),
        stats_ns_json(&summarize_ns(leaf)),
        stats_ns_json(&summarize_ns(root)),
        median_u64(&full_allocs),
        median_u64(&leaf_allocs),
        median_u64(&root_allocs),
    )
}

fn build_chain(depth: usize) -> TransformTree {
    let mut tree = TransformTree::new();
    for index in 0..depth {
        let id = SceneEntityId(index as u64 + 1);
        tree.set_local(id, translated(1.0));
        if index > 0 {
            tree.set_parent(id, Some(SceneEntityId(index as u64)))
                .expect("chain parent");
        }
    }
    tree
}

fn translated(x: f32) -> LocalTransform {
    LocalTransform {
        translation: Vec3::new(x, 0.0, 0.0),
        ..LocalTransform::identity()
    }
}

fn summarize(values: Vec<Duration>) -> Stats {
    summarize_with(values, |value| value.as_micros())
}

/// 深度阶梯用纳秒：深度 1 与增量叶子变动的单次耗时不足 1 µs，用微秒会被截断成 0，
/// 那样既看不出差异，也会让"必须为正"的门变成误报。
fn summarize_ns(values: Vec<Duration>) -> Stats {
    summarize_with(values, |value| value.as_nanos())
}

fn summarize_with(mut values: Vec<Duration>, scale: impl Fn(Duration) -> u128) -> Stats {
    values.sort_unstable();
    let at = |p: usize| ((values.len() * p).saturating_sub(1) / 100).min(values.len() - 1);
    Stats {
        count: values.len(),
        min_us: scale(values[0]),
        average_us: values.iter().map(|value| scale(*value)).sum::<u128>() / values.len() as u128,
        p50_us: scale(values[at(50)]),
        p95_us: scale(values[at(95)]),
        p99_us: scale(values[at(99)]),
        max_us: scale(values[values.len() - 1]),
    }
}

fn stats_json(stats: &Stats) -> String {
    stats_json_unit(stats, "us")
}

fn stats_ns_json(stats: &Stats) -> String {
    stats_json_unit(stats, "ns")
}

fn stats_json_unit(stats: &Stats, unit: &str) -> String {
    format!(
        "{{\"count\":{},\"min_{unit}\":{},\"average_{unit}\":{},\"p50_{unit}\":{},\
         \"p95_{unit}\":{},\"p99_{unit}\":{},\"max_{unit}\":{}}}",
        stats.count,
        stats.min_us,
        stats.average_us,
        stats.p50_us,
        stats.p95_us,
        stats.p99_us,
        stats.max_us
    )
}

fn median_u64(values: &[u64]) -> u64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}
