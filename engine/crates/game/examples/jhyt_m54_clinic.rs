//! Headless acceptance drive for the 医馆经营与疫情事件 (回春堂医馆, docs/54 阶段3,
//! M54 / 2.0).
//!
//! 验证医馆经营与疫情事件的真实链路（数据口径 = data/clinic.lua ==
//! systems/clinic.lua 接诊/疫情状态机/存档）：
//!   1. 静态：data.clinic / systems.clinic 的 validate_all 通过；clinic 建筑已注册。
//!   2. 建馆/升级：放置回春堂医馆（building API），耗木料铁锭升到 2 级。
//!   3. 接诊：按病种扣药材 → 诊费入账 + 医馆声望 +，last_action=clinic_treat_<id>；
//!      药材不足拒绝（not_enough）且不写 last_action。
//!   4. 疫情触发：clock 推到夏季第 1 天（day 29）显式调 on_day_start → 夏疫激活，
//!      症状/村民好感一次性扣减/诊费乘子 1.5 生效。
//!   5. 三决策分支：治疫（耗药材、声望大涨、疫情平息）/ 囤药（兑现现金、好感小罚）/
//!      闭馆（停诊拒 clinic_closed，疫情自然消退）。
//!   6. 存档往返：capture → 改动 → restore 后声望/好感/收益统计一致。
//!   7. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m54_clinic [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::path::Path;
use std::{env, fs, path::PathBuf};

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
    let save_path = fresh_save_path(&project, "jhyt_m54_clinic.sav");
    let project_arg = project.clone();

    banner("① 静态：data.clinic / systems.clinic validate_all 通过、clinic 建筑已注册");
    let mut runtime = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut runtime, &main_script)?;
    lua(&mut runtime, "scene.load('manor')")?;
    runtime.process_scene_transition()?;
    assert_scene_is(&mut runtime, "manor")?;
    assert_validate(&mut runtime, "data.clinic")?;
    assert_validate(&mut runtime, "systems.clinic")?;
    assert_str(
        &mut runtime,
        "require('data.buildings').get('clinic').name",
        "回春堂医馆",
    )?;
    assert_num(
        &mut runtime,
        "require('data.buildings').max_level(require('data.buildings').get('clinic'))",
        2.0,
    )?;
    assert_num(&mut runtime, "require('core.state').money", 120.0)?;
    println!("  validate_all 空；clinic 建筑 2 级；新档金币 120 ✓");

    banner("② 建馆 + 升级：放置回春堂医馆，耗木料×4/铁锭×2 升到 2 级");
    let (lx, ly) = find_legal_anchor(&mut runtime, "clinic")?;
    println!("  legal anchor: {lx},{ly}");
    lua(
        &mut runtime,
        &format!("require('systems.building').place('clinic', {lx}, {ly})"),
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "build_clinic",
    )?;
    assert_num(&mut runtime, "require('systems.clinic').built() or 0", 1.0)?;
    // 升级材料不足先拒绝（无副作用）。
    assert_str(
        &mut runtime,
        &format!("select(2, require('systems.building').upgrade({lx}, {ly}))"),
        "not_enough",
    )?;
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'timber', 4); \
         inv.add(require('core.state').inventory, 'iron_ingot', 2)",
    )?;
    assert_str(
        &mut runtime,
        &format!("tostring((require('systems.building').upgrade({lx}, {ly})))"),
        "true",
    )?;
    assert_num(&mut runtime, "require('systems.clinic').built()", 2.0)?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "upgrade_clinic_2",
    )?;
    println!("  医馆建成并升至 2 级 ✓");

    banner("③ 接诊：风寒扣药材 → 诊费入账 +25、声望 +1、last_action=clinic_treat_wind_cold");
    // 备齐药材（既有 item：herb/mint/lingzhi/chrysanthemum/prunella）。
    lua(
        &mut runtime,
        "inv.add(require('core.state').inventory, 'herb', 20); \
         inv.add(require('core.state').inventory, 'mint', 20); \
         inv.add(require('core.state').inventory, 'lingzhi', 10); \
         inv.add(require('core.state').inventory, 'chrysanthemum', 10); \
         inv.add(require('core.state').inventory, 'prunella', 10)",
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.clinic').treat('wind_cold'); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_treat_wind_cold",
    )?;
    assert_num(&mut runtime, "require('core.state').money", 145.0)?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 1.0)?;
    // 风寒处方 herb×2 + mint×1。
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'herb')",
        18.0,
    )?;
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'mint')",
        19.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.clinic').stats().treated",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.clinic').stats().income",
        25.0,
    )?;
    println!("  接诊风寒：金币 120→145、声望 1、药材扣减 ✓");

    banner("④ 药材不足拒绝：内伤要 ginseng×1/polygonum×1（持有 0）→ not_enough，不写 last_action");
    assert_str(
        &mut runtime,
        "(function() local ok, r = require('systems.clinic').treat('internal_injury'); \
         return tostring(ok) .. '|' .. tostring(r) end)()",
        "false|not_enough",
    )?;
    // 失败不写 last_action：仍是上一次成功接诊的 clinic_treat_wind_cold。
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_treat_wind_cold",
    )?;
    println!("  药材不足拒绝且 last_action 未被覆写 ✓");

    banner("⑤ 疫情触发：clock 到夏季第 1 天（day 29）显式 on_day_start → 夏疫激活");
    lua(&mut runtime, "clock.set(29, 0)")?;
    lua(&mut runtime, "require('systems.clinic').on_day_start()")?;
    assert_str(
        &mut runtime,
        "(function() local s = require('systems.clinic').status(); \
         return tostring(s.active) .. '|' .. tostring(s.id) .. '|' .. tostring(s.days_left) \
           .. '|' .. tostring(s.fee_mult) .. '|' .. tostring(s.closed) end)()",
        "true|summer_plague|5|1.5|false",
    )?;
    // 被动惩罚：激活当日村民好感一次性 -5。
    assert_num(&mut runtime, "require('systems.clinic').favor()", -5.0)?;
    // 疫情期间诊费 ×1.5：风寒基础 25 → round(37.5)=38。
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.clinic').treat('wind_cold'); return ok and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(&mut runtime, "require('core.state').money", 183.0)?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 2.0)?;
    println!("  夏疫激活、好感 -5、诊费 ×1.5（25→38）✓");

    banner("⑥ 决策分支一·治疫：耗 lingzhi×3/chrysanthemum×2 → 声望+15、好感+12、疫情平息");
    assert_str(
        &mut runtime,
        "(function() local ok = require('systems.clinic').decide('treat'); return tostring(ok) end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_decision_treat",
    )?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 17.0)?;
    assert_num(&mut runtime, "require('systems.clinic').favor()", 7.0)?;
    assert_num(
        &mut runtime,
        "(function() return require('systems.clinic').status().active and 1 or 0 end)()",
        0.0,
    )?;
    // lingzhi 由 10 扣 3 = 7。
    assert_num(
        &mut runtime,
        "inv.count(require('core.state').inventory, 'lingzhi')",
        7.0,
    )?;
    println!("  治疫：声望 2→17、好感 -5→7、疫情平息 ✓");

    banner("⑦ 决策分支二·囤药：立即兑现 +60 文、好感 -5，疫情按日消退");
    lua(
        &mut runtime,
        "(function() local ok = require('systems.clinic').force_trigger('autumn_fever'); return tostring(ok) end)()",
    )?;
    // 激活秋瘟：好感 7 → 7-4 = 3。
    assert_num(&mut runtime, "require('systems.clinic').favor()", 3.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok = require('systems.clinic').decide('stockpile'); return tostring(ok) end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_decision_stockpile",
    )?;
    // 兑现 +60：金币 183 → 243；好感 3 → 3-5 = -2。
    assert_num(&mut runtime, "require('core.state').money", 243.0)?;
    assert_num(&mut runtime, "require('systems.clinic').favor()", -2.0)?;
    // 秋瘟持续 4 天：显式 on_day_start 递减至自然消退（0..4 = 4 次递减）。
    for _ in 0..4 {
        lua(&mut runtime, "require('systems.clinic').on_day_start()")?;
    }
    assert_num(
        &mut runtime,
        "(function() return require('systems.clinic').status().active and 1 or 0 end)()",
        0.0,
    )?;
    println!("  囤药：金币 183→243、好感 -2、秋瘟消退 ✓");

    banner("⑧ 决策分支三·闭馆：停诊拒 clinic_closed，疫情自然消退后复馆");
    lua(
        &mut runtime,
        "(function() local ok = require('systems.clinic').force_trigger('winter_cold'); return tostring(ok) end)()",
    )?;
    // 激活冬瘟：好感 -2 → -2-6 = -8。
    assert_num(&mut runtime, "require('systems.clinic').favor()", -8.0)?;
    assert_str(
        &mut runtime,
        "(function() local ok = require('systems.clinic').decide('close'); return tostring(ok) end)()",
        "true",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_decision_close",
    )?;
    // 闭馆期间接诊被拒，且不写 last_action（仍是 clinic_decision_close）。
    assert_str(
        &mut runtime,
        "(function() local ok, r = require('systems.clinic').treat('wind_cold'); \
         return tostring(ok) .. '|' .. tostring(r) end)()",
        "false|clinic_closed",
    )?;
    assert_str(
        &mut runtime,
        "require('core.state').last_action",
        "clinic_decision_close",
    )?;
    // 冬瘟持续 6 天：递减至消退，闭馆自动解除（0..6 = 6 次递减）。
    for _ in 0..6 {
        lua(&mut runtime, "require('systems.clinic').on_day_start()")?;
    }
    assert_num(
        &mut runtime,
        "(function() return require('systems.clinic').status().active and 1 or 0 end)()",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() return require('systems.clinic').is_closed() and 1 or 0 end)()",
        0.0,
    )?;
    println!("  闭馆：停诊被拒、冬瘟消退、自动复馆 ✓");

    banner("⑨ 存档往返：capture → 接诊改动 → restore，声望/好感/收益一致");
    // main.lua 补丁尚未集成前，样例内自行注册 provider（与补丁接线一致）。
    lua(
        &mut runtime,
        "require('core.save_manager').register('clinic', \
           require('systems.clinic').capture, require('systems.clinic').restore)",
    )?;
    lua(&mut runtime, "__c = require('core.save_manager').capture()")?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 17.0)?;
    assert_num(&mut runtime, "require('systems.clinic').favor()", -8.0)?;
    assert_num(
        &mut runtime,
        "require('systems.clinic').stats().income",
        63.0,
    )?;
    // 改动：再接诊一次风寒（疫情已退，诊费回到 25）。
    lua(
        &mut runtime,
        "(function() require('systems.clinic').treat('wind_cold') end)()",
    )?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 18.0)?;
    assert_num(
        &mut runtime,
        "require('systems.clinic').stats().income",
        88.0,
    )?;
    // restore 回滚整批（含 clinic provider）。
    assert_str(
        &mut runtime,
        "(function() local ok, why = require('core.save_manager').restore(__c); \
         return tostring(ok) .. '|' .. tostring(why) end)()",
        "true|nil",
    )?;
    assert_num(&mut runtime, "require('systems.clinic').reputation()", 17.0)?;
    assert_num(&mut runtime, "require('systems.clinic').favor()", -8.0)?;
    assert_num(
        &mut runtime,
        "require('systems.clinic').stats().income",
        63.0,
    )?;
    println!("  存档往返一致：声望 17 / 好感 -8 / 收益 63 ✓");

    banner("⑩ 数据校验门（validate_data strict）在整条链路后依然通过");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m54_clinic_ok project={} clinic=built@{}|{} treat=wind_cold fee=145 plague=summer stockpile=243 close=blocked save=roundtrip",
        project.display(),
        lx,
        ly
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- helpers ----------

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

fn find_legal_anchor(
    runtime: &mut LuaRuntime,
    building_id: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() local b = require('systems.building'); \
         for y = 0, 21 do for x = 0, 39 do \
           local ok = b.can_place('{building_id}', x, y); \
           if ok then return x .. ',' .. y end end end return 'none' end)()"
    );
    let got = query_str(runtime, &expr)?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("no legal anchor for {building_id}: {got}"))?;
    Ok((x, y))
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

fn assert_validate(
    runtime: &mut LuaRuntime,
    module: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "local ok, issues = require('{module}').validate_all(); \
             if not ok then return table.concat(issues, ' | ') end; return 'ok'"
        ),
    )?;
    if got != "ok" {
        return Err(format!("{module}.validate_all failed: {got}").into());
    }
    Ok(())
}

fn assert_scene_is(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = query_str(runtime, "scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected}").into());
    }
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn fresh_save_path(project: &Path, name: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = fs::remove_file(&path);
    path
}
