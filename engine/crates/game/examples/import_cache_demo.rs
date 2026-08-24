use engine::core::{import_cache::ImportCache, resource_db::AssetId};
use std::fs;

fn main() {
    let root =
        std::env::temp_dir().join(format!("rustlua-import-cache-demo-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create root");
    let source = root.join("scene.ron");
    fs::write(&source, b"Scene(version=1)").expect("write source");
    let id = AssetId::from_seed("scene.ron");
    let mut cache = ImportCache::open(&root).expect("open cache");
    let first = cache
        .import_file(id, &source, "scene-1")
        .expect("first import");
    let hit = cache
        .import_file(id, &source, "scene-1")
        .expect("cache hit");
    fs::write(&source, b"Scene(version=2)").expect("change source");
    let changed = cache.import_file(id, &source, "scene-1").expect("reimport");
    cache.clear().expect("clear cache");
    let rebuilt = cache.import_file(id, &source, "scene-1").expect("rebuild");
    println!(
        "import_cache_demo_ok first_reused={} hit_reused={} changed_reused={} rebuilt_reused={} cache_files={}",
        first.reused,
        hit.reused,
        changed.reused,
        rebuilt.reused,
        cache.len()
    );
    let _ = fs::remove_dir_all(root);
}
