//! Headless acceptance drive for the codex / collection system（图鉴 6 类, docs/54 §4.12 /
//! docs/33 §4.12, M51）。
//!
//! 验证图鉴（收集）的真实链路（数据口径 = data/collections.lua 6 类条目清单 ==
//! systems/collection.lua 发现记录 / 进度 / 里程碑奖励 / 存档往返）：
//!   1. 静态：6 类注册条数与每类条目数（按实际注册表派生）；初始全部未发现；
//!      collection.validate_all 自检通过；
//!   2. 显式 notify 五连：作物稻谷(rice) / 鱼鲫鱼(crucian) / 矿铁矿石(iron_ore) /
//!      菜肴白米粥(rice_congee) / 敌人灰狼(enemy_wolf) → 对应标本解锁、各类进度正确；
//!   3. 里程碑奖励：把敌人类（共 9 条）补满到 50%（seen=5）发金币 +30、补满 100%
//!      （seen=9）再发 +100；每档只发一次（重复 notify 已见标本不重发）；
//!   4. 存档往返：capture → reset（标本清空）→ restore（标本与已发里程碑复原）；
//!   5. 负例：未知 category / 未知 id 一律拒绝；已见标本幂等返回 already；
//!   6. 数据校验门（validate_data strict）在整条链路后依然通过。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m51_codex [-- <game_jhyt-root>]

use engine::core::script::LuaRuntime;
use std::{env, path::PathBuf};

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

    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.clone());
    runtime.set_ui_viewport(1280, 720);
    runtime.set_delta(1.0 / 60.0);
    runtime.execute_project_script(&main_script)?;

    banner("① 静态：6 类注册条数与每类条目数（按实际注册表派生）、初始全未发现、validate_all 通过");
    // 新档（manor），图鉴为纯数据/系统模块，无需交互站位。
    open_main_menu(&mut runtime)?;
    click_button(&mut runtime, "BtnStart")?;
    assert_scene_is(&mut runtime, "manor")?;

    let order = query_str(
        &mut runtime,
        "(function() local c = require('systems.collection'); return table.concat(c.categories(), ',') end)()",
    )?;
    if order != "crop,fish,ore,herb,dish,enemy" {
        return Err(
            format!("图鉴 6 类顺序 = {order:?}，应为 crop,fish,ore,herb,dish,enemy").into(),
        );
    }
    // 每类条目数（派生自注册表的当前锚点）。
    assert_num(
        &mut runtime,
        "require('systems.collection').total('crop')",
        24.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').total('fish')",
        23.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').total('ore')",
        12.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').total('herb')",
        11.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').total('dish')",
        16.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').total('enemy')",
        9.0,
    )?;
    // 初始全部未发现：全局 seen=0、总额 95。
    assert_num(
        &mut runtime,
        "require('systems.collection').overall().seen",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').overall().total",
        95.0,
    )?;
    let report = query_str(
        &mut runtime,
        "return require('systems.collection').validate_all()",
    )?;
    if !report.is_empty() {
        return Err(format!("collection.validate_all 不通过:\n{report}").into());
    }
    println!(
        "  6 类 = 作物24/鱼23/矿12/药材11/菜肴16/敌人9（合计95）；初始全未发现；validate_all 空 ✓"
    );

    banner("② 显式 notify 五连：稻谷/鲫鱼/铁矿石/白米粥/灰狼 → 标本解锁、进度正确");
    notify(&mut runtime, "crop", "rice")?;
    notify(&mut runtime, "fish", "crucian")?;
    notify(&mut runtime, "ore", "iron_ore")?;
    notify(&mut runtime, "dish", "rice_congee")?;
    notify(&mut runtime, "enemy", "enemy_wolf")?;
    // 标本解锁断言
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('crop','rice') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('fish','crucian') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('ore','iron_ore') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('dish','rice_congee') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('enemy','enemy_wolf') and 1 or 0",
        1.0,
    )?;
    // 各类进度：每类 1 条
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('crop').seen",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('fish').seen",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('ore').seen",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('dish').seen",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').seen",
        1.0,
    )?;
    // 未 notify 的条目仍未发现
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('crop','radish') and 1 or 0",
        0.0,
    )?;
    // 全局进度 5/95 ≈ 5%
    assert_num(
        &mut runtime,
        "require('systems.collection').overall().seen",
        5.0,
    )?;
    println!("  五标本解锁；各类 seen=1、全局 5/95 ✓");

    banner("③ 里程碑奖励：敌人类补满到 50% 发 +30、100% 发 +100，每档只发一次");
    let money_base = query_num(&mut runtime, "require('core.state').money")?;
    // 灰狼已见(1)。依次补：ape_boss(2) whelp_1(3) whelp_2(4) —— 不到半程，不发奖。
    lua(
        &mut runtime,
        "local c=require('systems.collection'); c.notify('enemy','enemy_ape_boss'); c.notify('enemy','enemy_ape_whelp_1'); c.notify('enemy','enemy_ape_whelp_2')",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').seen",
        4.0,
    )?;
    assert_num(&mut runtime, "require('core.state').money", money_base)?; // 半程前不发奖
    // bandit(5)：跨过 ceil(9/2)=5 → 50% 发 +30
    notify(&mut runtime, "enemy", "enemy_bandit")?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_base + 30.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local g=require('systems.collection').granted('enemy'); return g[50] and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local g=require('systems.collection').granted('enemy'); return g[100] and 1 or 0 end)()",
        0.0,
    )?;
    // dummy(6) scorpion(7) wild_boar(8) —— 半程已发，不重复
    lua(
        &mut runtime,
        "local c=require('systems.collection'); c.notify('enemy','enemy_dummy'); c.notify('enemy','enemy_scorpion'); c.notify('enemy','enemy_wild_boar')",
    )?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_base + 30.0,
    )?;
    // xuanming(9)：补满 → 100% 再发 +100
    notify(&mut runtime, "enemy", "enemy_xuanming")?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_base + 130.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local g=require('systems.collection').granted('enemy'); return g[100] and 1 or 0 end)()",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').seen",
        9.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').pct",
        100.0,
    )?;
    // 重复 notify 已见标本：幂等，金币不重发
    notify(&mut runtime, "enemy", "enemy_wolf")?;
    assert_num(
        &mut runtime,
        "require('core.state').money",
        money_base + 130.0,
    )?;
    println!("  敌人类 seen=5 发 +30、seen=9 发 +100、重复见不重发；进度 9/9=100% ✓");

    banner("④ 存档往返：capture → reset（清空）→ restore（标本与里程碑复原）");
    lua(
        &mut runtime,
        "_G.__codex_blob = require('systems.collection').capture()",
    )?;
    lua(&mut runtime, "require('systems.collection').reset()")?;
    assert_num(
        &mut runtime,
        "require('systems.collection').overall().seen",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('crop','rice') and 1 or 0",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').seen",
        0.0,
    )?;
    lua(
        &mut runtime,
        "require('systems.collection').restore(_G.__codex_blob)",
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('crop','rice') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').seen('fish','crucian') and 1 or 0",
        1.0,
    )?;
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('enemy').seen",
        9.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local g=require('systems.collection').granted('enemy'); return g[100] and 1 or 0 end)()",
        1.0,
    )?;
    println!("  capture→reset→restore：5 标本 + 敌人 9/9 + 已发里程碑全部复原 ✓");

    banner("⑤ 负例：未知 category / 未知 id 拒绝；已见标本幂等 already");
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.collection').notify('bogus_cat','rice'); return ok and 1 or 0 end)()",
        0.0,
    )?;
    assert_num(
        &mut runtime,
        "(function() local ok = require('systems.collection').notify('crop','no_such_item_xyz'); return ok and 1 or 0 end)()",
        0.0,
    )?;
    // 拒绝不写标本：crop 仍只 rice 一条
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('crop').seen",
        1.0,
    )?;
    // 已见标本幂等返回 already（ok=true 但不重复发奖/不增计数）
    let already = query_str(
        &mut runtime,
        "(function() local ok, reason = require('systems.collection').notify('crop','rice'); return tostring(ok) .. ',' .. tostring(reason) end)()",
    )?;
    if already != "true,already" {
        return Err(format!("重复 notify rice = {already:?}，应为 true,already").into());
    }
    assert_num(
        &mut runtime,
        "require('systems.collection').progress('crop').seen",
        1.0,
    )?;
    println!("  未知 category/id 拒绝、不写脏数据；已见幂等 already ✓");

    banner("⑥ 数据校验门在整条链路后依然通过（validate_data strict）");
    let report = query_str(
        &mut runtime,
        "return require('validate_data').check({ strict = true })",
    )?;
    if !report.is_empty() {
        return Err(format!("校验不通过:\n{report}").into());
    }

    println!(
        "jhyt_m51_codex_ok project={} cats=crop24/fish23/ore12/herb11/dish16/enemy9 total=95 rewards=enemy@50:+30@100:+100 save=roundtrip negative=rejected",
        project.display()
    );
    Ok(())
}

fn banner(title: &str) {
    println!("\n== {title} ==");
}

fn open_main_menu(runtime: &mut LuaRuntime) -> Result<(), Box<dyn std::error::Error>> {
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

fn lua(runtime: &mut LuaRuntime, block: &str) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(block)?;
    Ok(())
}

fn notify(
    runtime: &mut LuaRuntime,
    category: &str,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    runtime.execute_console(&format!(
        "(function() local ok = require('systems.collection').notify('{category}','{id}'); if not ok then error('notify failed: {category}/{id}') end end)()"
    ))?;
    Ok(())
}

fn query_str(runtime: &mut LuaRuntime, block: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(runtime.execute_console(block)?.trim().to_owned())
}

fn query_num(runtime: &mut LuaRuntime, expr: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let value = query_str(runtime, &format!("return tostring({expr})"))?;
    value
        .parse::<f64>()
        .map_err(|error| format!("{value:?} is not a number: {error}").into())
}

fn assert_num(
    runtime: &mut LuaRuntime,
    expr: &str,
    expected: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let got = query_num(runtime, expr)?;
    if (got - expected).abs() > f64::EPSILON {
        return Err(format!("{expr} = {got}, want {expected}").into());
    }
    Ok(())
}

fn assert_scene_is(
    runtime: &mut LuaRuntime,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let scene = query_str(runtime, "return scene.reload()")?;
    if scene != expected {
        return Err(format!("scene = {scene:?}, want {expected:?}").into());
    }
    Ok(())
}
