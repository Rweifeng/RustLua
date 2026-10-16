//! WP3 存储 A/B 基准（`wp0-v5`）：**替换前的** hash 存储 vs dense/sparse。
//!
//! 目的：把 WP3 的"是否替换 ECS 热存储"从意见变成证据。两侧跑同一套固定工作负载
//! （插入 / 按 id 乱序读 / 全量可变遍历 / 隔一个删一个），同时报告时间与**分配次数**。
//!
//! 两侧口径：`HashStorage` 是**替换前** `core/world.rs` 的 `TypedStorage`（HashMap 存值 +
//! 一份排序的实体表，插入/删除对实体表做 O(n) memmove，可变遍历每次调用**克隆整份实体表**）；
//! `DenseStorage` 是**替换后**生产 `World` 在用的存储（dense 值数组 + sparse 槽位 + 惰性重建
//! 的有序 id 索引）。因此本基准现在是"旧实现 vs 现生产实现"的对拍，而不是两个候选方案。
//!
//! 运行：
//! ```powershell
//! cd engine
//! cargo run --release -p engine --example wp0_ecs_storage -- --cycles 30 `
//!   --output ..\baselines\wp0-ecs-storage.json
//! ```

#[path = "support/wp0_alloc.rs"]
mod wp0_alloc;

use rustlua_ecs::dense_storage::DenseStorage;
use rustlua_ecs::entity::{EntityAllocator, EntityId};
use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

const REPORT_VERSION: &str = "wp0-v5";
const SCALES: [usize; 3] = [1_000, 10_000, 100_000];
const DEFAULT_CYCLES: usize = 30;

struct Config {
    output: Option<PathBuf>,
    cycles: usize,
}

fn main() {
    let config = parse_config();
    let stairs = SCALES
        .iter()
        .map(|scale| run_stair(*scale, config.cycles))
        .collect::<Vec<_>>();
    let json = format_report(&stairs, &config);
    if let Some(path) = &config.output {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).expect("create report directory");
        }
        fs::write(path, &json).expect("write wp0-v5 report");
        println!("report: {}", path.display());
    } else {
        print!("{json}");
    }
    println!(
        "wp0_ecs_storage_ok report_version={REPORT_VERSION} scales={}",
        stairs.len()
    );
}

fn parse_config() -> Config {
    let mut output = None;
    let mut cycles = DEFAULT_CYCLES;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ));
            }
            "--cycles" => {
                cycles = args
                    .next()
                    .expect("--cycles requires a value")
                    .parse()
                    .expect("--cycles must be an integer");
            }
            "--help" | "-h" => {
                println!("Usage: wp0_ecs_storage [--cycles N] [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    assert!(cycles > 0, "--cycles must be greater than zero");
    Config { output, cycles }
}

/// 两侧共用的最小接口：只保留热路径真正用到的四个操作。
trait Storage: Default {
    fn insert(&mut self, entity: EntityId, value: f32);
    fn get(&self, entity: EntityId) -> Option<&f32>;
    fn for_each_mut(&mut self, visit: &mut dyn FnMut(EntityId, &mut f32));
    fn remove(&mut self, entity: EntityId) -> Option<f32>;
}

impl Storage for DenseStorage<f32> {
    fn insert(&mut self, entity: EntityId, value: f32) {
        DenseStorage::insert(self, entity, value);
    }
    fn get(&self, entity: EntityId) -> Option<&f32> {
        DenseStorage::get(self, entity)
    }
    fn for_each_mut(&mut self, visit: &mut dyn FnMut(EntityId, &mut f32)) {
        for (entity, value) in self.iter_mut() {
            visit(entity, value);
        }
    }
    fn remove(&mut self, entity: EntityId) -> Option<f32> {
        DenseStorage::remove(self, entity)
    }
}

/// **替换前**的生产口径：`HashMap<EntityId, T>` + 排序实体表（保留为历史基线）。
#[derive(Default)]
struct HashStorage<T> {
    values: HashMap<EntityId, T>,
    entities: Vec<EntityId>,
}

impl Storage for HashStorage<f32> {
    fn insert(&mut self, entity: EntityId, value: f32) {
        if self.values.insert(entity, value).is_none() {
            match self.entities.binary_search(&entity) {
                Ok(_) => {}
                Err(index) => self.entities.insert(index, entity),
            }
        }
    }
    fn get(&self, entity: EntityId) -> Option<&f32> {
        self.values.get(&entity)
    }
    fn for_each_mut(&mut self, visit: &mut dyn FnMut(EntityId, &mut f32)) {
        // `HashStorage` = 替换前的生产模型（HashMap + 排序实体表），这里克隆整份实体表是**旧形状**
        // 的建模。它并**不**对应 `World::for_each_mut`：后者走的是"移出有序索引再放回"（`mem::take`，
        // 不克隆），而且自惰性重建补上脏守卫后，稳态帧连重排也不做了（见
        // `rustlua-ecs/src/world.rs` 的 `ensure_order_mut`）。保留这份克隆是为了让
        // 「替换前 vs 替换后」两条曲线可比——`for_each_mut_allocs=1` 是旧形状的读数。
        let entities = self.entities.clone();
        for entity in entities {
            if let Some(value) = self.values.get_mut(&entity) {
                visit(entity, value);
            }
        }
    }
    fn remove(&mut self, entity: EntityId) -> Option<f32> {
        let removed = self.values.remove(&entity);
        if removed.is_some()
            && let Ok(index) = self.entities.binary_search(&entity)
        {
            self.entities.remove(index);
        }
        removed
    }
}

struct Ops {
    insert: (Stats, u64),
    get: (Stats, u64),
    for_each_mut: (Stats, u64),
    remove_half: (Stats, u64),
}

/// 固定种子 LCG：给"按 id 读"制造稳定的乱序访问，避免顺序访问偏袒某一侧。
fn scattered_order(count: usize) -> Vec<EntityId> {
    let mut order = (0..count as u32)
        .map(|index| EntityId::from_raw(index, 1))
        .collect::<Vec<_>>();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for index in (1..order.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let swap_with = (state >> 33) as usize % (index + 1);
        order.swap(index, swap_with);
    }
    order
}

fn filled<S: Storage>(count: usize) -> S {
    let mut storage = S::default();
    for index in 0..count as u32 {
        storage.insert(EntityId::from_raw(index, 1), index as f32);
    }
    storage
}

fn run_ops<S: Storage>(entities: usize, cycles: usize) -> Ops {
    let order = scattered_order(entities);
    let samples = |run: &mut dyn FnMut()| -> (Stats, u64) {
        let mut times = Vec::with_capacity(cycles);
        let mut allocs = Vec::with_capacity(cycles);
        for _ in 0..cycles {
            wp0_alloc::reset_window();
            let started = Instant::now();
            run();
            times.push(started.elapsed());
            allocs.push(wp0_alloc::window_count());
        }
        (summarize(times), median_u64(&allocs))
    };

    // 插入
    let insert = samples(&mut || {
        let mut storage = S::default();
        for index in 0..entities as u32 {
            storage.insert(EntityId::from_raw(index, 1), index as f32);
        }
        std::hint::black_box(&storage);
    });

    // 按 id 乱序读
    let mut storage = filled::<S>(entities);
    let mut sink = 0.0_f32;
    let get = samples(&mut || {
        for entity in &order {
            if let Some(value) = storage.get(*entity) {
                sink += *value;
            }
        }
        std::hint::black_box(sink);
    });

    // 全量可变遍历
    let foreach = samples(&mut || {
        storage.for_each_mut(&mut |_, value| *value += 1.0);
    });

    // 隔一个删一个
    let remove_half = samples(&mut || {
        let mut storage = filled::<S>(entities);
        for index in (0..entities as u32).step_by(2) {
            storage.remove(EntityId::from_raw(index, 1));
        }
        std::hint::black_box(&storage);
    });

    Ops {
        insert,
        get,
        for_each_mut: foreach,
        remove_half,
    }
}

fn ops_json(ops: &Ops) -> String {
    format!(
        "{{\"insert\":{},\"insert_allocs\":{},\
         \"get\":{},\"get_allocs\":{},\
         \"for_each_mut\":{},\"for_each_mut_allocs\":{},\
         \"remove_half\":{},\"remove_half_allocs\":{}}}",
        stats_json(&ops.insert.0),
        ops.insert.1,
        stats_json(&ops.get.0),
        ops.get.1,
        stats_json(&ops.for_each_mut.0),
        ops.for_each_mut.1,
        stats_json(&ops.remove_half.0),
        ops.remove_half.1,
    )
}

fn run_stair(entities: usize, cycles: usize) -> String {
    // 两侧各自建一遍实体表：分配器只用来产出合法的稀疏槽位。
    let mut allocator = EntityAllocator::new();
    for _ in 0..entities {
        allocator.spawn();
    }
    let hash = run_ops::<HashStorage<f32>>(entities, cycles);
    let dense = run_ops::<DenseStorage<f32>>(entities, cycles);
    format!(
        "{{\"entities\":{entities},\"samples\":{cycles},\
         \"hash\":{},\"dense\":{},\
         \"speedup\":{{\"insert\":{:.2},\"get\":{:.2},\"for_each_mut\":{:.2},\"remove_half\":{:.2}}}}}",
        ops_json(&hash),
        ops_json(&dense),
        ratio(hash.insert.0.p50_us, dense.insert.0.p50_us),
        ratio(hash.get.0.p50_us, dense.get.0.p50_us),
        ratio(hash.for_each_mut.0.p50_us, dense.for_each_mut.0.p50_us),
        ratio(hash.remove_half.0.p50_us, dense.remove_half.0.p50_us),
    )
}

fn ratio(hash_ns: u128, dense_ns: u128) -> f64 {
    if dense_ns == 0 {
        return 0.0;
    }
    hash_ns as f64 / dense_ns as f64
}

fn format_report(stairs: &[String], config: &Config) -> String {
    format!(
        "{{\n  \"report_version\": \"{REPORT_VERSION}\",\n  \"benchmark\": \"ecs_storage\",\n  \
         \"target\": {{\"cycles\": {}, \"scales\": [{}, {}, {}]}},\n  \"stairs\": [\n    {}\n  ]\n}}\n",
        config.cycles,
        SCALES[0],
        SCALES[1],
        SCALES[2],
        stairs.join(",\n    ")
    )
}

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

/// 纳秒口径：1k 档的单项操作常在微秒以下，用微秒会被截断成 0。
fn summarize(mut values: Vec<Duration>) -> Stats {
    values.sort_unstable();
    let at = |p: usize| ((values.len() * p).saturating_sub(1) / 100).min(values.len() - 1);
    let scale = |value: Duration| value.as_nanos();
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
    format!(
        "{{\"count\":{},\"min_ns\":{},\"average_ns\":{},\"p50_ns\":{},\"p95_ns\":{},\
         \"p99_ns\":{},\"max_ns\":{}}}",
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
