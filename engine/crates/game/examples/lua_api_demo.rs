use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
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
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_delta(1.0 / 60.0);
    host.set_input(input);
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
