//! Headless acceptance drive for 阶段3「节气历法（二十四节气）」（docs/33 2.1，M55）。
//!
//! 设计口径（先读 docs/33 §2.1 时间日历后定）：每季 28 天、四季 112 天循环。传统
//! 二十四节气在本游戏里**每季取 6 个**（立春→大寒，共 24 个），按"季内第几天"单日生效
//! （不像阶段2 节日是 3 天窗口）。节气日刻意压在 8 个节日窗口的正中央——春分=春 9
//! （茶会 8-10 正中）、清明=春 16（踏青 15-17 正中）、冬至=冬 16（封山 15-17 正中）……
//! 节气是节日的天文原点，两套时间轴只读对照（festival_link），互不改动。
//!
//! 本门钉死六件事（不重走 m34/m49 的真实交互——那些已有样例逐字守住）：
//!   1. 注册数 24、季内日 1..28 合法；112 天逐日扫描恰 24 天有节气、无一天撞两个、
//!      每季恰 6 个；
//!   2. 修饰查询面：清明（绝对日 16）season_modifiers().gather_mult=2，次日（非节气日）
//!      回落到默认 1——经系统查询而非真实采集；
//!   3. 与 8 个阶段2 节日的对应关系：每个被 link 的节气，其绝对日落在对应节日窗口内
//!      （只读数据，不改 festivals.lua）；
//!   4. 纯函数性质：同一天反复查一致（拨钟不影响显式传 day 的查询）；
//!   5. on_day_start 钩子显式调用：回当日节气 id，且不落状态（跨天无残留）；
//!   6. 校验门：data.solar_terms.validate_all 空、systems.solar_terms.validate_all 空、
//!      validate_data strict 空。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m55_solarterm [-- <game_jhyt-root>]

use rustlua_input::input::InputSnapshot;
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

const TEN_AM: i64 = 600;

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
    let save_path = fresh_save_path(&project, "jhyt_m55_solarterm.sav");
    let project_arg = project.clone();

    let (mut rt, host) = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    // ---------------- ① 注册数 + 季内日合法 + 112 天逐日扫描 ----------------
    banner("M55① 注册 24 节气、季内日 1..28；112 天逐日恰 24 天有节气、无撞日、每季 6 个");
    assert_num(&mut rt, "#require('data.solar_terms').terms", 24.0)?;
    assert_str(&mut rt, "require('data.solar_terms').season_length", "28")?;
    let event_days = query_str(
        &mut rt,
        "(function() local s = require('systems.solar_terms'); local out = {}; \
           for d = 1, 112 do local id = s.id(d); if id ~= '' then out[#out + 1] = tostring(d) end end; \
           return table.concat(out, ',') end)()",
    )?;
    let expected_days = "1,4,7,9,16,23,29,32,37,44,49,54,57,60,65,69,72,79,85,88,93,100,105,111";
    if event_days != expected_days {
        return Err(
            format!("节气覆盖日数不符：数据给出 {event_days}，期望 {expected_days}").into(),
        );
    }
    // 无一天撞两个节气（单日生效）：每天最多一个 id，24 天各出现一次。
    let hit_count = query_num(
        &mut rt,
        "(function() local s = require('systems.solar_terms'); \
           local n = 0; for d = 1, 112 do if s.id(d) ~= '' then n = n + 1 end end; return n end)()",
    )?;
    if hit_count != 24.0 {
        return Err(format!("112 天里应恰 24 天有节气，得到 {hit_count}").into());
    }
    let per_season = query_str(
        &mut rt,
        "(function() local cal = require('world.calendar'); local s = require('systems.solar_terms'); \
           local counts = {}; \
           for d = 1, 112 do \
             local id = s.id(d); \
             if id ~= '' then local sec = cal.season_of(d); counts[sec] = (counts[sec] or 0) + 1 end \
           end; \
           local out = {}; \
           for _, sec in ipairs({'spring','summer','autumn','winter'}) do \
             out[#out + 1] = sec .. '=' .. tostring(counts[sec] or 0) end; \
           return table.concat(out, ',') end)()",
    )?;
    if per_season != "spring=6,summer=6,autumn=6,winter=6" {
        return Err(format!("每季节气数不符：{per_season}，期望每季 6 个").into());
    }
    println!("  112 天里恰 24 天有节气（{per_season}），88 天无节气；单日生效无撞日");

    // ---------------- ② 修饰查询面：清明 gather_mult=2，次日回落默认 ----------------
    banner("M55② 修饰查询面：清明（绝对日16）gather_mult=2；次日（非节气日）回落默认 1");
    assert_str(&mut rt, "require('systems.solar_terms').id(16)", "qingming")?;
    assert_str(&mut rt, "require('systems.solar_terms').name(16)", "清明")?;
    assert_num(
        &mut rt,
        "require('systems.solar_terms').season_modifiers(16).gather_mult",
        2.0,
    )?;
    // 绝对日 17 = 春 17：是清明踏青**节日**窗口（15-17），但不是节气日（清明=春16）。
    // 节气修饰查询面应当回落默认，证明节气(单日) 与节日(3天窗口) 是两套轴。
    assert_str(&mut rt, "require('systems.solar_terms').id(17)", "")?;
    assert_num(
        &mut rt,
        "require('systems.solar_terms').season_modifiers(17).gather_mult",
        1.0,
    )?;
    assert_num(
        &mut rt,
        "require('systems.solar_terms').season_modifiers(17).sell_bonus",
        0.0,
    )?;
    // 春分（绝对日 9）：sell_bonus=10（茶会行情），gather_mult 回落默认 1。
    assert_str(&mut rt, "require('systems.solar_terms').id(9)", "chunfen")?;
    assert_num(
        &mut rt,
        "require('systems.solar_terms').season_modifiers(9).sell_bonus",
        10.0,
    )?;
    assert_num(
        &mut rt,
        "require('systems.solar_terms').season_modifiers(9).gather_mult",
        1.0,
    )?;
    println!("  清明日16 gather_mult=2 / 次日17 回落默认1 / 春分日9 sell_bonus=10");

    // ---------------- ③ 与 8 个阶段2 节日的对应关系（只读） ----------------
    banner("M55③ 节气→节日只读对照：8 个 link 的节气绝对日都落在对应节日窗口内");
    let links = query_str(
        &mut rt,
        "(function() return table.concat(require('systems.solar_terms').festival_links(), ',') end)()",
    )?;
    let expected_links = "bailu->midautumn,chunfen->teafair,dahan->chuxi,dongzhi->dongzhi,\
        hanlu->chongyang,mangzhong->duanwu,qingming->qingming,xiazhi->xiazhi";
    if links != expected_links {
        return Err(format!("节气→节日对照不符：{links}，期望 {expected_links}").into());
    }
    // 逐 link 校验：节气绝对日 ∈ 该节日 [from,to] 窗口（跨 season 边界换算绝对日）。
    let inside = query_str(
        &mut rt,
        "(function() local cal = require('world.calendar'); \
           local fdata = require('data.festivals'); \
           local sdata = require('data.solar_terms'); \
           local SL = fdata.season_length; local S = {spring=1,summer=2,autumn=3,winter=4}; \
           local bad = {}; \
           for _, t in ipairs(sdata.terms) do \
             if t.festival_link then \
               local fest = fdata.get(t.festival_link); \
               local abs = (S[t.season]-1)*SL + t.day_in_season; \
               local fabs_from = (S[fest.season]-1)*SL + fest.from; \
               local fabs_to = (S[fest.season]-1)*SL + fest.to; \
               if abs < fabs_from or abs > fabs_to then \
                 bad[#bad + 1] = t.id .. '@' .. tostring(abs) .. 'notin' .. fest.id \
               end \
             end \
           end; \
           if #bad == 0 then return 'ok' end; return table.concat(bad, ',') end)()",
    )?;
    if inside != "ok" {
        return Err(format!("节气未落在对应节日窗口内：{inside}").into());
    }
    println!("  8 个 link 节气绝对日均落在节日窗口正中（春分↔茶会/冬至↔封山/大寒↔守岁…）");

    // ---------------- ④ 纯函数性质：拨钟不影响显式传 day 的查询 ----------------
    banner("M55④ 纯函数：同日反复查一致，拨钟前后 id(16) 恒为 qingming");
    let a = query_str(&mut rt, "require('systems.solar_terms').id(16)")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, &host, 2)?;
    let b = query_str(&mut rt, "require('systems.solar_terms').id(16)")?;
    if a != b || a != "qingming" {
        return Err(format!("节气查询不纯：拨钟前 {a:?} / 拨钟后 {b:?}").into());
    }
    println!("  拨钟前后 id(16) 恒为 qingming（纯 day 派生，无状态）");

    // ---------------- ⑤ on_day_start 钩子显式调用 + 跨天无残留 ----------------
    banner("M55⑤ on_day_start 回当日节气 id；跨天后节气随 day 派生（不入档）");
    lua(&mut rt, &format!("clock.set(9, {TEN_AM})"))?;
    frames(&mut rt, &host, 2)?;
    assert_str(&mut rt, "require('systems.solar_terms').id()", "chunfen")?;
    assert_str(
        &mut rt,
        "require('systems.solar_terms').on_day_start()",
        "chunfen",
    )?;
    // 睡一天到日 10：节气从春分(日9) 消失（单日生效），节日茶会(8-10) 仍在是另一轴。
    lua(&mut rt, "day.sleep()")?;
    frames(&mut rt, &host, 2)?;
    assert_num(&mut rt, "clock.now().day", 10.0)?;
    assert_str(&mut rt, "require('systems.solar_terms').id()", "")?;
    // capture/restore/reset 空实现：capture 恒空表、restore 恒真、reset 不报错。
    assert_str(
        &mut rt,
        "(function() local n = 0; for _ in pairs(require('systems.solar_terms').capture()) do n = n + 1 end; return tostring(n) end)()",
        "0",
    )?;
    lua(&mut rt, "require('systems.solar_terms').reset()")?;
    println!("  日9 on_day_start=chunfen；睡到日10 节气消失（单日），capture 空、reset 无副作用");

    // ---------------- ⑥ 校验门 ----------------
    banner("M55⑥ 校验门：data.solar_terms.validate_all 空 + systems.validate_all 空 + strict 空");
    let data_report = query_str(
        &mut rt,
        "(function() local ok, issues = require('data.solar_terms').validate_all(); \
           if ok then return '' end; return table.concat(issues or {}, '\\n') end)()",
    )?;
    if !data_report.is_empty() {
        return Err(format!("data.solar_terms.validate_all 不通过:\n{data_report}").into());
    }
    let sys_report = query_str(&mut rt, "require('systems.solar_terms').validate_all()")?;
    if !sys_report.is_empty() {
        return Err(format!("systems.solar_terms.validate_all 不通过:\n{sys_report}").into());
    }
    let report = query_str(&mut rt, "require('validate_data').check({ strict = true })")?;
    if !report.is_empty() {
        return Err(format!("validate_data strict 不通过:\n{report}").into());
    }
    println!("  data/systems validate_all 均空、validate_data strict 空 ✓");

    println!(
        "jhyt_m55_solarterm_ok project={} terms=24 hit_days=24/112 per_season=6x4 \
         qingming_day16_gather_mult=2 chunfen_day9_sell_bonus=10 \
         festival_links=8 inside_windows pure=derived save_fields=0",
        project.display()
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers（与 jhyt_m49_festival / m42 同一套） ----------

fn new_runtime(
    project: &Path,
    save_path: &Path,
) -> Result<(LuaRuntime, rustlua_input::host::HostInput), Box<dyn std::error::Error>> {
    let (installers, handles) = rustlua_gameplay::input_lua::registry_with_handles();
    let host = handles.input;
    let mut runtime = LuaRuntime::with_installers(installers)?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    Ok((runtime, host))
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

fn frames(
    runtime: &mut LuaRuntime,
    host: &rustlua_input::host::HostInput,
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    host.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
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
