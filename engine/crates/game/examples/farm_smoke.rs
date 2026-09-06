//! Headless smoke test for the `game_farm` scripts.
//!
//! Loads the project's `scripts/main.lua` into a `LuaRuntime` with no window,
//! runs `on_create` + a few update frames, and reports whether the farm
//! vertical slice boots. Used during M16 development and M16-12 acceptance.
//!
//! Usage:
//!   cargo run -p game --example farm_smoke [-- <game_farm-root>]
//!
//! The project root defaults to `../../../game_farm` relative to the `game`
//! crate (the repo's `game_farm/` directory).

use engine::core::{
    input::{InputKey, InputSnapshot},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::legacy_games_dir().join("game_farm"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }

    let main_script = project.join("scripts/main.lua");
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project);
    let data_json = project.join("data/farm.json");
    if !data_json.is_file() {
        return Err(format!("missing {}", data_json.display()).into());
    }

    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.clone());
    runtime.set_save_path(save_path.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);

    // on_create(): spawns the player, defines items, creates the inventory.
    runtime.execute_project_script(&main_script)?;

    // A few idle frames; update() must return "farm_day_<n>".
    for _ in 0..5 {
        let result = runtime.call_update()?;
        if !result.starts_with("farm_day_") {
            return Err(format!("update() returned {result:?}").into());
        }
    }

    // data.load("data/farm.json") must resolve against the project root.
    let plots = runtime.execute_console(
        "local farm = data.load('data/farm.json')\nreturn #farm.plots, farm.plot_radius",
    )?;
    if !plots.contains("9") {
        return Err(format!("data.load(farm.json) unexpected: {plots:?}").into());
    }

    // A movement frame (D held) must not error.
    let mut input = InputSnapshot::default();
    input.key_event(InputKey::D, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;

    // Player and inventory exist after on_create.
    let alive = runtime.execute_console(
        "local p = entity.find('FieldFarmer')\nreturn tostring(p ~= nil and entity.is_alive(p))",
    )?;
    if !alive.contains("true") {
        return Err(format!("player not alive after on_create: {alive:?}").into());
    }
    let seeds = runtime.execute_console("return inv.count('player', 'seed_tomato')")?;
    if seeds.trim() != "9" {
        return Err(format!("expected 9 starting seeds, got {seeds:?}").into());
    }

    // Item definitions must come from data/items.json (data pipeline, M16-11).
    let tomato_sell =
        runtime.execute_console("local d = item.def('tomato')\nreturn tostring(d and d.sell)")?;
    if !tomato_sell.contains("60") {
        return Err(format!("item.def('tomato').sell != 60: {tomato_sell:?}").into());
    }

    // Crop data table (M16-05) loads and validates in on_create.
    let tomato_stages = runtime.execute_console(
        "local c = data.load('data/crops.json')\nreturn tostring(c.tomato and c.tomato.stages)",
    )?;
    if !tomato_stages.contains("3") {
        return Err(format!("crops.json tomato.stages != 3: {tomato_stages:?}").into());
    }

    // Full vertical-slice loop (M16-06): till -> plant -> water -> sleep -> water
    // -> sleep -> harvest, driven by simulated single key presses.
    runtime.execute_console("local p = entity.find('FieldFarmer')\ntransform.set(p, 480, 260)")?;
    press(&mut runtime, InputKey::from_char('q'))?; // till
    press(&mut runtime, InputKey::from_char('e'))?; // plant seed_tomato
    press(&mut runtime, InputKey::from_char('r'))?; // water
    press(&mut runtime, InputKey::from_char('t'))?; // sleep -> day 2, crop stage 1
    press(&mut runtime, InputKey::from_char('r'))?; // water again
    press(&mut runtime, InputKey::from_char('t'))?; // sleep -> day 3, crop stage 2 (mature)
    press(&mut runtime, InputKey::from_char('f'))?; // harvest -> tomato into inventory

    let tomatoes = runtime.execute_console("return inv.count('player', 'tomato')")?;
    if tomatoes.trim() != "1" {
        return Err(format!("expected 1 tomato after the loop, got {tomatoes:?}").into());
    }
    let day = runtime.execute_console("return clock.now().day")?;
    if day.trim() != "3" {
        return Err(format!("expected day 3 after two sleeps, got {day:?}").into());
    }

    // Shop (M16-10): price service + selling the harvested tomato at the market.
    let tomato_price = runtime
        .execute_console("local p = shop.price('farm', 'tomato')\nreturn tostring(p and p.sell)")?;
    if !tomato_price.contains("60") {
        return Err(format!("shop.price('farm','tomato').sell != 60: {tomato_price:?}").into());
    }
    runtime.execute_console("local p = entity.find('FieldFarmer')\ntransform.set(p, 680, 180)")?;
    press(&mut runtime, InputKey::from_char('g'))?; // sell at the market
    let tomatoes_after = runtime.execute_console("return inv.count('player', 'tomato')")?;
    if tomatoes_after.trim() != "0" {
        return Err(format!("expected tomato sold at market, got {tomatoes_after:?}").into());
    }

    // ---- M16-04: tilled state lives on the FarmGrid tilemap ----
    // The vertical slice tilled the facing plot. Facing was "right" (a movement
    // frame ran earlier), so the facing cell from (480,260) is (13,6)=(540,260).
    let tilled_meta = runtime.execute_console(
        "local g = entity.find('FarmGrid')\nreturn tostring(tilemap.get_meta(g, 13, 6, 'tilled'))",
    )?;
    if tilled_meta.trim() != "1" {
        return Err(format!("expected tilled meta on cell (13,6), got {tilled_meta:?}").into());
    }

    // ---- M16-01: Tilemap-solid collision + facing ----
    runtime.execute_console("local p = entity.find('FieldFarmer')\ntransform.set(p, 50, 160)")?;
    for _ in 0..60 {
        press(&mut runtime, InputKey::from_char('d'))?;
    }
    let wall_x = runtime.execute_console("return transform.get(entity.find('FieldFarmer')).x")?;
    let wall_x: f32 = wall_x.trim().parse()?;
    if !(50.0 < wall_x && wall_x < 80.0) {
        return Err(format!("player should be blocked at the house wall, got x={wall_x}").into());
    }
    let facing = runtime.execute_console("return require('state').facing")?;
    if facing.trim() != "right" {
        return Err(format!("expected facing 'right' after moving D, got {facing:?}").into());
    }

    // ---- M16-02: door TriggerZone fires on_zone_enter ----
    runtime.execute_console("local p = entity.find('FieldFarmer')\ntransform.set(p, 190, 340)")?;
    press(&mut runtime, InputKey::from_char('w'))?;
    let zone = runtime.execute_console("return tostring(require('state').zone_name)")?;
    if zone.trim() != "house_door" {
        return Err(format!("expected house_door zone entry, got {zone:?}").into());
    }

    // ---- M16-03: tool system ----
    let tools_energy = runtime.execute_console(
        "local t = data.load('data/tools.json')\nreturn tostring(t.hoe and t.hoe.energy)",
    )?;
    if tools_energy.trim() != "5" {
        return Err(format!("tools.json hoe.energy != 5: {tools_energy:?}").into());
    }
    press(&mut runtime, InputKey::from_char('2'))?; // hotbar slot 2 = hoe
    let sel_tool = runtime.execute_console("return tostring(require('tools').selected_tool())")?;
    if sel_tool.trim() != "hoe" {
        return Err(format!("selected_tool != hoe after slot 2: {sel_tool:?}").into());
    }
    let action = runtime.execute_console(
        "local a = require('tools').use('hoe')\nreturn a.kind .. ':' .. tostring(#a.cells)",
    )?;
    if action.trim() != "hoe:1" {
        return Err(format!("ToolAction unexpected: {action:?}").into());
    }

    // ---- M16-07: equipment skeleton + item use + container drag ----
    let equip = runtime.execute_console("return tostring(inv.equip_get('player', 'tool'))")?;
    if equip.trim() != "hoe" {
        return Err(format!("expected equipment tool=hoe, got {equip:?}").into());
    }
    runtime.execute_console("inv.set('player', 5, 'bread', 1)")?;
    runtime.execute_console("require('state').energy = 50")?;
    press(&mut runtime, InputKey::from_char('5'))?; // use the bread
    let energy_after = runtime.execute_console("return tostring(require('state').energy)")?;
    let bread_left = runtime.execute_console("return tostring(inv.count('player', 'bread'))")?;
    if energy_after.trim() != "70" || bread_left.trim() != "0" {
        return Err(format!(
            "bread use: energy={energy_after:?} bread={bread_left:?} (want 70 / 0)"
        )
        .into());
    }
    runtime.execute_console("inv.set('player', 9, 'tomato', 1)")?;
    runtime.execute_console("local p = entity.find('FieldFarmer')\ntransform.set(p, 660, 430)")?;
    runtime.call_update()?; // near the crate -> opens the container panel
    let ckey = runtime.execute_console("return tostring(require('state').container_key)")?;
    if ckey.trim() == "nil" {
        return Err("expected container to open near the crate".into());
    }
    runtime.execute_console("ui.drag_begin('bag', 1); ui.drag_end('crate', 1)")?;
    let moved = runtime.execute_console(
        "local k = require('state').container_key\nlocal s = inv.slot(k, 1)\nreturn tostring(s and s.item)",
    )?;
    let bag_cleared = runtime.execute_console("return tostring(inv.slot('player', 9))")?;
    if moved.trim() != "tomato" || bag_cleared.trim() != "nil" {
        return Err(format!(
            "bag->crate drag: crate={moved:?} bag_slot9={bag_cleared:?} (want tomato / nil)"
        )
        .into());
    }

    // Save/load round-trip (M16-09): F1 persists the WorldSnapshot; a fresh
    // runtime loads it with F2 and must land on the same day.
    press(&mut runtime, InputKey::Other(1))?; // F1 save
    if !save_path.is_file() {
        return Err("save file not written after F1".into());
    }
    let mut second = LuaRuntime::new()?;
    second.set_project_root(project.clone());
    second.set_save_path(save_path.clone());
    second.set_ui_viewport(1280, 720);
    second.set_delta(1.0 / 60.0);
    second.execute_project_script(&main_script)?;
    press(&mut second, InputKey::Other(2))?; // F2 load
    let loaded_day = second.execute_console("return clock.now().day")?;
    if loaded_day.trim() != "3" {
        return Err(format!("expected day 3 after F2 load, got {loaded_day:?}").into());
    }
    let loaded_version = second.execute_console("return save.version()")?;
    if loaded_version.trim() != "1" {
        return Err(format!("expected save.version()==1, got {loaded_version:?}").into());
    }

    println!(
        "farm_smoke_ok project={} seeds={} plots={} tomato_sell={} tomato_stages={} harvest_tomatoes={} day={} shop_sell={} tomatoes_after={} loaded_day={} save_version={}",
        project.display(),
        seeds.trim(),
        plots.trim(),
        tomato_sell.trim(),
        tomato_stages.trim(),
        tomatoes.trim(),
        day.trim(),
        tomato_price.trim(),
        tomatoes_after.trim(),
        loaded_day.trim(),
        loaded_version.trim()
    );
    let _ = fs::remove_file(save_path);
    Ok(())
}

/// Simulates a single key press: one frame with the key down, one clean frame.
fn press(runtime: &mut LuaRuntime, key: InputKey) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
    Ok(())
}

fn fresh_save_path(project: &Path) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("farm_smoke.sav");
    let _ = fs::remove_file(&path);
    path
}
