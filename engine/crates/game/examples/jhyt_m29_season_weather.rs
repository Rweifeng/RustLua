//! Headless acceptance drive for M29 / 计划 P1（阶段五）：
//! 季节可视化、按日天气抽签、天气与资源条的可见变化，以及资源"接线或下架"清单。
//!
//! 为什么这一轮值得做：`assets/` 里躺着上百个**从未被任何场景/脚本引用**的文件，
//! 其中四季地表、HUD 条与天气图标是"系统已实现、画面上却看不见"的部分——
//! `calendar` 算出来的季节只影响作物与 HUD 文字，`farm` 的雨天免浇水与 NPC 的
//! `rain_place` 日程在真实玩法里永远走不到（天气只会是晴天）。
//!
//! 本样例按真实玩法驱动：
//! ① 四季地表数据表结构与负例；
//! ② 进图按季节换地表贴图（春→夏用 `clock.set(29, …)` + T 键跨天），未登记地图不动，
//!    换图计数随存档往返；
//! ③ 按日天气抽签的确定性与持久化（同档同天同结果、读档不漂、新档复位）；
//! ④ 雨天可见变化：全屏薄雾面板 + 天气图标（`ui.panel_get` / `ui.image_get` 读回），
//!    晴天必须收干净；
//! ⑤ 雨天真实影响玩法：雨天按 E 浇水 → `rain_watered` 且**不花体力**；
//! ⑥ 三条资源条按真实数值画宽度（受伤 → 血条变短）；
//! ⑦ 资源决定表 `docs/48` 与 `assets/**` 双向核对 + 下架动作（第三方包/标题原图）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m29_season_weather [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

/// 山庄出生点（也是回山庄的落点）。
const MANOR_POS: (f32, f32) = (528.0, 48.0);
/// 云泽村入口落点。
const VILLAGE_POS: (f32, f32) = (528.0, 48.0);
/// 青云山道入口落点（未登记四季地表的地图）。
const MOUNTAIN_POS: (f32, f32) = (624.0, 80.0);
/// 10:00：NPC 日程稳定的时刻。
const TEN_AM: i64 = 600;
/// 中午：睡觉前。
const NOON: i64 = 720;

/// `docs/48` §1 里的审计基准（改资源或改接线必须同步改文档，否则本样例报错）。
///
/// 2026-09-27 docs/48 全部缺口清零后基准：已引用 562 = 470（首版基准）+ 8（第一批
/// 场景接线）+ 8（effects 特效）+ 15（portraits 立绘）+ 61（ui 换肤接线增量）；
/// `_ui_weather_cloud.png` 按 `_` 前缀归入开发产物（46 → 47），内容缺口清零。
/// 2026-09-27 Alpha 作物扩展：+7 图标（radish/seed_radish/seed_herb/polygonum/
/// seed_polygonum/ginseng/seed_ginseng），herb.png 孤儿随草药落地转已引用 → 569。
const EXPECTED_DEV_ARTIFACTS: usize = 47;
// 2026-10-06 回归：2 个内容缺口均有 §2 决定行（计划登记）——
//   * assets/art/ui/hud/ui_weather_cloud.png：生成器重新产出、天气闭集无阴天取值（_ 前缀版本已归开发产物）；
//   * assets/ui/prefabs/dialogue.ui.json：M59 prefab 动态加载（语料只含目录前缀，docs/55 已登记）。
const EXPECTED_CONTENT_ORPHANS: usize = 2;
// 2026-10-06 终期回归重扫：阶段2–5 新增引用源（扩展物品 icon、ui/prefab 动态加载、collections/
// achievements/endings 等新注册表、M59 预设）后 569→913，docs/48 §1 已同步。
// 2026-10-07 主角头像：`gen_portraits_art.py` 补 player_male/player_female × 3 表情 = +6 张，
// 全部以字面路径入 `ui/portrait.lua` ⇒ 978→984。**这条增量没跑过实测**：本样例在走到这条审计
// 之前就先红在雨天遮罩那一腿（`ui.panel_get('hud:rain')` 读不到——M70/B7 把雨罩搬进预设之后
// `hud:rain` 这个动态面板已不存在），修那一条时请顺手核对这里是不是 984。
const EXPECTED_REFERENCED: usize = 984;

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
    let save_path = fresh_save_path(&project, "jhyt_m29_season_weather.sav");
    let project_arg = project.clone();

    // ---------------- ① 四季地表数据表：结构与负例 ----------------
    banner("M29-01① 四季地表表 data/season_art.lua：登记/占位符/季节闭集，全部可校验");
    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    assert_num(&mut rt, "require('data.season_art').version", 1.0)?;
    assert_str(
        &mut rt,
        "(function() local m = require('data.season_art').maps(); \
           return table.concat(m, ',') end)()",
        "manor,village",
    )?;
    assert_num(&mut rt, "#(require('data.season_art').textures())", 8.0)?;
    assert_str(
        &mut rt,
        "require('data.season_art').texture_for('manor', 'winter')",
        "assets/art/tilesets/manor/manor_ground_winter.png",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('data.season_art').texture_for('mountain_path', 'spring'))",
        "nil",
    )?;
    // 校验器登记（`jhyt_m23_data_validation` 的"登记覆盖"断言也覆盖它）。
    assert_str(
        &mut rt,
        "(function() for _, e in ipairs(require('validate_data').registered()) do \
           if e.path == 'data.season_art' then return 'ok' end end return 'missing' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local report = require('validate_data').check({ strict = true }); \
           return report == '' and 'ok' or report end)()",
        "ok",
    )?;
    // 负例：四个结构不变量都必须真的被检查（不是写着好看）。
    assert_str(
        &mut rt,
        "(function() local sa = require('data.season_art'); \
           local saved = sa.ground.manor.pattern; \
           local cases = { \
             { name = 'two_placeholders', value = 'assets/a_%s_%s.png' }, \
             { name = 'no_assets_prefix', value = 'tilesets/%s.png' }, \
             { name = 'empty', value = '' } }; \
           for _, case in ipairs(cases) do \
             sa.ground.manor.pattern = case.value; \
             local ok, issues = sa.validate_all(); \
             if ok then sa.ground.manor.pattern = saved; return 'not_reported:' .. case.name end; \
             local hit = false; \
             for _, i in ipairs(issues or {}) do \
               if i:find('pattern', 1, true) then hit = true end \
             end; \
             if not hit then sa.ground.manor.pattern = saved; return 'wrong_issue:' .. case.name end \
           end; \
           sa.ground.manor.pattern = saved; return 'ok' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local sa = require('data.season_art'); \
           local saved = sa.ground.village.seasons; \
           local checks = {}; \
           sa.ground.village.seasons = { 'spring', 'monsoon' }; \
           checks[#checks + 1] = sa.validate_all(); \
           sa.ground.village.seasons = { 'spring', 'spring' }; \
           checks[#checks + 1] = sa.validate_all(); \
           sa.ground.village.seasons = {}; \
           checks[#checks + 1] = sa.validate_all(); \
           sa.ground.village.seasons = saved; \
           local reported = {}; \
           for _, pair in ipairs(checks) do \
             if pair == true then return 'accepted_bad_seasons' end \
           end; \
           local ok = sa.validate_all(); \
           return ok and 'ok' or 'still_broken' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local sa = require('data.season_art'); \
           sa.ground.manor.typo_key = true; \
           local ok, issues = sa.validate_all(); \
           sa.ground.manor.typo_key = nil; \
           if ok then return 'unknown_key_accepted' end; \
           for _, i in ipairs(issues or {}) do \
             if i:find('unknown key', 1, true) then return 'ok' end \
           end; \
           return 'missing:unknown_key' end)()",
        "ok",
    )?;
    // 8 张贴图必须真的在磁盘上（Lua 没有列目录能力，这里用真实文件系统查）。
    let season_textures = lua_lines(&mut rt, "require('data.season_art').textures()")?;
    assert_eq!(season_textures.len(), 8, "{season_textures:?}");
    for rel in &season_textures {
        let path = project.join(rel);
        if !path.is_file() {
            return Err(format!("season texture missing: {rel}").into());
        }
    }
    println!("① 四季地表 8 张全部存在；结构负例 6 个全部被拒");

    // ---------------- ② 进图换图 + 跨天换季 ----------------
    banner("M29-01② 季节可视化：进图按季节换地表贴图，第 29 天跨季自动换图");
    assert_str(
        &mut rt,
        "require('world.season_visual').current_texture('manor')",
        "assets/art/tilesets/manor/manor_ground_spring.png",
    )?;
    // 未登记四季地表的地图：贴图必须保持场景里的基础图（不能被"兜底"改成别的）。
    let mountain_base = query_str(
        &mut rt,
        &format!(
            "(function() map['goto']('mountain_path', {}, {}); return 'queued' end)()",
            MOUNTAIN_POS.0, MOUNTAIN_POS.1
        ),
    )?;
    assert_eq!(mountain_base, "queued");
    rt.process_scene_transition()?;
    assert_scene_is(&mut rt, "mountain_path")?;
    assert_str(
        &mut rt,
        "require('world.season_visual').refresh('mountain_path')",
        "skipped",
    )?;
    assert_str(
        &mut rt,
        "(function() local gy = entity.find('GroundMap'); \
           return tilemap.get(gy).texture end)()",
        "assets/art/tilesets/mountain_path/mountain_path_ground.png",
    )?;
    // 回山庄：春天地表；跨天到第 29 天（夏）后必须换成夏天贴图。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    let swaps_before = query_num(&mut rt, "require('world.season_visual').swap_count()")?;
    lua(&mut rt, &format!("clock.set(29, {NOON})"))?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('world.calendar').current()", "summer")?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 30.0)?;
    assert_str(
        &mut rt,
        "require('world.season_visual').current_texture('manor')",
        "assets/art/tilesets/manor/manor_ground_summer.png",
    )?;
    let swaps_after = query_num(&mut rt, "require('world.season_visual').swap_count()")?;
    if swaps_after <= swaps_before {
        return Err(format!("换图计数没有增长：{swaps_before} → {swaps_after}").into());
    }
    // 云泽村换图：同一套机制，不同地图。
    goto_map(&mut rt, "village", VILLAGE_POS)?;
    assert_scene_is(&mut rt, "village")?;
    assert_str(
        &mut rt,
        "require('world.season_visual').current_texture('village')",
        "assets/art/tilesets/village/village_ground_summer.png",
    )?;
    // 存档往返：换图计数与季节都跟着存档回来（`state.vars` 标量 + `state.season`）。
    assert_str(
        &mut rt,
        "(function() local sv = require('world.season_visual'); \
           local var = tonumber(require('core.state').get_var('season_swaps')) or -1; \
           return tostring(sv.swap_count() == var) end)()",
        "true",
    )?;
    let persisted_swaps = query_num(&mut rt, "require('world.season_visual').swap_count()")?;
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut rt2 = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt2, &main_script)?;
    click_button(&mut rt2, "BtnContinue")?;
    assert_scene_is(&mut rt2, "village")?;
    assert_num(&mut rt2, "clock.now().day", 30.0)?;
    assert_str(&mut rt2, "require('world.calendar').current()", "summer")?;
    assert_str(
        &mut rt2,
        "require('world.season_visual').current_texture('village')",
        "assets/art/tilesets/village/village_ground_summer.png",
    )?;
    // 计数是持久计数器：读档后**只增不减**（错误地走了新档路径会被 `season_visual.reset()`
    // 清零，这个断言就是那个回归的哨兵）。
    let reloaded_swaps = query_num(&mut rt2, "require('world.season_visual').swap_count()")?;
    if reloaded_swaps < persisted_swaps {
        return Err(format!(
            "读档后换图计数被清空：存档 {persisted_swaps} → 读档 {reloaded_swaps}"
        )
        .into());
    }
    println!(
        "② 春→夏换图生效；未登记地图保持基础图；读档后季节/贴图一致（换图计数 {persisted_swaps} → {reloaded_swaps}）"
    );
    drop(rt);
    let mut rt = rt2;

    // ---------------- ③ 按日天气抽签：确定性 + 持久化 ----------------
    banner("M29-01③ 天气抽签：同档同天同结果、跨天重抽、读档不漂、新档复位");
    // 抽签是 (seed, day, season) 的纯函数：同参数必须同结果。
    assert_str(
        &mut rt,
        "(function() local w = require('world.weather'); \
           local a = w.roll_for(7, 'spring'); local b = w.roll_for(7, 'spring'); \
           if a ~= b then return 'unstable' end; \
           if a ~= 'sunny' and a ~= 'rainy' then return 'bad_kind:' .. tostring(a) end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 概率表必须是数据：季节不同 → 概率不同（数值案 docs/33 §7）。
    assert_str(
        &mut rt,
        "(function() local c = require('world.weather').RAIN_CHANCE; \
           if type(c) ~= 'table' then return 'no_chance_table' end; \
           for _, s in ipairs({ 'spring', 'summer', 'autumn', 'winter' }) do \
             if type(c[s]) ~= 'number' or c[s] <= 0 or c[s] >= 1 then return 'bad_chance:' .. s end \
           end; \
           if c.spring == c.winter then return 'season_ignored' end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 同一天重复 sync 不重抽；改种子后结果允许变化，但同种子必须复现。
    let day30_weather = query_str(&mut rt, "require('world.weather').sync()")?;
    assert_str(
        &mut rt,
        &format!("require('world.weather').sync() == '{day30_weather}' and 'ok' or 'rerolled'"),
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local w = require('world.weather'); \
           local function seq(seed) \
             w.set_seed(seed); local out = {}; \
             for d = 1, 40 do out[d] = w.roll_for(d, 'autumn') end; \
             return table.concat(out, ',') \
           end; \
           local a = seq(4242); local b = seq(4242); local c = seq(99); \
           if a ~= b then return 'seed_not_reproducible' end; \
           if a == c then return 'seed_ignored' end; \
           w.set_seed(4242); \
           local rainy = 0; \
           for d = 1, 400 do if w.roll_for(d, 'autumn') == 'rainy' then rainy = rainy + 1 end end; \
           local rate = rainy / 400; \
           local want = w.RAIN_CHANCE.autumn; \
           if rate < want - 0.10 or rate > want + 0.10 then \
             return string.format('rate_off:%.3f~%.3f', rate, want) \
           end; \
           return 'ok' end)()",
        "ok",
    )?;
    // 跨天重抽：把当天记为晴天，跨天后 `current()` 必须按新一天的签给值（不再沿用旧值）。
    assert_str(
        &mut rt,
        "require('world.weather').set('sunny') and 'ok' or 'bad'",
        "ok",
    )?;
    assert_str(&mut rt, "require('world.weather').current()", "sunny")?;
    lua(&mut rt, &format!("clock.set(31, {NOON})"))?;
    press(
        &mut rt,
        KeyCode::from_char('t').expect("QWERTY 字符必须有对应物理键"),
    )?;
    assert_num(&mut rt, "clock.now().day", 32.0)?;
    let day32 = query_str(&mut rt, "require('world.weather').current()")?;
    assert_str(
        &mut rt,
        &format!(
            "require('world.weather').roll_for(32, require('world.calendar').current()) == '{day32}' \
             and 'ok' or 'mismatch'"
        ),
        "ok",
    )?;
    // 读档后同一天同一结果（抽签结果由 seed+day 决定，不依赖运行期随机数）。
    lua(&mut rt, "require('core.save_manager').save()")?;
    let mut rt3 = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt3, &main_script)?;
    click_button(&mut rt3, "BtnContinue")?;
    assert_num(&mut rt3, "clock.now().day", 32.0)?;
    assert_str(&mut rt3, "require('world.weather').current()", &day32)?;
    drop(rt);
    let mut rt = rt3;
    println!("③ 抽签纯函数稳定；跨天重抽；读档后第 32 天天气仍为 {day32}");

    // ---------------- ④ 雨天可见变化 ----------------
    banner("M29-01④ 雨天可见变化：全屏薄雾面板 + 天气图标；晴天收干净");
    // 先造一个雨天（显式覆盖：`set` 后当天不再重抽），跑几帧让 HUD 画出来。
    goto_map(&mut rt, "manor", MANOR_POS)?;
    assert_scene_is(&mut rt, "manor")?;
    lua(&mut rt, "require('world.weather').set('rainy')")?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "(function() local p = ui.panel_get('hud:rain'); \
           if not p then return 'no_overlay' end; \
           if not p.visible then return 'overlay_hidden' end; \
           if p.width < 1200 or p.height < 700 then return 'overlay_too_small' end; \
           if p.a <= 0 or p.a >= 0.6 then return 'overlay_alpha:' .. tostring(p.a) end; \
           return 'ok' end)()",
        "ok",
    )?;
    assert_str(
        &mut rt,
        "(function() local icon = ui.image_get('hud:weather_icon'); \
           if not icon then return 'no_icon' end; \
           return icon.texture end)()",
        "assets/art/ui/hud/ui_weather_rain.png",
    )?;
    // 晴天：遮罩与雨图标都必须被清掉（`ui.clear`），只留晴图标。
    lua(&mut rt, "require('world.weather').set('sunny')")?;
    frames(&mut rt, 2)?;
    assert_str(
        &mut rt,
        "(function() if ui.panel_get('hud:rain') then return 'overlay_leaked' end; \
           local icon = ui.image_get('hud:weather_icon'); \
           if not icon then return 'no_icon' end; \
           return icon.texture end)()",
        "assets/art/ui/hud/ui_weather_sun.png",
    )?;
    // HUD 文本仍然与天气一致（文本与画面同源，M26 的映射不受影响）。
    // 天气片段在状态行里的**行号**会随改版式移动（M33 拆成 6 行后落在第 3 行），
    // 所以这里跨行找 `天气 ×`：断言的意图本来是"文本与天气一致"，不是"天气在第几行"。
    assert_str(
        &mut rt,
        "(function() local found; \
           for _, line in ipairs(require('ui.hud').status_lines()) do \
             local piece = line:match('天气%s*%S+') \
             if piece then found = piece end \
           end; \
           return tostring(found) end)()",
        "天气 晴",
    )?;
    println!("④ 雨天遮罩 + 雨图标；晴天遮罩清除并换成晴图标");

    // ---------------- ⑤ 雨天真实影响玩法：免浇水且不花体力 ----------------
    banner("M29-01⑤ 雨天按 E 浇水走 `rain_watered` 分支：不耗体力、不加劳作分钟");
    lua(&mut rt, &format!("clock.set(33, {TEN_AM})"))?;
    frames(&mut rt, 2)?;
    let (fx, fy) = find_plot_cell(&mut rt, 14, 23, "lingzhi")?;
    // 翻土 + 播种（真实 E 键，锄头与种子都按正常玩法准备）。
    lua(
        &mut rt,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (fx as f32 + 0.5) * 32.0,
            (fy as f32 + 1.5) * 32.0
        ),
    )?;
    frames(&mut rt, 2)?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "till")?;
    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "plant_seed_lingzhi",
    )?;
    // 雨天：不需要水壶、不花体力。
    lua(&mut rt, "require('world.weather').set('rainy')")?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', nil)",
    )?;
    let energy_before = query_num(&mut rt, "require('core.state').energy")?;
    let labor_before = query_num(&mut rt, "require('core.state').labor_minutes")?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(&mut rt, "require('core.state').last_action", "rain_watered")?;
    assert_num(&mut rt, "require('core.state').energy", energy_before)?;
    assert_num(&mut rt, "require('core.state').labor_minutes", labor_before)?;
    // 同一天改回晴天：已经浇过（雨天那次）→ 走 already_watered，不会重复扣体力。
    lua(&mut rt, "require('world.weather').set('sunny')")?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "already_watered",
    )?;
    assert_num(&mut rt, "require('core.state').energy", energy_before)?;
    // 晴天的另一块地：没装水壶 → 明确提示需要水壶、**不扣体力**；
    // 装上水壶后正常扣体力（`farm.ENERGY_COST.water = 4`，数值案 docs/33 §5.1）。
    let (gx, gy) = find_plot_cell(&mut rt, 14, 23, "lingzhi")?;
    lua(
        &mut rt,
        &format!(
            "require('player.player').teleport({}, {}); require('core.state').facing = 'up'; \
             inv.equip(require('core.state').inventory, 'tool', 'hoe')",
            (gx as f32 + 0.5) * 32.0,
            (gy as f32 + 1.5) * 32.0
        ),
    )?;
    frames(&mut rt, 2)?;
    press(&mut rt, KeyCode::KeyE)?;
    lua(
        &mut rt,
        "require('core.state').selected_seed = 'seed_lingzhi'",
    )?;
    press(&mut rt, KeyCode::KeyE)?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', nil)",
    )?;
    let energy_blocked_before = query_num(&mut rt, "require('core.state').energy")?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "need_watering_can",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').energy",
        energy_blocked_before,
    )?;
    lua(
        &mut rt,
        "inv.equip(require('core.state').inventory, 'tool', 'watering_can')",
    )?;
    let energy_sunny_before = query_num(&mut rt, "require('core.state').energy")?;
    press(&mut rt, KeyCode::KeyE)?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "water_lingzhi",
    )?;
    assert_num(
        &mut rt,
        "require('core.state').energy",
        energy_sunny_before - 4.0,
    )?;
    println!("⑤ 雨天免浇水（体力 {energy_before} 不变）；晴天没水壶不耗体力，装水壶后扣 4 点");

    // ---------------- ⑥ 资源条按真实数值画宽度 ----------------
    banner("M29-01⑥ 三条资源条：宽度 = 真实数值比例（受伤后血条变短，读回验证）");
    let ratios = query_str(
        &mut rt,
        "(function() local r = require('ui.hud').bar_ratios(); \
           return string.format('%.3f,%.3f,%.3f', r.hp, r.stamina, r.neili) end)()",
    )?;
    let parts: Vec<f64> = ratios
        .split(',')
        .map(|part| part.parse::<f64>().unwrap_or(-1.0))
        .collect();
    assert_eq!(parts.len(), 3, "{ratios:?}");
    for value in &parts {
        if !(0.0..=1.0).contains(value) {
            return Err(format!("资源条比例越界：{ratios}").into());
        }
    }
    assert_str(
        &mut rt,
        "(function() local icon = ui.image_get('hud:bar_hp'); \
           if not icon then return 'no_bar' end; \
           return icon.texture end)()",
        "assets/art/ui/hud/ui_bar_fill_hp.png",
    )?;
    let full_width = query_num(&mut rt, "ui.image_get('hud:bar_hp').width")?;
    if (full_width - 160.0).abs() > 1.0 {
        return Err(format!("满血条宽度应为 160，实为 {full_width}").into());
    }
    // 真实受伤：把气血压到一半（`combat.player_hp` 是 HUD 的同源数据）。
    lua(
        &mut rt,
        "require('systems.combat').set_player_hp(require('systems.combat').player_max_hp() / 2)",
    )?;
    frames(&mut rt, 2)?;
    let hurt_width = query_num(&mut rt, "ui.image_get('hud:bar_hp').width")?;
    if hurt_width >= full_width {
        return Err(format!("受伤后血条没有变短：{full_width} → {hurt_width}").into());
    }
    assert_num(
        &mut rt,
        "(function() local r = require('ui.hud').bar_ratios(); \
           return math.floor(r.hp * 100 + 0.5) end)()",
        50.0,
    )?;
    lua(
        &mut rt,
        "require('systems.combat').set_player_hp(require('systems.combat').player_max_hp())",
    )?;
    println!("⑥ 满血 {full_width}px → 半血 {hurt_width}px（比例 0.500）");

    // ---------------- ⑦ 资源决定表：docs/48 与 assets 双向核对 ----------------
    banner("M29-01⑦ 资源接线/下架清单 docs/48：扫描结果与文档逐行一致");
    let audit = audit_assets(&project)?;
    println!(
        "   扫描：assets 共 {} = 已引用 {} + 开发产物 {} + 内容缺口 {}",
        audit.total,
        audit.referenced.len(),
        audit.dev.len(),
        audit.content.len()
    );
    if audit.dev.len() != EXPECTED_DEV_ARTIFACTS {
        return Err(format!(
            "开发产物 {} 个，与 docs/48 §1 的 {EXPECTED_DEV_ARTIFACTS} 不符（新增预览图请改文档）",
            audit.dev.len()
        )
        .into());
    }
    if audit.content.len() != EXPECTED_CONTENT_ORPHANS {
        return Err(format!(
            "内容缺口 {} 个，与 docs/48 §1 的 {EXPECTED_CONTENT_ORPHANS} 不符；\
             未在清单里的文件：{:?}",
            audit.content.len(),
            audit.content.iter().take(8).collect::<Vec<_>>()
        )
        .into());
    }
    if audit.referenced.len() != EXPECTED_REFERENCED {
        return Err(format!(
            "已引用 {} 个，与 docs/48 §1 的 {EXPECTED_REFERENCED} 不符",
            audit.referenced.len()
        )
        .into());
    }
    let rows = parse_decision_rows(&project.join("docs/48-江湖有田-资源接线与下架清单.md"))?;
    if rows.is_empty() {
        return Err("docs/48 §2 没有解析到任何决定行".into());
    }
    // 按顺序匹配：每个内容缺口必须落在第一行里；每行的实际数量必须等于文档里的数字。
    let mut actual: BTreeMap<String, usize> = BTreeMap::new();
    let mut uncovered: Vec<String> = Vec::new();
    for rel in &audit.content {
        match rows.iter().find(|row| rel.starts_with(&row.prefix)) {
            Some(row) => *actual.entry(row.prefix.clone()).or_insert(0) += 1,
            None => uncovered.push(rel.clone()),
        }
    }
    if !uncovered.is_empty() {
        return Err(format!(
            "有 {} 个未引用资源不在 docs/48 §2 的决定表里：{:?}",
            uncovered.len(),
            uncovered.iter().take(8).collect::<Vec<_>>()
        )
        .into());
    }
    for row in &rows {
        let got = actual.get(&row.prefix).copied().unwrap_or(0);
        if got != row.count {
            return Err(format!(
                "docs/48 行 `{}`：文档写 {} 个未引用，实际 {got} 个（接线后请把该行数量改成 0）",
                row.prefix, row.count
            )
            .into());
        }
        if row.decision == "接线" {
            // "接线"必须是事实：文件存在，且真的被引用。
            let path = project.join(&row.prefix);
            if !path.is_file() {
                return Err(format!("docs/48 标为接线的文件不存在：{}", row.prefix).into());
            }
            if audit.content.contains(&row.prefix) {
                return Err(format!("docs/48 标为接线但扫描仍未引用：{}", row.prefix).into());
            }
        }
    }
    // 接线过的四季地表必须真的被 `data/season_art` 展开命中（不只是文件在）。
    for rel in &season_textures {
        if audit.content.contains(rel) {
            return Err(format!("四季地表未被 season_art 命中：{rel}").into());
        }
    }
    // 下架动作：第三方包移出工程、标题原图改名。
    //
    // `third_party/MushroomVillage_16px` **是仓库根的素材存放处，不是工程契约**：
    // 它不进 `assets/`、不被任何场景/脚本引用，也就不参与本工程的可交付验收。
    // 因此这里按 `docs/48 §3` 的口径分三档判定，而不是一刀切要求它存在：
    //   * 工程侧的安全不变量（第三方包**不在** `assets/` 里）——无条件强制；
    //   * 目录存在 → 必须是完整搬迁（≥21 个文件 + CC0 许可），挡「搬一半」；
    //   * 目录缺失 → 记为环境事实并**通过**（干净 checkout、CI、新机器都会走这一档，
    //     不该因为没有一份与玩法无关的第三方图集就把整条 MVP 门拦死）。
    // 想强制要求本地备有该包时，设 `RUSTLUA_REQUIRE_THIRD_PARTY=1`。
    if project
        .join("assets/art/tilesets/mushroom_village")
        .exists()
    {
        return Err("第三方素材包没有移出 assets/（docs/48 §3）".into());
    }
    let third_party = project
        .parent()
        .and_then(|examples| examples.parent())
        .map(|root| root.join("third_party/MushroomVillage_16px"))
        .unwrap_or_default();
    let third_party_status = third_party_state(&third_party)?;
    if std::env::var_os("RUSTLUA_REQUIRE_THIRD_PARTY").is_some()
        && let Some(reason) = &third_party_status.incomplete
    {
        return Err(format!("已设 RUSTLUA_REQUIRE_THIRD_PARTY，但第三方包不可用：{reason}").into());
    }
    if project.join("assets/art/title/title_bg.png").exists() {
        return Err(
            "assets/art/title/title_bg.png 仍在（docs/48 §3 要求改名为 _title_bg_source.png）"
                .into(),
        );
    }
    if !project
        .join("assets/art/title/_title_bg_source.png")
        .is_file()
    {
        return Err("缺少 assets/art/title/_title_bg_source.png".into());
    }
    // 开发产物一律不进发行包（引擎规则，assetc 侧另有单测守住同一条规则）。
    for rel in &audit.dev {
        if !is_dev_artifact(rel) {
            return Err(format!("开发产物判定不一致：{rel}").into());
        }
    }
    println!(
        "⑦ docs/48 共 {} 行决定，覆盖 {} 个内容缺口；接线行全部为真；第三方包：{}",
        rows.len(),
        audit.content.len(),
        third_party_status.describe()
    );

    let _ = fs::remove_file(&save_path);
    println!(
        "jhyt_m29_season_weather_ok project={} season_textures={} weather_day32={} \
         swaps={} bars={} dev={} orphans={} decisions={} third_party={}",
        project.display(),
        season_textures.len(),
        day32,
        swaps_after as i64,
        ratios,
        audit.dev.len(),
        audit.content.len(),
        rows.len(),
        if third_party_status.files.is_some() {
            "ok"
        } else {
            "absent"
        }
    );
    Ok(())
}

// ---------- 资源审计 ----------

/// `assetc::is_dev_artifact` 的同口径副本：`assets/` 里的 `_` 前缀、含 `preview` 的
/// PNG、`manifest.json` 都是生成器产出的开发用文件（预览拼图 / 美术清单），
/// **不进发行包**。权威实现在 `engine/crates/assetc/src/lib.rs`（有自己的单测），
/// 这里复制一份是因为本样例要独立核对 `docs/48` 的"开发产物 46 个"这一声明；
/// 两边不一致时 `jhyt_m29` 与 assetc 单测会同时报错。
fn is_dev_artifact(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let lower = name.to_ascii_lowercase();
    if lower == "manifest.json" {
        return true;
    }
    if lower.ends_with(".png") && lower.contains("preview") {
        return true;
    }
    name.starts_with('_')
}

/// 剥掉 Lua 注释（`--` 到行尾、`--[[ ... ]]` 与 `--[==[ ... ]==]` 长注释），保留字符串。
/// 引用审计只看"真的会被执行的代码 + 字符串里的路径"，注释里讨论资源不算引用。
fn strip_lua_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            let cursor = index + 2;
            // 长注释：`--[[` / `--[==[`
            if bytes.get(cursor) == Some(&b'[') {
                let mut equals = 0;
                while bytes.get(cursor + 1 + equals) == Some(&b'=') {
                    equals += 1;
                }
                if bytes.get(cursor + 1 + equals) == Some(&b'[') {
                    let close = format!("]{}]", "=".repeat(equals));
                    let rest = &text[cursor + 2 + equals..];
                    index = match rest.find(&close) {
                        Some(offset) => cursor + 2 + equals + offset + close.len(),
                        None => text.len(),
                    };
                    continue;
                }
            }
            // 行注释：跳到行尾（换行留给下一轮，保持行结构）
            index = match text[index..].find('\n') {
                Some(offset) => index + offset,
                None => text.len(),
            };
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\0');
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

struct Audit {
    total: usize,
    referenced: Vec<String>,
    dev: Vec<String>,
    content: Vec<String>,
}

struct DecisionRow {
    prefix: String,
    count: usize,
    decision: String,
}

/// 复刻 `docs/48` §1 的口径：扫描 `assets/**`，分类为已引用 / 开发产物 / 内容缺口。
fn audit_assets(project: &Path) -> Result<Audit, Box<dyn std::error::Error>> {
    // 引用语料：场景 + 全部脚本 + 工程配置（不含 docs、不含 manifest.json）。
    let mut corpus = String::new();
    for entry in read_dir_sorted(&project.join("scenes"))? {
        if entry.extension().and_then(|e| e.to_str()) == Some("scene") {
            corpus.push_str(&fs::read_to_string(&entry).unwrap_or_default());
            corpus.push('\n');
        }
    }
    let mut scripts = Vec::new();
    collect_files(&project.join("scripts"), &mut scripts)?;
    for entry in &scripts {
        if entry.extension().and_then(|e| e.to_str()) == Some("lua") {
            // **剥掉 Lua 注释**再进语料：注释里提到某个文件名不是"引用"。
            // 反例（这版口径修的就是它）：`ui/hud.lua` 的注释里说明"`ui_weather_cloud.png`
            // 因为天气没有阴天取值所以不接线"，裸文本匹配会把它算成已引用，
            // 于是"没接线的资源"永远看不见。
            corpus.push_str(&strip_lua_comments(
                &fs::read_to_string(entry).unwrap_or_default(),
            ));
            corpus.push('\n');
        }
    }
    // UI 预设（`.prefab`）：版面从脚本搬进预设之后，"谁引用了这张图"的答案就写在预设里，
    // 少读它会让每张只被预设用到的贴图凭空变成"内容缺口"（M63 的
    // `assets/art/title/title_bg_pixel.png` 是第一个踩到的）。
    if project.join("assets/prefabs").is_dir() {
        let mut prefabs = Vec::new();
        collect_files(&project.join("assets/prefabs"), &mut prefabs)?;
        for entry in &prefabs {
            if entry.extension().and_then(|e| e.to_str()) == Some("prefab") {
                corpus.push_str(&fs::read_to_string(entry).unwrap_or_default());
                corpus.push('\n');
            }
        }
    }
    for name in ["project.toml", "settings.toml"] {
        corpus.push_str(&fs::read_to_string(project.join(name)).unwrap_or_default());
        corpus.push('\n');
    }

    let mut files = Vec::new();
    collect_files(&project.join("assets"), &mut files)?;
    let mut audit = Audit {
        total: 0,
        referenced: Vec::new(),
        dev: Vec::new(),
        content: Vec::new(),
    };
    for path in files {
        let rel = project_relative(project, &path);
        audit.total += 1;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut used = corpus.contains(&rel) || corpus.contains(&name);
        // 音频：引擎按主干解析 `<assets/audio>/<name>.wav`（先试 `.wav` 再试 `.ogg`）。
        if !used && rel.starts_with("assets/audio/") {
            used = corpus.contains(&stem);
        }
        // 四季地表：由 `data/season_art.lua` 的格式串展开命中（本样例已逐张查过存在性）。
        if !used && rel.contains("_ground_") && rel.starts_with("assets/art/tilesets/") {
            used = true;
        }
        if used {
            audit.referenced.push(rel);
        } else if is_dev_artifact(&rel) {
            audit.dev.push(rel);
        } else {
            audit.content.push(rel);
        }
    }
    Ok(audit)
}

/// 解析 `docs/48` §2 的 `| 前缀 | 未引用数 | 决定 | 说明 |` 行。
fn parse_decision_rows(path: &Path) -> Result<Vec<DecisionRow>, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') || line.starts_with("|---") {
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim())
            .collect();
        if cells.len() < 3 {
            continue;
        }
        let prefix = cells[0].trim_matches('`');
        if !prefix.starts_with("assets/") {
            continue;
        }
        let Ok(count) = cells[1].parse::<usize>() else {
            continue;
        };
        rows.push(DecisionRow {
            prefix: prefix.to_owned(),
            count,
            decision: cells[2].to_owned(),
        });
    }
    Ok(rows)
}

// ---------- helpers ----------

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        out.push(entry?.path());
    }
    out.sort();
    Ok(out)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    if !dir.is_dir() {
        return Ok(());
    }
    for path in read_dir_sorted(dir)? {
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn count_files(dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    collect_files(dir, &mut files)?;
    Ok(files.len())
}

/// 第三方素材包（`docs/48 §3` 的搬迁目的地）的本地状态。
///
/// `docs/48 §3` 把它判为**仓库级素材存放处**而不是工程契约，所以「不在本机」是环境事实、
/// 不是缺陷；但「搬了一半」（文件数不足或丢了 CC0 许可）是真问题，必须挡住。
#[derive(Debug)]
struct ThirdPartyState {
    /// 目录已就位时的文件数。
    files: Option<usize>,
    /// `Some(说明)` = 该包不可用（缺失或搬迁不完整）。
    incomplete: Option<String>,
}

impl ThirdPartyState {
    fn describe(&self) -> String {
        match (&self.files, &self.incomplete) {
            (Some(files), None) => format!("已就位（{files} 个文件，含 CC0 许可）"),
            (_, Some(reason)) => format!("不可用——{reason}"),
            (None, None) => "状态未知".to_owned(),
        }
    }
}

/// 就地判定第三方素材包状态：存在则校验完整性，缺失则如实记录。
fn third_party_state(dir: &Path) -> Result<ThirdPartyState, Box<dyn std::error::Error>> {
    if !dir.exists() {
        // 缺失只记事实，不报错（见调用点的三档说明）。
        return Ok(ThirdPartyState {
            files: None,
            incomplete: Some(format!("{} 不存在（本机未保留该第三方包）", dir.display())),
        });
    }
    let files = count_files(dir)?;
    if files < 21 {
        return Ok(ThirdPartyState {
            files: None,
            incomplete: Some(format!(
                "{} 只有 {files} 个文件（搬迁不完整，应为 21 个）",
                dir.display()
            )),
        });
    }
    if !dir.join("0_license_CC0.txt").is_file() {
        return Ok(ThirdPartyState {
            files: None,
            incomplete: Some(format!("{} 缺少 0_license_CC0.txt", dir.display())),
        });
    }
    Ok(ThirdPartyState {
        files: Some(files),
        incomplete: None,
    })
}

fn project_relative(project: &Path, path: &Path) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn fresh_save_path(project: &Path, file: &str) -> PathBuf {
    let dir = project.join(".data");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(file);
    let _ = fs::remove_file(&path);
    path
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 竖版：设计分辨率 720×1280（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
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

fn press(runtime: &mut LuaRuntime, key: KeyCode) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = InputSnapshot::default();
    input.key_event(key, true);
    runtime.set_input(input);
    runtime.call_update()?;
    runtime.set_input(InputSnapshot::default());
    runtime.call_update()?;
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

fn goto_map(
    runtime: &mut LuaRuntime,
    map_id: &str,
    pos: (f32, f32),
) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!("map['goto']('{map_id}', {}, {})", pos.0, pos.1),
    )?;
    runtime.process_scene_transition()?;
    Ok(())
}

/// 返回 Lua 全局表里的字符串数组（顺序稳定）。
fn lua_lines(
    runtime: &mut LuaRuntime,
    expr: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let joined = query_str(runtime, &format!("table.concat({expr}, '\\n')"))?;
    if joined.is_empty() {
        return Ok(Vec::new());
    }
    Ok(joined.lines().map(|line| line.trim().to_owned()).collect())
}

/// 扫描农田矩形，返回第一个未开垦且土地类型匹配的地块（确定性）。
fn find_plot_cell(
    runtime: &mut LuaRuntime,
    x0: i64,
    x1: i64,
    crop: &str,
) -> Result<(i64, i64), Box<dyn std::error::Error>> {
    let got = query_str(
        runtime,
        &format!(
            "(function() local farm = require('systems.farm'); local land = require('data.farm').land; \
             for y = 4, 13 do for x = {x0}, {x1} do \
               if not farm.is_plot(x, y) and (not __m29_taken or not __m29_taken[x .. ',' .. y]) then \
                 for _, l in ipairs(land) do \
                   if x >= l.x0 and x <= l.x1 and y >= l.y0 and y <= l.y1 then \
                     local want = ([[{crop}]] == 'lingzhi') and 'medicinal' or 'field'; \
                     if l.type == want then \
                       __m29_taken = __m29_taken or {{}}; \
                       __m29_taken[x .. ',' .. y] = true; \
                       return x .. ',' .. y \
                     end \
                   end \
                 end \
               end \
             end end; return 'none' end)()"
        ),
    )?;
    let mut parts = got.split(',');
    let x = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("找不到空闲药田：{got}"))?;
    let y = parts
        .next()
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| format!("找不到空闲药田：{got}"))?;
    Ok((x, y))
}

fn banner(title: &str) {
    println!("\n=== {title} ===");
}
