//! Headless acceptance drive for 阶段2「NPC 10→16 + 生日 + 好感事件 30」
//! （docs/54 §4.8 / docs/33 §8.3，2026-10-06 1.4）。
//!
//! 四锁链：data/npcs.lua（+6 NPC + birthday + birthday_of 纯函数）
//!   → data/places.lua（新 home 地点）+ data/schedules/<id>.lua（含雨天变体）
//!   → data/relationships.lua（爱/喜/厌 三档偏好表 + 生日 ×3 乘子）
//!   → data/relationship_events.lua + systems/relationship_events.lua（≥30 事件）。
//!
//! 验收：
//!   1. 静态：NPC 数 = 16，每人 birthday ∈ 1..112；birthday_of(day) 纯函数正确；
//!      各模块 validate_all 空、validate_data strict 空；
//!   2. 送礼三档：牛大壮 爱礼+40 / 喜礼+20 / 厌礼-10 逐档钉死（既有 gifts 锚点不动）；
//!   3. 生日 ×3：阿药喜礼平日 +20、生日当天 +60（=20×3）；
//!   4. 好感事件：石敢当心数 2（=20 分）门槛——不到档不可见、到档出现、触发发奖、
//!      once 不重复；
//!   5. on_day_start 钩子显式调用：生日当天标注今日寿星，非生日为 nil；
//!   6. 存档往返：relationship 分数随 save/continue 恢复；事件完成表 capture/restore 一致。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m48_npc [-- <game_jhyt-root>]

use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let main_script = project.join("scripts/main.lua");
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m48_npc.sav");

    // ---------------- ① 静态：16 NPC + birthday 合法 + birthday_of ----------------
    banner("阶段2① NPC 数=16、每人 birthday∈1..112、birthday_of 纯函数");
    let mut rt = new_runtime(&project, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_num(
        &mut rt,
        "(function() return #require('data.npcs').list() end)()",
        16.0,
    )?;
    // 每人 birthday 必须是 1..cycle_days 的整数（漏写/越界 = 0）。
    assert_num(
        &mut rt,
        "(function() local npcs = require('data.npcs'); local bad = 0; \
         for _, n in ipairs(npcs.list()) do \
           if type(n.birthday) ~= 'number' or n.birthday ~= math.floor(n.birthday) \
             or n.birthday < 1 or n.birthday > npcs.cycle_days then bad = bad + 1 end \
         end; return bad end)()",
        0.0,
    )?;
    // birthday_of 纯函数：第 12 天 = 牛大壮，第 45 天 = 阿药。
    assert_str(
        &mut rt,
        "require('data.npcs').birthday_of(12).persist_id",
        "npc_niu_dazhuang",
    )?;
    assert_str(
        &mut rt,
        "require('data.npcs').birthday_of(45).persist_id",
        "npc_a_yao",
    )?;
    // 数据门：新事件数据/系统自校验空，且既有校验链（npcs/places/relationships…）strict 通过。
    assert_str(
        &mut rt,
        "table.concat((select(2, require('data.relationship_events').validate_all())) or {}, '\\n')",
        "",
    )?;
    assert_str(
        &mut rt,
        "table.concat((select(2, require('systems.relationship_events').validate_all())) or {}, '\\n')",
        "",
    )?;
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    println!("  16 NPC、birthday 全合法；birthday_of(12)=牛大壮 / (45)=阿药；validate 门全空");

    let rel = "require('player.relationship')";
    let rev = "require('systems.relationship_events')";

    // ---------------- ② 送礼三档：爱+40 / 喜+20 / 厌-10（牛大壮） ----------------
    banner("阶段2② 送礼偏好三档：牛大壮 爱礼+40 / 喜礼+20 / 厌礼-10");
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'rice', 5); \
         inv.add(require('core.state').inventory, 'wild_fruit', 5); \
         inv.add(require('core.state').inventory, 'iron_ore', 5)",
    )?;
    // 牛大壮生日=12，下面三天都不是生日，×3 不触发，分值就是档基本身。
    lua(&mut rt, "clock.set(1, 600)")?;
    assert_str(
        &mut rt,
        &format!("select(2, {rel}.gift('npc_niu_dazhuang', 'rice'))"),
        "40",
    )?;
    lua(&mut rt, "clock.set(2, 600)")?;
    assert_str(
        &mut rt,
        &format!("select(2, {rel}.gift('npc_niu_dazhuang', 'wild_fruit'))"),
        "60",
    )?;
    lua(&mut rt, "clock.set(3, 600)")?;
    assert_str(
        &mut rt,
        &format!("select(2, {rel}.gift('npc_niu_dazhuang', 'iron_ore'))"),
        "50",
    )?;
    assert_num(&mut rt, &format!("{rel}.points('npc_niu_dazhuang')"), 50.0)?;
    println!(
        "  牛大壮：爱礼 rice → +40；喜礼 wild_fruit → +20（40→60）；厌礼 iron_ore → -10（60→50）"
    );

    // ---------------- ③ 生日当天送礼 ×3（阿药，生日=45） ----------------
    banner("阶段2③ 生日 ×3：阿药喜礼平日 +20、生日当天 +60（=20×3）");
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'mung_bean', 2); \
         inv.add(require('core.state').inventory, 'cucumber', 2)",
    )?;
    // 第 10 天（非生日）：喜礼 mung_bean = +20。
    lua(&mut rt, "clock.set(10, 600)")?;
    assert_str(
        &mut rt,
        &format!("select(2, {rel}.gift('npc_a_yao', 'mung_bean'))"),
        "20",
    )?;
    // 第 45 天（阿药生日）：喜礼 cucumber = +20 ×3 = +60（20→80，不触顶）。
    lua(&mut rt, "clock.set(45, 600)")?;
    assert_str(
        &mut rt,
        &format!("select(2, {rel}.gift('npc_a_yao', 'cucumber'))"),
        "80",
    )?;
    assert_num(&mut rt, &format!("{rel}.points('npc_a_yao')"), 80.0)?;
    println!("  阿药：平日喜礼 mung_bean → +20；生日当天喜礼 cucumber → +60（20→80）");

    // ---------------- ④ 好感事件门槛 / 触发 / once（石敢当） ----------------
    banner("阶段2④ 好感事件：石敢当心数2门槛——不到档不可见、到档触发发奖、once 不重复");
    // 先把石敢当关系分推到 10（门槛 2 心 = 20 分之下）。
    lua(
        &mut rt,
        &format!("{rel}.add('npc_shi_gandang', 10, 'test', 'setup')"),
    )?;
    assert_num(
        &mut rt,
        &format!("{rev}.list_for('npc_shi_gandang') and 1 or 0"),
        1.0,
    )?;
    // 10 分时 ev_shi_1（2心=20分）不可触发。
    assert_num(
        &mut rt,
        "(function() local n = 0; for _, e in ipairs(require('systems.relationship_events').list_for('npc_shi_gandang')) do \
           if e.id == 'ev_shi_1' then n = n + 1 end end; return n end)()",
        0.0,
    )?;
    // 再 +10 到 20 = 2 心门槛，事件出现。
    lua(
        &mut rt,
        &format!("{rel}.add('npc_shi_gandang', 10, 'test', 'setup')"),
    )?;
    assert_num(
        &mut rt,
        "(function() local n = 0; for _, e in ipairs(require('systems.relationship_events').list_for('npc_shi_gandang')) do \
           if e.id == 'ev_shi_1' then n = n + 1 end end; return n end)()",
        1.0,
    )?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    let rice_before = query_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
    )?;
    // 触发：发奖励 关系分+10 / 金币+50 / 物品 rice×1。
    assert_str(
        &mut rt,
        "(function() local ok = require('systems.relationship_events').trigger('ev_shi_1'); return tostring(ok) end)()",
        "true",
    )?;
    assert_num(&mut rt, &format!("{rel}.points('npc_shi_gandang')"), 30.0)?;
    assert_num(&mut rt, "require('core.state').money", money_before + 50.0)?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'rice')",
        rice_before + 1.0,
    )?;
    // once：重复触发拒绝。
    assert_str(
        &mut rt,
        "(function() local ok, reason = require('systems.relationship_events').trigger('ev_shi_1'); return tostring(ok) .. '|' .. tostring(reason) end)()",
        "false|already_done",
    )?;
    println!(
        "  石敢当：10分不见 ev_shi_1 → 20分出现 → 触发发奖(+10/+50/rice) → 再触发 already_done"
    );

    // ---------------- ⑤ on_day_start 钩子：今日寿星标注 ----------------
    banner("阶段2⑤ on_day_start：生日当天标注寿星，非生日为 nil");
    lua(&mut rt, "clock.set(12, 600)")?;
    assert_str(
        &mut rt,
        "require('systems.relationship_events').on_day_start()",
        "npc_niu_dazhuang",
    )?;
    assert_str(
        &mut rt,
        "require('systems.relationship_events').birthday_today()",
        "npc_niu_dazhuang",
    )?;
    lua(&mut rt, "clock.set(1, 600)")?;
    assert_str(
        &mut rt,
        "require('systems.relationship_events').on_day_start()",
        "nil",
    )?;
    println!("  on_day_start：第12天 → 牛大壮；第1天 → nil");

    // ---------------- ⑥ 存档往返：关系分数 + 事件完成表 ----------------
    banner("阶段2⑥ 存档往返：关系分随 save/continue 恢复；事件完成表 capture/restore 一致");
    // 事件完成表的 capture/restore（main.lua 的 save 注册行由补丁接入，样例直连模块）。
    assert_str(
        &mut rt,
        "(function() local ev = require('systems.relationship_events'); \
           local snap = ev.capture(); local was = ev.done('ev_shi_1'); \
           ev.reset(); local after_reset = ev.done('ev_shi_1'); \
           ev.restore(snap); local after_restore = ev.done('ev_shi_1'); \
           return tostring(was) .. ',' .. tostring(after_reset) .. ',' .. tostring(after_restore) end)()",
        "true,false,true",
    )?;
    // 真实存档：relationship 是已注册 provider，分数跨 save/continue 回来。
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project, &save_path)?;
    enter_main_menu(&mut second, &main_script)?;
    click_button(&mut second, "BtnContinue")?;
    assert_num(
        &mut second,
        &format!("{rel}.points('npc_shi_gandang')"),
        30.0,
    )?;
    assert_num(&mut second, &format!("{rel}.points('npc_a_yao')"), 80.0)?;
    assert_num(
        &mut second,
        &format!("{rel}.points('npc_niu_dazhuang')"),
        50.0,
    )?;
    println!(
        "  读档：石敢当 30 / 阿药 80 / 牛大壮 50 关系分一致；事件表 capture→reset→restore 一致"
    );

    println!(
        "jhyt_m48_npc_ok project={} npc=16 birthday_valid=16 gift_like=40/20/-10 birthday_x3=80 event=ev_shi_1_done",
        project.display()
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers（与 jhyt_m34_relationship_tier 同一套） ----------

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok(runtime)
}

fn enter_main_menu(
    runtime: &mut LuaRuntime,
    main_script: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_project_script(main_script)?;
    runtime.execute_console("scene.load('main')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(runtime, "main")?;
    Ok(())
}

fn click_button(runtime: &mut LuaRuntime, button: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!("on_click('{button}')"))?;
    runtime.process_scene_transition()?;
    Ok(())
}

fn lua(runtime: &mut LuaRuntime, code: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(code)?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, expr: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value = runtime.execute_console(expr)?;
    Ok(value.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, expr)?;
    value
        .parse::<f64>()
        .map_err(|e| format!("{value:?} not a number: {e}").into())
}

fn assert_str(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(runtime, &format!("return tostring({expr})"))?;
    if got != expected {
        return Err(format!("{expr} = {got:?}, want {expected:?}").into());
    }
    Ok(())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, &format!("return {expr}"))?;
    if (got - expected).abs() > 1e-6 {
        return Err(format!("{expr} = {got}, want {expected}").into());
    }
    Ok(())
}

fn assert_scene_is(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = query_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
