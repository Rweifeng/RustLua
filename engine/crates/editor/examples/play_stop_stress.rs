use editor::{PlaySession, SceneDocument};
use engine::core::scene::SceneEntityId;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const ITERATIONS: usize = 100;

fn unique_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos()
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rustlua-m15-play-stop-{label}-{}", unique_token()))
}

fn sample_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
}

fn copy_dir(source: &Path, destination: &Path) {
    if !source.is_dir() {
        return;
    }
    for entry in fs::read_dir(source).expect("read dir") {
        let entry = entry.expect("dir entry");
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            fs::create_dir_all(&target_path).expect("create target dir");
            copy_dir(&source_path, &target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::copy(&source_path, &target_path).expect("copy file");
        }
    }
}

fn copy_sample_project(label: &str) -> PathBuf {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    fs::create_dir_all(&root).expect("create temp root");
    fs::copy(source.join("project.toml"), root.join("project.toml")).expect("copy manifest");
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    copy_dir(&source.join("assets"), &root.join("assets"));
    root
}

fn main() {
    let root = copy_sample_project("stress");
    let path = root.join("scenes/production.scene");
    let document = SceneDocument::open(&path).expect("open sample scene");
    let original = document.clone();
    let started = Instant::now();

    for iteration in 0..ITERATIONS {
        let mut session = PlaySession::start(&document);
        session
            .runtime_document_mut()
            .scene
            .metadata
            .insert("frame".to_owned(), iteration.to_string());
        session
            .runtime_document_mut()
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .expect("sample player entity")
            .name = format!("RuntimePlayer{iteration}");
        session.runtime_document_mut().mark_dirty();
        let restored = session.stop();
        assert_eq!(restored, original, "restored scene should match original");
    }

    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "play_stop_stress_ok iterations={ITERATIONS} total_ms={total_ms:.3} avg_ms={:.3} entities={}",
        total_ms / ITERATIONS as f64,
        original.scene.entities.len()
    );

    let _ = fs::remove_dir_all(root);
}
