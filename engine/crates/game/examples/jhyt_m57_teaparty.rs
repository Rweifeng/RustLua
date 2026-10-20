//! Headless acceptance drive for 阶段3「茶会社交」（山庄茶会，docs/54 §7.5 /
//! docs/33 §8.3，2026-10-06 2.3）。
//!
//! 三锁链：data/tea_party.lua（门槛/规模/冷却/茶品白名单/话题池/结算常数 +
//! validate_all）→ systems/tea_party.lua（邀请/筹备/开场/结算/冷却/capture/restore/
//! reset）→ main.lua 补丁（save 注册 "tea_party"、new_game reset、on_day_start 挂点）。
//!
//! 验收：
//!   1. 静态：话题池 ≥10、两个模块 validate_all 空、validate_data strict 空；
//!   2. 邀请门槛：好感 0（<20）拒绝 threshold_not_met；推到 20（2 心）可邀，
//!      写 last_action=tea_invite_<npc>；仅 1 客时开场拒绝 too_few_guests；
//!   3. 备茶：非茶品 unknown_tea、缺茶 not_enough、持有后消耗 1 件；
//!   4. 话题：同话题 duplicate_topic、不存在 bad_topic、两合法不同话题开场通过；
//!   5. 结算：逐客基础 3 + 命中话题系数（顾长风 sect 命中武学 +6=9、沈青萝 village
//!      命中农桑 +6=9）走 relationship.add 入账（20→29）；阵营声望 sect/village
//!      各 +2；话题命中记情报 intel；写 last_action=tea_party_tp_1；
//!   6. 冷却：当天再邀同人 cooldown；第 5 天仍冷却；第 8 天（=1+7）on_day_start
//!      返回解除名单并可再邀；
//!   7. 存档往返：capture→reset→restore 一致；tea_party provider 自注册后
//!      save/continue 恢复场次/情报/好感/声望与冷却。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m57_teaparty [-- <game_jhyt-root>]

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
    let save_path = fresh_save_path(&project, "jhyt_m57_teaparty.sav");

    let rel = "require('player.relationship')";

    // ---------------- ① 静态：话题池 ≥10 + 两模块 validate_all 空 + strict 门 ----------------
    banner("阶段3① 静态：话题池=10、data/systems validate_all 空、validate_data strict 空");
    let mut rt = new_runtime(&project, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;
    assert_num(&mut rt, "#require('data.tea_party').topics", 10.0)?;
    assert_str(
        &mut rt,
        "table.concat((select(2, require('data.tea_party').validate_all())) or {}, '\\n')",
        "",
    )?;
    assert_str(
        &mut rt,
        "table.concat((select(2, require('systems.tea_party').validate_all())) or {}, '\\n')",
        "",
    )?;
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    println!("  话题池 10 条；两模块与 strict 校验门全空");

    // ---------------- ② 邀请门槛：好感不足拒绝 → 到档可邀（写 last_action） ----------------
    banner("阶段3② 邀请门槛：顾长风 0 分拒绝 threshold_not_met → 推到 20 分（2 心）可邀");
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|threshold_not_met",
    )?;
    // 推到 2 心门槛（20 分）。新档无建筑/未饮茶，social_multiplier=1.0，逐分钉死。
    lua(
        &mut rt,
        &format!("{rel}.add('npc_gu_changfeng', 20, 'test', 'setup')"),
    )?;
    lua(
        &mut rt,
        &format!("{rel}.add('npc_shen_qingluo', 20, 'test', 'setup')"),
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "true|npc_gu_changfeng",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "tea_invite_npc_gu_changfeng",
    )?;
    // 仅 1 客：开场拒绝 too_few_guests（且不写 last_action）。
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').begin('tp_martial', 'tp_farming'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|too_few_guests",
    )?;
    // 重复邀请同一人 → already_invited。
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|already_invited",
    )?;
    // 邀第二位：沈青萝（凑齐最低 2 客）。
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_shen_qingluo'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "true|npc_shen_qingluo",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "tea_invite_npc_shen_qingluo",
    )?;
    println!(
        "  顾长风 0 分拒邀 → 20 分可邀(tea_invite_gu)；1 客开场 too_few_guests；重复 already_invited；沈青萝入席"
    );

    // ---------------- ③ 备茶：非茶品/缺茶拒绝 → 持有消耗 1 件 ----------------
    banner("阶段3③ 备茶：rice 非茶品 unknown_tea、缺茶 not_enough → 备 tea_brew 消耗 1 件");
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').prepare('rice'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|unknown_tea",
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').prepare('tea_brew'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|not_enough",
    )?;
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'tea_brew', 2)",
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').prepare('tea_brew'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "true|tea_brew",
    )?;
    assert_num(
        &mut rt,
        "inv.count(require('core.state').inventory, 'tea_brew')",
        1.0,
    )?;
    println!("  rice 拒收、空背包拒收；备 tea_brew 成功，背包 2→1");

    // ---------------- ④ 话题选择：同话题/不存在拒绝 → 两合法话题开场 ----------------
    banner("阶段3④ 话题：同话题 duplicate_topic、未知 bad_topic → 武学+农桑开场通过");
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').begin('tp_martial', 'tp_martial'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|duplicate_topic",
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').begin('tp_nope', 'tp_farming'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|bad_topic",
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').begin('tp_martial', 'tp_farming'); \
         return tostring(ok)..'|'..(ok and table.concat(r, ',') or tostring(r)) end)()",
        "true|tp_martial,tp_farming",
    )?;
    println!("  同话题/未知话题拒收；武学+农桑开场通过");

    // ---------------- ⑤ 结算：好感/声望入账、情报、last_action ----------------
    banner("阶段3⑤ 结算：顾长风/沈青萝各 +9（基础3+命中6）、sect/village 声望各 +2、情报 2 条");
    let settled = query_str(
        &mut rt,
        "(function() local ok, s = require('systems.tea_party').settle(); \
         if not ok then return 'FAIL:'..tostring(s) end \
         return tostring(s.id)..'|tea='..tostring(s.tea)..'|intel='..tostring(s.intel_count)..\
         '|gu='..tostring(s.guests.npc_gu_changfeng)..'|shen='..tostring(s.guests.npc_shen_qingluo)..\
         '|sect='..tostring(s.reputation.sect)..'|village='..tostring(s.reputation.village) end)()",
    )?;
    if settled != "tp_1|tea=tea_brew|intel=2|gu=29|shen=29|sect=2|village=2" {
        return Err(format!(
            "结算明细 = {settled:?}，应为 tp_1|tea=tea_brew|intel=2|gu=29|shen=29|sect=2|village=2"
        )
        .into());
    }
    assert_num(&mut rt, &format!("{rel}.points('npc_gu_changfeng')"), 29.0)?;
    assert_num(&mut rt, &format!("{rel}.points('npc_shen_qingluo')"), 29.0)?;
    assert_num(&mut rt, &format!("{rel}.reputation('sect')"), 2.0)?;
    assert_num(&mut rt, &format!("{rel}.reputation('village')"), 2.0)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "tea_party_tp_1",
    )?;
    // 情报：命中才解锁——顾长风命中武学、不命中农桑。
    assert_num(
        &mut rt,
        "(function() return require('systems.tea_party').intel_known('npc_gu_changfeng','tp_martial') and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut rt,
        "(function() return require('systems.tea_party').intel_known('npc_gu_changfeng','tp_farming') and 1 or 0 end)()",
        0.0,
    )?;
    assert_num(&mut rt, "require('systems.tea_party').party_count()", 1.0)?;
    println!(
        "  结算 tp_1：两客 20→29、sect/village 声望 +2、情报 gu|武学 解锁；last_action=tea_party_tp_1"
    );

    // ---------------- ⑥ 冷却：当天/第 5 天再邀拒绝 → 第 8 天解除可再邀 ----------------
    banner("阶段3⑥ 冷却：当天与第 5 天再邀顾长风 cooldown；第 8 天 on_day_start 解除并可再邀");
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|cooldown",
    )?;
    lua(&mut rt, "clock.set(5, 600)")?; // 5-1=4 < 7，仍在冷却
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|cooldown",
    )?;
    lua(&mut rt, "clock.set(8, 600)")?; // 8-1=7 >= 7，冷却解除
    assert_str(
        &mut rt,
        "(function() local list = require('systems.tea_party').on_day_start(); \
         return table.concat(list, ',') end)()",
        "npc_gu_changfeng,npc_shen_qingluo",
    )?;
    assert_str(
        &mut rt,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "true|npc_gu_changfeng",
    )?;
    println!("  当天/第5天 cooldown；第8天 on_day_start 返回 gu,shen；顾长风可再邀");

    // ---------------- ⑦ 存档：capture→reset→restore 一致 ----------------
    banner("阶段3⑦ 存档往返：capture→reset→restore（场次/情报/冷却）一致");
    assert_str(
        &mut rt,
        "(function() local tp = require('systems.tea_party'); \
           local snap = tp.capture(); \
           local w_intel, w_party = tp.intel_known('npc_gu_changfeng','tp_martial'), tp.party_count(); \
           tp.reset(); \
           local r_intel, r_party = tp.intel_known('npc_gu_changfeng','tp_martial'), tp.party_count(); \
           tp.restore(snap); \
           local a_intel, a_party = tp.intel_known('npc_gu_changfeng','tp_martial'), tp.party_count(); \
           return tostring(w_intel)..','..tostring(r_intel)..','..tostring(a_intel)..'|'..\
                  tostring(w_party)..','..tostring(r_party)..','..tostring(a_party) end)()",
        "true,false,true|1,0,1",
    )?;
    println!("  capture→reset→restore：情报 true→false→true、场次 1→0→1");

    // ---------------- ⑧ 真实存档往返：provider 自注册后 save/continue ----------------
    banner(
        "阶段3⑧ 真实存档：tea_party provider 自注册 → save/continue 恢复场次/情报/好感/声望/冷却",
    );
    // main.lua 的 save 注册行由补丁接入，样例直连模块自注册（与 m48 同一处理）。
    lua(
        &mut rt,
        "require('core.save_manager').register('tea_party', require('systems.tea_party').capture, require('systems.tea_party').restore)",
    )?;
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut second = new_runtime(&project, &save_path)?;
    // 进标题之前先挂 provider（否则 continue 时 save_manager.load 不认得 "tea_party"）。
    // 注意必须在 enter_main_menu 之后——它内部会再次 execute_project_script 重载 main.lua，
    // 之前 console 注册的 provider 表会被重置（execute_project_script 与 boot 的先后关系）。
    enter_main_menu(&mut second, &main_script)?;
    lua(
        &mut second,
        "require('core.save_manager').register('tea_party', require('systems.tea_party').capture, require('systems.tea_party').restore)",
    )?;
    click_button(&mut second, "BtnContinue")?;
    assert_scene_is(&mut second, "manor")?;
    assert_num(
        &mut second,
        "require('systems.tea_party').party_count()",
        1.0,
    )?;
    assert_num(
        &mut second,
        "(function() return require('systems.tea_party').intel_known('npc_gu_changfeng','tp_martial') and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut second,
        &format!("{rel}.points('npc_gu_changfeng')"),
        29.0,
    )?;
    assert_num(&mut second, &format!("{rel}.reputation('sect')"), 2.0)?;
    // 时钟随存档回到第 8 天；拨回第 2 天 → 顾长风仍冷却（cooldown day=1 已恢复：2-1=1<7）。
    lua(&mut second, "clock.set(2, 600)")?;
    assert_str(
        &mut second,
        "(function() local ok, r = require('systems.tea_party').invite('npc_gu_changfeng'); \
         return tostring(ok)..'|'..tostring(r) end)()",
        "false|cooldown",
    )?;
    println!("  读档：场次 1、情报/好感 29/sect 声望 2 恢复；第 2 天顾长风仍 cooldown");

    println!(
        "jhyt_m57_teaparty_ok project={} topics=10 invite_threshold=20 prepare=tea_brew settle=affinity_29 reputation_sect+village_2 cooldown=7d save_roundtrip=ok",
        project.display()
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers（与 jhyt_m48_npc 同一套） ----------

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())?;
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
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
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
    let got = query_num(runtime, expr)?;
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
