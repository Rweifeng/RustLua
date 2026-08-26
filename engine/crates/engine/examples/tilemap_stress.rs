use engine::{
    core::{
        scene::{Scene, SceneEntityId},
        tilemap::Tilemap,
        transform::Vec2,
    },
    host::RuntimeScene,
};
use std::{env, time::Instant};

#[derive(Debug)]
struct Config {
    width: usize,
    height: usize,
    cycles: usize,
}

fn parse_config() -> Config {
    let mut config = Config {
        width: 100,
        height: 100,
        cycles: 3,
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
            "--help" | "-h" => {
                println!("Usage: tilemap_stress [--width N] [--height N] [--cycles N]");
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
        "x=0;y=0;rotation=0;scale_x=1;scale_y=1".to_owned(),
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

    let mut last_vertices = 0usize;
    let mut last_draws = 0usize;
    let started = Instant::now();
    for _ in 0..config.cycles {
        let mesh = runtime.vertices(1280, 720).expect("build vertices");
        last_vertices = mesh.vertices.len();
        last_draws = mesh.draws.len();
    }
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "tilemap_stress_ok width={} height={} cycles={} total_ms={total_ms:.3} avg_ms={:.3} vertices={} draws={}",
        config.width,
        config.height,
        config.cycles,
        total_ms / config.cycles as f64,
        last_vertices,
        last_draws
    );
}
