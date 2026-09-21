use engine::{
    core::{
        scene::{Scene, SceneEntityId},
        tilemap::Tilemap,
        transform::Vec2,
    },
    host::RuntimeScene,
};
use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const REPORT_VERSION: &str = "wp0-v2";

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

#[derive(Debug)]
struct Config {
    width: usize,
    height: usize,
    cycles: usize,
    output: Option<PathBuf>,
}

fn parse_config() -> Config {
    let mut config = Config {
        width: 100,
        height: 100,
        cycles: 30,
        output: None,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--width" => {
                config.width = args
                    .next()
                    .unwrap_or_else(|| panic!("{flag} requires a value"))
                    .parse()
                    .expect("--width must be an integer")
            }
            "--height" => {
                config.height = args
                    .next()
                    .unwrap_or_else(|| panic!("{flag} requires a value"))
                    .parse()
                    .expect("--height must be an integer")
            }
            "--cycles" => {
                config.cycles = args
                    .next()
                    .unwrap_or_else(|| panic!("{flag} requires a value"))
                    .parse()
                    .expect("--cycles must be an integer")
            }
            "--output" => {
                config.output = Some(PathBuf::from(
                    args.next()
                        .unwrap_or_else(|| panic!("{flag} requires a value")),
                ))
            }
            "--help" | "-h" => {
                println!(
                    "Usage: tilemap_stress [--width N] [--height N] [--cycles N] [--output PATH]"
                );
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(config.width > 0, "--width must be greater than zero");
    assert!(config.height > 0, "--height must be greater than zero");
    assert!(config.cycles > 0, "--cycles must be greater than zero");
    config
}

fn build_scene(width: usize, height: usize) -> Scene {
    let mut scene = Scene::new(format!("tilemap-{width}x{height}"));
    scene
        .add_entity(SceneEntityId(1), "Map", None)
        .expect("add tilemap entity");
    let entity = scene
        .entities
        .get_mut(&SceneEntityId(1))
        .expect("tilemap entity exists");
    entity.components.insert(
        "Transform".to_owned(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
    );

    let mut tilemap = Tilemap::new(Vec2::new(32.0, 32.0), width, height).expect("tilemap");
    tilemap.texture = "assets/textures/slime.png".to_owned();
    for y in 0..height {
        for x in 0..width {
            let cell = ((x + y) % 6 + 1) as u8;
            tilemap.set_cell(x, y, cell).expect("set tile");
            if (x + y) % 7 == 0 {
                tilemap.set_solid(x, y, true).expect("set solid");
            }
        }
    }
    entity
        .components
        .insert("Tilemap".to_owned(), tilemap.to_component_str());
    scene
}

fn main() {
    let config = parse_config();
    let scene = build_scene(config.width, config.height);
    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");

    let mut samples = Vec::with_capacity(config.cycles);
    let mut last_vertices = 0usize;
    let mut last_draws = 0usize;
    for _ in 0..config.cycles {
        let started = Instant::now();
        let mesh = runtime.vertices(1280, 720).expect("build vertices");
        samples.push(started.elapsed());
        last_vertices = mesh.vertices.len();
        last_draws = mesh.draws.len();
    }
    let stats = summarize(samples);
    let instances = last_vertices / 6;
    let triangles = instances * 2;
    println!(
        "tilemap_stress_ok report_version={REPORT_VERSION} width={} height={} cycles={} p50_us={} p95_us={} p99_us={} vertices={} draws={} instances={} triangles={}",
        config.width,
        config.height,
        config.cycles,
        stats.p50_us,
        stats.p95_us,
        stats.p99_us,
        last_vertices,
        last_draws,
        instances,
        triangles
    );
    if let Some(path) = config.output.as_ref() {
        write_report(
            path,
            &config,
            stats,
            last_vertices,
            last_draws,
            instances,
            triangles,
        );
    }
}

fn summarize(mut values: Vec<Duration>) -> Stats {
    values.sort_unstable();
    let percentile =
        |percent: usize| ((values.len() * percent).saturating_sub(1) / 100).min(values.len() - 1);
    Stats {
        count: values.len(),
        min_us: values[0].as_micros(),
        average_us: values.iter().map(Duration::as_micros).sum::<u128>() / values.len() as u128,
        p50_us: values[percentile(50)].as_micros(),
        p95_us: values[percentile(95)].as_micros(),
        p99_us: values[percentile(99)].as_micros(),
        max_us: values[values.len() - 1].as_micros(),
    }
}

fn write_report(
    path: &PathBuf,
    config: &Config,
    stats: Stats,
    vertices: usize,
    draws: usize,
    instances: usize,
    triangles: usize,
) {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create report directory");
    }
    let json = format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"benchmark\": \"tilemap_stress\",\n  \"width\": {},\n  \"height\": {},\n  \"cycles\": {},\n  \"frame_us\": {},\n  \"render\": {{\"draw_calls\":{},\"instances\":{},\"triangles\":{},\"vertices\":{},\"cpu_frame_us\":null,\"gpu_frame_us\":null}}\n}}\n",
        config.width,
        config.height,
        config.cycles,
        stats_json(stats),
        draws,
        instances,
        triangles,
        vertices
    );
    fs::write(path, json).expect("write tilemap report");
}

fn stats_json(stat: Stats) -> String {
    format!(
        "{{\"count\":{},\"min_us\":{},\"average_us\":{},\"p50_us\":{},\"p95_us\":{},\"p99_us\":{},\"max_us\":{}}}",
        stat.count,
        stat.min_us,
        stat.average_us,
        stat.p50_us,
        stat.p95_us,
        stat.p99_us,
        stat.max_us
    )
}
