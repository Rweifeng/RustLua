//! Reproducible ECS workload for the M2-07 performance baseline.

use std::{
    env, fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use engine::core::{
    commands::CommandQueue,
    transform::{Transform, Vec2, Velocity, integrate_velocity},
    world::World,
};

const DEFAULT_ENTITIES: usize = 10_000;
const DEFAULT_SECONDS: u64 = 60;
const REPLACE_BATCH: usize = 100;
const REPORT_VERSION: &str = "m2-07-v1";

#[derive(Debug)]
struct Config {
    entities: usize,
    seconds: u64,
    output: Option<PathBuf>,
}

#[derive(Default)]
struct Samples {
    create: Vec<Duration>,
    query: Vec<Duration>,
    update: Vec<Duration>,
    destroy: Vec<Duration>,
    command: Vec<Duration>,
}

#[derive(Debug)]
struct Stats {
    count: usize,
    min_us: u128,
    average_us: u128,
    p95_us: u128,
    max_us: u128,
}

fn main() {
    let config = parse_config();
    let started = Instant::now();
    let (cycles, final_count, min_count, max_count, samples) = run(&config);
    let elapsed = started.elapsed();
    println!("M2-07 ECS stress baseline ({REPORT_VERSION})");
    println!(
        "entities={} requested_seconds={} elapsed_ms={}",
        config.entities,
        config.seconds,
        elapsed.as_millis()
    );
    println!(
        "cycles={} final_entities={} entity_range={}..{}",
        cycles, final_count, min_count, max_count
    );
    let stats = [
        ("create", summarize(samples.create)),
        ("query", summarize(samples.query)),
        ("update", summarize(samples.update)),
        ("destroy", summarize(samples.destroy)),
        ("command", summarize(samples.command)),
    ];
    for (name, stat) in &stats {
        print_stats(name, stat);
    }
    if let Some(ref path) = config.output {
        write_report(
            path,
            &config,
            elapsed,
            cycles,
            final_count,
            min_count,
            max_count,
            &stats,
        );
        println!("report: {}", path.display());
    }
}

fn parse_config() -> Config {
    let mut config = Config {
        entities: DEFAULT_ENTITIES,
        seconds: DEFAULT_SECONDS,
        output: None,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--entities" => {
                config.entities = value(&mut args, &flag)
                    .parse()
                    .expect("--entities must be an integer")
            }
            "--seconds" => {
                config.seconds = value(&mut args, &flag)
                    .parse()
                    .expect("--seconds must be an integer")
            }
            "--output" => config.output = Some(PathBuf::from(value(&mut args, &flag))),
            "--help" | "-h" => {
                println!("Usage: ecs_stress [--entities N] [--seconds N] [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(config.entities > 0, "--entities must be greater than zero");
    config
}

fn value(args: &mut impl Iterator<Item = String>, flag: &str) -> String {
    args.next()
        .unwrap_or_else(|| panic!("{flag} requires a value"))
}

fn run(config: &Config) -> (u64, usize, usize, usize, Samples) {
    let mut world = World::new();
    let mut active = Vec::with_capacity(config.entities);
    let mut samples = Samples::default();
    let started = Instant::now();
    for index in 0..config.entities {
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform::from_position(Vec2::new(index as f32, 0.0)),
            )
            .expect("spawned entity accepts Transform");
        world
            .insert(entity, Velocity::new(Vec2::new(1.0, 0.25), 0.01))
            .expect("spawned entity accepts Velocity");
        active.push(entity);
    }
    samples.create.push(started.elapsed());
    let initial = world.entity_count();
    let mut min_count = initial;
    let mut max_count = initial;
    let deadline = Instant::now() + Duration::from_secs(config.seconds);
    let mut cycles = 0;
    while Instant::now() < deadline || cycles == 0 {
        let started = Instant::now();
        let query = world.entities_with2::<Transform, Velocity>();
        assert_eq!(query.len(), config.entities);
        black_box(query);
        samples.query.push(started.elapsed());

        let started = Instant::now();
        assert_eq!(
            integrate_velocity(&mut world, 1.0 / 60.0).expect("valid fixed step"),
            config.entities
        );
        samples.update.push(started.elapsed());

        let batch = REPLACE_BATCH.min(active.len());
        let start = (cycles as usize * batch) % active.len();
        let started = Instant::now();
        let mut queue = CommandQueue::new();
        for offset in 0..batch {
            queue.despawn(active[(start + offset) % active.len()]);
        }
        samples.destroy.push(started.elapsed());

        let started = Instant::now();
        for index in 0..batch {
            let token = queue.spawn();
            queue.insert(
                token,
                Transform::from_position(Vec2::new((cycles as usize + index) as f32, 1.0)),
            );
            queue.insert(token, Velocity::new(Vec2::new(1.0, 0.25), 0.01));
        }
        queue.apply(&mut world).expect("command batch applies");
        samples.command.push(started.elapsed());

        active = world.entities_with::<Transform>();
        assert_eq!(active.len(), config.entities);
        min_count = min_count.min(world.entity_count());
        max_count = max_count.max(world.entity_count());
        cycles += 1;
    }
    (cycles, world.entity_count(), min_count, max_count, samples)
}

fn summarize(mut values: Vec<Duration>) -> Stats {
    values.sort_unstable();
    let total: u128 = values.iter().map(Duration::as_micros).sum();
    let p95 = ((values.len() * 95).saturating_sub(1) / 100).min(values.len() - 1);
    Stats {
        count: values.len(),
        min_us: values[0].as_micros(),
        average_us: total / values.len() as u128,
        p95_us: values[p95].as_micros(),
        max_us: values[values.len() - 1].as_micros(),
    }
}

fn print_stats(name: &str, stat: &Stats) {
    println!(
        "{name}: count={} min_us={} avg_us={} p95_us={} max_us={}",
        stat.count, stat.min_us, stat.average_us, stat.p95_us, stat.max_us
    );
}

#[allow(clippy::too_many_arguments)]
fn write_report(
    path: &PathBuf,
    config: &Config,
    elapsed: Duration,
    cycles: u64,
    final_count: usize,
    min_count: usize,
    max_count: usize,
    stats: &[(&str, Stats); 5],
) {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).expect("create report directory");
    }
    let mut json = format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"entities\": {},\n  \"requested_seconds\": {},\n  \"elapsed_ms\": {},\n  \"cycles\": {},\n  \"final_entities\": {},\n  \"min_entities\": {},\n  \"max_entities\": {},\n  \"samples\": {{\n",
        config.entities,
        config.seconds,
        elapsed.as_millis(),
        cycles,
        final_count,
        min_count,
        max_count
    );
    for (index, (name, stat)) in stats.iter().enumerate() {
        json.push_str(&format!("    \"{name}\": {{\"count\":{},\"min_us\":{},\"average_us\":{},\"p95_us\":{},\"max_us\":{}}}{}\n", stat.count, stat.min_us, stat.average_us, stat.p95_us, stat.max_us, if index == stats.len() - 1 { "" } else { "," }));
    }
    json.push_str("  }\n}\n");
    fs::write(path, json).expect("write stress report");
}
