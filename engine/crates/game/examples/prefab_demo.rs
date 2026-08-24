use engine::core::{
    prefab::Prefab,
    resource_db::AssetId,
    scene::{Scene, SceneEntityId},
};
use std::{collections::BTreeMap, fs};

fn main() {
    let root = std::env::temp_dir().join(format!("rustlua-prefab-demo-{}", std::process::id()));
    let path = root.join("player.prefab");
    let mut source = Scene::new("source");
    source
        .add_entity(SceneEntityId(1), "Player", None)
        .expect("root");
    source
        .add_entity(SceneEntityId(2), "Weapon", Some(SceneEntityId(1)))
        .expect("child");
    source
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("Damage".into(), "10".into());
    let prefab = Prefab::from_scene(
        &source,
        SceneEntityId(1),
        AssetId::from_seed("player.prefab"),
        "Player",
    )
    .expect("extract");
    prefab.save(&path).expect("save prefab");
    let loaded = Prefab::load(&path).expect("load prefab");
    let mut target = Scene::new("target");
    let first = loaded
        .instantiate(&mut target, None, &BTreeMap::new())
        .expect("first instance");
    let mut overrides = BTreeMap::new();
    overrides.insert((SceneEntityId(2), "component:Damage".into()), "25".into());
    let second = loaded
        .instantiate(&mut target, None, &overrides)
        .expect("second instance");
    let second_weapon = second.source_to_instance[&SceneEntityId(2)];
    println!(
        "prefab_demo_ok entities={} instances=2 unique_roots={} override={} roundtrip={}",
        target.entities.len(),
        first.instance_root != second.instance_root,
        target.entities[&second_weapon].components["Damage"],
        loaded == prefab
    );
    let _ = fs::remove_dir_all(root);
}
