use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scripting::script::{LuaRuntime, ReloadStatus};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

const SCRIPT: &str = r#"
local enemy_x = 8
local collected = false
local door_open = false
local result = "running"

function on_create()
    scene.load("arena")
    player = entity.spawn("player")
    enemy = entity.spawn("enemy")
    pickup = entity.spawn("pickup")
    door = entity.spawn("door")
    trigger = entity.spawn("trigger")
    transform.set(player, 0, 0)
    transform.set(enemy, enemy_x, 0)
    transform.set(pickup, 1, 0)
    transform.set(door, 3, 0)
    transform.set(trigger, 2, 0)
    health.set(player, 3)
    ui.text("objective", "Collect the key and reach the door")
end

function on_destroy()
    for _, name in ipairs({"player", "enemy", "pickup", "door", "trigger"}) do
        local id = entity.find(name)
        if id and entity.is_alive(id) then entity.destroy(id) end
    end
end

function update()
    if result ~= "running" then return result end
    transform.move(player, 1, 0)
    local position = transform.get(player)
    if not collected and entity.is_alive(pickup) and position.x >= 1 then
        entity.destroy(pickup)
        collected = true
        ui.text("objective", "Reach the door")
    end
    for _, id in ipairs(physics.overlap(position.x, position.y, 0.6)) do
        if id == enemy then
            health.damage(player, 1)
            result = "failure"
            ui.text("objective", "Defeated by the enemy")
            return result
        end
    end
    if collected and position.x >= 2 then door_open = true end
    if door_open and position.x >= 3 then
        result = "victory"
        ui.text("objective", "Victory!")
    end
    return result
end
"#;

fn run_to_victory(
    path: &std::path::Path,
) -> Result<(LuaRuntime, String), Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.execute_project_script(path)?;
    assert_eq!(runtime.call_update()?, "running");
    assert_eq!(runtime.call_update()?, "running");
    let result = runtime.call_update()?;
    assert_eq!(result, "victory");
    Ok((runtime, result))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!(
        "rustlua-scene-script-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    let scene_path = root.join("arena.scene");
    let script_path = root.join("arena.lua");
    fs::create_dir_all(&root)?;

    let mut scene = Scene::new("Arena");
    scene.add_entity(SceneEntityId(1), "Player", None)?;
    scene.add_entity(SceneEntityId(2), "Enemy", None)?;
    scene.add_entity(SceneEntityId(3), "Pickup", None)?;
    scene.add_entity(SceneEntityId(4), "Door", None)?;
    scene.add_entity(SceneEntityId(5), "Trigger", None)?;
    for (id, role) in [
        (1, "player"),
        (2, "enemy"),
        (3, "pickup"),
        (4, "door"),
        (5, "trigger"),
    ] {
        scene
            .entities
            .get_mut(&SceneEntityId(id))
            .unwrap()
            .components
            .insert("ScriptRole".into(), role.into());
    }
    scene.save(&scene_path)?;
    let loaded_scene = Scene::load(&scene_path)?;
    assert_eq!(loaded_scene.entities.len(), 5);
    assert_eq!(
        loaded_scene.entities[&SceneEntityId(5)].components["ScriptRole"],
        "trigger"
    );

    fs::write(&script_path, SCRIPT)?;
    let (_, first_result) = run_to_victory(&script_path)?;
    let (_, reloaded_scene_result) = run_to_victory(&script_path)?;
    assert_eq!(first_result, reloaded_scene_result);

    let mut hot_runtime = LuaRuntime::new()?;
    hot_runtime.execute_project_script(&script_path)?;
    assert_eq!(hot_runtime.call_update()?, "running");
    let failure_script = SCRIPT.replace("local enemy_x = 8", "local enemy_x = 1.2");
    fs::write(&script_path, failure_script)?;
    assert_eq!(hot_runtime.reload_if_changed()?, ReloadStatus::Reloaded);
    assert_eq!(hot_runtime.call_update()?, "failure");

    println!(
        "scene_script_demo_ok entities={} rules=lua victory={} reload_consistent={} hot_reload=failure",
        loaded_scene.entities.len(),
        first_result == "victory",
        first_result == reloaded_scene_result
    );
    fs::remove_dir_all(root)?;
    Ok(())
}
