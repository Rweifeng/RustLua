use engine_core::resource_db::{ImportStatus, ResourceDatabase};

fn main() {
    let root =
        std::env::temp_dir().join(format!("rustlua-resource-db-demo-{}", std::process::id()));
    let mut db = ResourceDatabase::open(&root).expect("open database");
    let texture = db
        .register("assets/hero.png", "png-1")
        .expect("register texture");
    let scene = db
        .register("scenes/main.ron", "scene-1")
        .expect("register scene");
    db.set_dependencies(scene, vec![texture])
        .expect("set dependency");
    db.set_import_result(texture, ImportStatus::Imported, ".p2/cache/hero", None)
        .expect("import texture");
    db.move_resource("assets/hero.png", "assets/player.png")
        .expect("move texture");
    db.set_import_result(
        scene,
        ImportStatus::Failed,
        ".p2/cache/main",
        Some("missing entity component".into()),
    )
    .expect("record failure");
    db.save().expect("save database");
    let moved_id = db
        .find_by_path("assets/player.png")
        .expect("find moved texture")
        .expect("moved texture")
        .id;
    let stable_id = moved_id == texture;
    let dependents = db.remove("assets/player.png").expect("remove texture");
    println!(
        "resource_db_demo_ok records={} stable_id={} failed_status={:?} dependents={}",
        db.len(),
        stable_id,
        db.get(scene).map(|record| &record.status),
        dependents.len()
    );
    let _ = std::fs::remove_dir_all(root);
}
