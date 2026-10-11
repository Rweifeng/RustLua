use rustlua_assets::import_cache::ImportCache;
use rustlua_assets::importer::{Importer, ImporterRegistry};
use rustlua_assets::resource_db::AssetId;
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_scene::asset_import::full_registry;
use std::{fs, path::Path};

/// 同一条缓存生命周期（首次 → 命中 → 源变化重导 → 清空重建）跑一遍一种资源。
fn drive(
    cache: &mut ImportCache,
    importer: &'static dyn Importer,
    root: &Path,
    file_name: &str,
    first_bytes: &[u8],
    changed_bytes: &[u8],
) -> String {
    let source = root.join(file_name);
    fs::write(&source, first_bytes).expect("write source");
    let id = AssetId::from_seed(file_name);
    let first = cache
        .import_file(id, &source, importer)
        .expect("first import");
    let hit = cache.import_file(id, &source, importer).expect("cache hit");
    fs::write(&source, changed_bytes).expect("change source");
    let changed = cache.import_file(id, &source, importer).expect("reimport");
    cache.clear().expect("clear cache");
    let rebuilt = cache.import_file(id, &source, importer).expect("rebuild");
    let (first, hit, changed, rebuilt) = (first.reused, hit.reused, changed.reused, rebuilt.reused);
    assert!(
        !first && hit && !changed && !rebuilt,
        "{file_name}: 复用语义不符 (first={first}, hit={hit}, changed={changed}, rebuilt={rebuilt})"
    );
    format!("{file_name}:reused={first},{hit},{changed},{rebuilt}")
}

fn main() {
    let root =
        std::env::temp_dir().join(format!("rustlua-import-cache-demo-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create root");
    // core 拆分后 builtin 注册表不含 Scene/Prefab，导入方必须用 full_registry 组装。
    let registry: &ImporterRegistry = &full_registry();
    let mut cache = ImportCache::open(&root).expect("open cache");
    let scene = drive(
        &mut cache,
        registry.get(ResourceKind::Scene).expect("scene importer"),
        &root,
        "scene.ron",
        b"Scene(version=1)",
        b"Scene(version=2)",
    );
    let atlas = drive(
        &mut cache,
        registry.get(ResourceKind::Atlas).expect("atlas importer"),
        &root,
        "hero.atlas",
        b"atlas_version=1\ntexture=sprites/hero.png\nregion\tidle\t0\t0\t64\t64\n",
        b"atlas_version=1\ntexture=sprites/hero.png\nregion\tidle\t0\t0\t64\t64\nregion\twalk\t64\t0\t64\t64\n",
    );
    println!(
        "import_cache_demo_ok scene[{scene}] atlas[{atlas}] cache_files={}",
        cache.len()
    );
    let _ = fs::remove_dir_all(root);
}
