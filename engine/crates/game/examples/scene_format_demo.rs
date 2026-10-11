use rustlua_assets::resource_db::AssetId;
use rustlua_scene::scene::{Scene, SceneEntityId};
use std::fs;

fn main() {
    let root = std::env::temp_dir().join(format!("rustlua-scene-demo-{}", std::process::id()));
    let path = root.join("main.scene");
    let mut scene = Scene::new("Main");
    scene
        .add_entity(SceneEntityId(1), "Root", None)
        .expect("root");
    scene
        .add_entity(SceneEntityId(2), "Player", Some(SceneEntityId(1)))
        .expect("player");
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Transform".into(), "x=1,y=2".into());
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .resources
        .insert("texture".into(), AssetId::from_seed("player.png"));
    scene.save(&path).expect("save");
    let loaded = Scene::load(&path).expect("load");
    loaded
        .save(root.join("roundtrip.scene"))
        .expect("roundtrip save");
    let error = Scene::from_text("scene_version=1\nname=broken\nunknown\n").unwrap_err();
    println!(
        "scene_format_demo_ok version={} entities={} roundtrip={} error_has_line={}",
        loaded.version,
        loaded.entities.len(),
        loaded == Scene::load(&path).unwrap(),
        error.to_string().contains("line 3")
    );
    let _ = fs::remove_dir_all(root);
}
