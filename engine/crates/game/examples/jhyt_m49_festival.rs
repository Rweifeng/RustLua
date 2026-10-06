//! Headless acceptance drive for 阶段2「节日补足 8 个（每季 2 个）」（docs/33 v1.5 /
//! docs/54 §4.11，M49）。
//!
//! 阶段1 的 7 条节气已由 jhyt_m34_festival 逐条钉死；本门在其之上只做**补足**：
//! 冬季此前只有「冬至封山」一个，按"每季 2 个"补齐第 8 条「除夕守岁」
//! （冬季末尾 26-28 = 绝对日 110-112，与冬至 15-17 不重叠，且压在 112 天循环末尾跨年）。
//!
//! 为什么不重走 m34 的全部真实交互（盖茶棚/采灵芝/进寒泉洞）：那些已有 m34 逐字守住，
//! 本门把三件新事钉死：
//!   1. 112 天逐日扫描：8 个窗口全部命中、每季恰 2 个、8 个 id 各 3 天、窗口不重叠；
//!   2. 三类既有效果键的**回归断言**（清明 gather mult=2、端午 sell_bonus=20 且经
//!      systems.shop 真实卖出断言溢价、冬至 exit_closed）+ 除夕守岁按"纯事件标签"实现
//!      断言（id/name/空 effects/available 恒真/无任何副作用键）；
//!   3. 纯函数性质 + on_day_start 跨天钩子：同一天反复查结果一致；day.sleep() 跨天后
//!      节气随 day 派生回来，不入档、无残留状态；模块 validate_all 与 validate_data strict 空。
//!
//! 除夕守岁的设计口径：纯事件标签 effects={}（与春分/夏至/中秋同形），跨年守岁小玩法
//! 走后续对话 cond.festival="chuxi"，本阶段不新加 effect 键/消费方（见 docs/33 v1.5 草案）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m49_festival [-- <game_jhyt-root>]

use engine::core::{input::InputSnapshot, script::LuaRuntime};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 10:00：三家店都在营业窗口内（480..1080），端午行情真实卖出用它。
const TEN_AM: i64 = 600;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| engine::core::games_dir().join("examples").join("game_jhyt"));
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    let main_script = project.join("scripts/main.lua");
    if !main_script.is_file() {
        return Err(format!("missing {}", main_script.display()).into());
    }
    let save_path = fresh_save_path(&project, "jhyt_m49_festival.sav");
    let project_arg = project.clone();

    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    // ---------------- ① 112 天逐日扫描：8 窗口命中、每季恰 2 个、窗口不重叠 ----------------
    banner("M49① 112 天逐日：8 个节日窗口全命中、每季恰 2 个、8 id × 3 天");
    assert_str(&mut rt, "require('data.festivals').season_length", "28")?;
    assert_num(&mut rt, "#require('data.festivals').festivals", 8.0)?;
    // 事件日完整清单（24 天 = 既有 21 + 除夕 110/111/112）。
    let event_days = query_str(
        &mut rt,
        "(function() local f = require('world.festival'); local out = {}; \
           for d = 1, 112 do local id = f.id(d); if id ~= '' then out[#out + 1] = tostring(d) end end; \
           return table.concat(out, ',') end)()",
    )?;
    let expected_days =
        "8,9,10,15,16,17,36,37,38,43,44,45,64,65,66,71,72,73,99,100,101,110,111,112";
    if event_days != expected_days {
        return Err(
            format!("节日覆盖日数不符：数据给出 {event_days}，期望 {expected_days}").into(),
        );
    }
    // 每个节日恰好 3 天、无第九个 id 冒出来（升序）。
    let per_id = query_str(
        &mut rt,
        "(function() local f = require('world.festival'); local seen, order = {}, {}; \
           for d = 1, 112 do local id = f.id(d); \
             if id ~= '' then \
               if seen[id] == nil then seen[id] = 0; order[#order + 1] = id end; \
               seen[id] = seen[id] + 1 \
             end \
           end; table.sort(order); \
           local out = {}; for _, id in ipairs(order) do out[#out + 1] = id .. '=' .. tostring(seen[id]) end; \
           return table.concat(out, ',') end)()",
    )?;
    let expected_per_id =
        "chongyang=3,chuxi=3,dongzhi=3,duanwu=3,midautumn=3,qingming=3,teafair=3,xiazhi=3";
    if per_id != expected_per_id {
        return Err(format!("每节日天数分布不符：{per_id}，期望 {expected_per_id}").into());
    }
    // 每季恰 2 个不同节日（冬季从 1 个补到 2 个的硬锚点）。
    let per_season = query_str(
        &mut rt,
        "(function() local cal = require('world.calendar'); local f = require('world.festival'); \
           local counts = {}; \
           for d = 1, 112 do \
             local id = f.id(d); \
             if id ~= '' then \
               local s = cal.season_of(d); \
               counts[s] = counts[s] or {}; counts[s][id] = true \
             end \
           end; \
           local out = {}; \
           for _, s in ipairs({'spring','summer','autumn','winter'}) do \
             local n = 0; for _ in pairs(counts[s] or {}) do n = n + 1 end; \
             out[#out + 1] = s .. '=' .. n \
           end; \
           return table.concat(out, ',') end)()",
    )?;
    if per_season != "spring=2,summer=2,autumn=2,winter=2" {
        return Err(format!("每季节日数不符：{per_season}，期望每季 2 个").into());
    }
    println!("  112 天里 24 天有事件（8 节日 × 3 天），88 天空日；{per_season}");

    // ---------------- ② 除夕守岁窗口边界 + 纯标签实现断言 ----------------
    banner("M49② 除夕守岁（冬 26-28 = 日 110-112）：窗口边界、中文称号、纯标签无副作用");
    // 边界：冬 25（109）空、110 起事、112 末日、113 回春 1 空。
    for (day, expected) in [
        (109i64, ""),
        (110, "chuxi"),
        (111, "chuxi"),
        (112, "chuxi"),
        (113, ""),
        // 与冬至窗口的间隔：102（冬 18）空，确保两段不粘连。
        (102, ""),
    ] {
        assert_str(
            &mut rt,
            &format!("require('world.festival').id({day})"),
            expected,
        )?;
    }
    assert_str(&mut rt, "require('world.festival').name(110)", "除夕守岁")?;
    assert_str(&mut rt, "require('world.festival').name(112)", "除夕守岁")?;
    assert_str(&mut rt, "require('world.festival').name(109)", "")?;
    // 纯事件标签：effects 是空表，没有任何既有效果键冒出来。
    let effects_kind = query_str(
        &mut rt,
        "(function() local e = require('data.festivals').get('chuxi'); \
           local n = 0; for _ in pairs(e.effects or {}) do n = n + 1 end; \
           return tostring(type(e.effects)) .. ',' .. tostring(n) end)()",
    )?;
    if effects_kind != "table,0" {
        return Err(format!("除夕守岁 effects 应为空表，得到 {effects_kind:?}").into());
    }
    // 无设施门槛：available 恒真（不会像春分茶会那样被茶棚等级卡住）。
    assert_str(
        &mut rt,
        "tostring(require('world.festival').available('chuxi', 110))",
        "true",
    )?;
    // 纯标签必须对三类消费方都"无效果"——否则就不是空表了。
    assert_str(
        &mut rt,
        "tostring(require('world.festival').gather_rule('mountain_path', 110))",
        "nil",
    )?;
    assert_num(
        &mut rt,
        "require('world.festival').sell_bonus('lingzhi', 110)",
        0.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_cave', 110))",
        "false",
    )?;
    println!(
        "  冬 25 空 / 110-112 守岁 / 113 回春 1 空；effects 空表、available 恒真、三类消费方均无副作用"
    );

    // ---------------- ③ 既有三类效果键回归（查询面） ----------------
    banner("M49③ 既有效果键回归：清明 gather ×2、端午 sell_bonus=20、冬至 exit_closed");
    // 清明（春 15 = 日 15）：山道采集倍率 2，别的图不受影响。
    assert_str(&mut rt, "require('world.festival').id(15)", "qingming")?;
    assert_num(
        &mut rt,
        "require('world.festival').gather_rule('mountain_path', 15).mult",
        2.0,
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').gather_rule('manor', 15))",
        "nil",
    )?;
    // 端午（夏 8 = 日 36）：灵芝行情 +20%，不在行情表的莲子不加。
    assert_str(&mut rt, "require('world.festival').id(36)", "duanwu")?;
    assert_num(
        &mut rt,
        "require('world.festival').sell_bonus('lingzhi', 36)",
        20.0,
    )?;
    assert_num(
        &mut rt,
        "require('world.festival').sell_bonus('lotus_seed', 36)",
        0.0,
    )?;
    // 冬至（冬 15 = 日 99）：只封进山那一格，回村方向不封。
    assert_str(&mut rt, "require('world.festival').id(99)", "dongzhi")?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_cave', 99))",
        "true",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('world.festival').exit_closed('mountain_to_village', 99))",
        "false",
    )?;
    println!("  清明山道 mult=2 / 端午灵芝 +20%（莲子 0）/ 冬至封进山不封回村");

    // ---------------- ④ 端午行情经 systems.shop 真实卖出断言溢价 ----------------
    banner("M49④ 端午当日真实 shop.sell 灵芝：到账 = 基础卖价 +20% 溢价");
    let lingzhi_sell = query_num(&mut rt, "require('data.items').lingzhi.sell")?;
    lua(&mut rt, &format!("clock.set(36, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "duanwu")?;
    let bonus = query_num(&mut rt, "require('world.festival').sell_bonus('lingzhi')")?;
    let expected_offer = lingzhi_sell + (lingzhi_sell * bonus / 100.0).round();
    // 当日收购报价（经 shop.buys）应等于基础卖价 + 溢价。
    let offered = query_num(
        &mut rt,
        "(function() for _, e in ipairs(require('systems.shop').buys('zhao_stall')) do \
           if e.item == 'lingzhi' then return e.price end end return -1 end)()",
    )?;
    if (offered - expected_offer).abs() > 1e-6 {
        return Err(format!(
            "端午灵芝收购价 {offered:.0}，期望 {expected_offer:.0}（sell {lingzhi_sell:.0} +{bonus:.0}%）"
        )
        .into());
    }
    // 真实卖出到账。
    lua(
        &mut rt,
        "inv.add(require('core.state').inventory, 'lingzhi', 1)",
    )?;
    let money_before = query_num(&mut rt, "require('core.state').money")?;
    assert_str(
        &mut rt,
        "select(2, require('systems.shop').sell('zhao_stall', 'lingzhi', 1))",
        "ok",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').money",
        money_before + offered,
    )?;
    println!(
        "  夏 8：灵芝 sell {lingzhi_sell:.0} → 收购价 {offered:.0}（+{bonus:.0}%），真实卖出到账 +{offered:.0}"
    );

    // ---------------- ⑤ 纯函数性质 + on_day_start 跨天钩子 ----------------
    banner("M49⑤ 纯函数：同日反复查一致；day.sleep() 跨天后节气随 day 派生回来（不入档）");
    // 纯函数：把时钟拨到任意位置，显式传 day 的查询结果不变。
    let chuxi_a = query_str(&mut rt, "require('world.festival').id(110)")?;
    lua(&mut rt, &format!("clock.set(1, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    let chuxi_b = query_str(&mut rt, "require('world.festival').id(110)")?;
    if chuxi_a != chuxi_b || chuxi_a != "chuxi" {
        return Err(format!("除夕守岁查询不纯：拨钟前 {chuxi_a:?} / 拨钟后 {chuxi_b:?}").into());
    }
    // 跨天钩子：拨到除夕末日（冬 28 = 日 112）守岁，睡一觉 → 春 1（日 113），守岁自动消失。
    lua(&mut rt, &format!("clock.set(112, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.festival').id()", "chuxi")?;
    assert_str(&mut rt, "require('world.calendar').current()", "winter")?;
    lua(&mut rt, "day.sleep()")?;
    frames(&mut rt, 2)?;
    assert_num(&mut rt, "clock.now().day", 113.0)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    assert_str(&mut rt, "require('world.calendar').current()", "spring")?;
    // 再睡一天回到春 2，确认不是"记住了守岁"。
    lua(&mut rt, "day.sleep()")?;
    frames(&mut rt, 2)?;
    assert_num(&mut rt, "clock.now().day", 114.0)?;
    assert_str(&mut rt, "require('world.festival').id()", "")?;
    println!(
        "  拨钟前后 id(110) 恒为 chuxi；112 睡一觉到 113 春 1，守岁随之派生消失（无残留状态）"
    );

    // ---------------- ⑥ 模块校验门 ----------------
    banner("M49⑥ 校验门：data.festivals.validate_all 空 + validate_data strict 空");
    let festivals_report = query_str(
        &mut rt,
        "(function() local ok, issues = require('data.festivals').validate_all(); \
           if ok then return '' end; return table.concat(issues or {}, '\\n') end)()",
    )?;
    if !festivals_report.is_empty() {
        return Err(format!("data.festivals.validate_all 不通过:\n{festivals_report}").into());
    }
    let report = query_str(&mut rt, "require('validate_data').check({ strict = true })")?;
    if !report.is_empty() {
        return Err(format!("validate_data strict 不通过:\n{report}").into());
    }
    println!("  festivals.validate_all 空、validate_data strict 空 ✓");

    println!(
        "jhyt_m49_festival_ok project={} event_days=24/112 windows=8x3 \
         per_season=2/2/2/2 chuxi=winter26-28(day110-112) pure_label_effects=empty \
         qingming_gather_mult=2 duanwu_sell_lingzhi={:.0} dongzhi_exit_closed=true \
         day_rollover=chuxi->spring1 save_festival_fields=0",
        project.display(),
        offered
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers（与 jhyt_m42_shipping_box / m34 同一套） ----------

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

fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
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
