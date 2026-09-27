use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "rustlua-lua-api-{}.lua",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::write(
        &path,
        r#"
player = entity.spawn("player")
transform.set(player, 10, 20)
transform.move(player, input.axis("horizontal") * 2, 0)
health.set(player, 100)
local remaining = health.damage(player, 25)
ui.text("status", "HP=" .. remaining)
save.write("last_scene", "arena")
scene.load("arena")
asset_token = asset.load("textures/player.png")
assert(physics.overlap(12, 20, 2)[1] == player)
function update() return "lua-api-ok" end
"#,
    )?;
    let mut input = InputSnapshot::default();
    input.key_event(KeyCode::KeyD, true);
    let mut runtime = LuaRuntime::new()?;
    runtime.set_delta(1.0 / 60.0);
    runtime.set_input(input);
    runtime.execute_project_script(&path)?;
    let result = runtime.call_update()?;
    assert_eq!(result, "lua-api-ok");

    println!(
        "lua_api_demo_ok update={result} quit={}",
        runtime.quit_requested()
    );
    fs::remove_file(path)?;
    Ok(())
}
