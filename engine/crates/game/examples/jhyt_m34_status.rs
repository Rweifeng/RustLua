//! Headless acceptance drive for M34-03：中毒与瞬时状态机制
//! （`data/combat.lua` 的 `status` 表 + `systems/status.lua` 账本 + `systems/combat.lua` 结算）。
//!
//! 为什么要这一门：`data/enemies.lua` 里毒蝎的注释一直写着"带中毒"，而全工程没有任何地方
//! 结算它；HUD 的八张状态图标里 `poison` 那张自己也承认"已就绪但没人驱动"。这类**假事实**
//! 最贵的地方在于它读起来像已完成。所以本样例钉死四件事：
//!
//! 1. **文档 ↔ 数据**：解析 `docs/33-江湖有田-数值设计案.md` §10 状态效果表，把"每 2 秒损失
//!    3 生命 / 10 秒"按本仓唯一的计时单位（60Hz 固定步）换算后与 `data/combat.lua` 逐格比对；
//!    表里另外四行（坚定/轻功无敌/治疗/内息恢复）各自声明归属，新增一行不登记就门红。
//! 2. **真实一击**：山道上让毒蝎自己追上来咬一口（不是脚本直接 apply）——账本挂上、战斗日志
//!    的 hit 条目带 `status = "poison"`、HUD 的中毒图标真的被画出来。
//! 3. **精确到步**：第 120 步才掉第一口（本步挂上的状态从下一步才走）、600 步内正好 5 口共
//!    15 点、到期图标消失且不再掉血；DOT **不给**硬直/无敌帧/击退（中毒的人照样能走动）；
//!    再被咬只刷新持续时间、不重置跳表（docs/33 §10 那句"同类持续状态默认不可叠加，只刷新
//!    持续时间"的实现位置）。
//! 4. **毒可以毒死人，但走的是同一套战败流程**：致死那一跳之后 `systems/death` 的入口照常
//!    回庄；瞬时状态不入档（与硬直/无敌帧同一裁定）——换图不清、读档必清。
//!
//! 另有 10 条数据负例 + 2 条账本 API 边界 + 1 条"数据加了状态但 HUD 没配中文"的配对检查，
//! 全部要求**真的报出来**。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m34_status [-- <game_jhyt-root>]

use rustlua_input::input::InputSnapshot;
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 青云山道入口落点（与 jhyt_m34_festival 同一处）。
const MOUNTAIN_POS: (f32, f32) = (624.0, 80.0);
/// 本仓唯一的计时单位：`game_app.rs` 的 `FixedTimeStep::new(60, 8)`。
const HZ: f64 = 60.0;

/// 在毒蝎索敌圈内找一个"只有它够得着我"的站位并落过去（真实追击的前提）。
/// 判据：离毒蝎 60..145 像素（aggro 150 之内、又不重叠）、离山道其它敌人 > 320 像素
/// （它们的最大索敌是 260），且毒蝎 A* 能走到这一格。
const LURE_SCORPION: &str = "(function() \
    local g = entity.find('GroundMap'); \
    if not g then return 'no_ground' end; \
    local enemy_sys = require('systems.enemy'); \
    local sc = enemy_sys.query('enemy_scorpion'); \
    if not sc or not sc.alive or not sc.cell_x then return 'no_scorpion' end; \
    local others = {}; \
    for _, e in ipairs(require('data.enemies').list()) do \
      if e.map == 'mountain_path' and e.persist_id ~= 'enemy_scorpion' then \
        local q = enemy_sys.query(e.persist_id); \
        if q and q.alive and q.cell_x then others[#others + 1] = { x = q.cell_x, y = q.cell_y } end; \
      end; \
    end; \
    local tile, sx, sy = 32, sc.cell_x, sc.cell_y; \
    local dirs = { {0,-1}, {-1,0}, {0,1}, {1,0}, {-1,-1}, {1,-1}, {-1,1}, {1,1} }; \
    for r = 3, 4 do \
      for _, d in ipairs(dirs) do \
        local cx, cy = sx + d[1] * r, sy + d[2] * r; \
        if not tilemap.is_solid(g, cx, cy) then \
          local px, py = (cx + 0.5) * tile, (cy + 0.5) * tile; \
          local dist = math.sqrt((px - (sx + 0.5) * tile) ^ 2 + (py - (sy + 0.5) * tile) ^ 2); \
          local ok = dist >= 60 and dist <= 145; \
          for _, o in ipairs(others) do \
            local odx = px - (o.x + 0.5) * tile; \
            local ody = py - (o.y + 0.5) * tile; \
            if math.sqrt(odx * odx + ody * ody) < 320 then ok = false end; \
          end; \
          if ok and tilemap.find_path(g, sx, sy, cx, cy, false) then \
            require('player.player').teleport(px, py); \
            return 'ok ' .. cx .. ',' .. cy .. ' dist=' .. string.format('%.0f', dist) \
              .. ' scorpion=' .. sx .. ',' .. sy; \
          end; \
        end; \
      end; \
    end; \
    return 'no_spot' \
  end)()";

/// 中毒条目的剩余量：`steps_left/next_in/source`，未挂 → `none`。
const POISON_REMAIN: &str = "(function() \
    local e = require('systems.status').remaining('poison', 'player'); \
    if not e then return 'none' end; \
    return tostring(e.steps_left) .. '/' .. tostring(e.next_in) .. '/' .. tostring(e.source) \
  end)()";

/// 战斗日志里 DOT 结算的条数与累计扣血（比"气血差"更硬：混进别的掉血也不会算进来）。
const DOT_COUNT: &str = "(function() local n = 0; \
    for _, e in ipairs(require('systems.combat').log()) do \
      if e.event == 'dot' then n = n + 1 end end; return tostring(n) end)()";
const DOT_DAMAGE: &str = "(function() local d = 0; \
    for _, e in ipairs(require('systems.combat').log()) do \
      if e.event == 'dot' then d = d + (tonumber(e.damage) or 0) end end; return tostring(d) end)()";

/// 账本快照（种类@剩余步，按施加顺序）——图标排布与"不可叠加"都靠这个顺序确定性。
const STATUS_LIST: &str = "(function() local out = {}; \
    for _, e in ipairs(require('systems.status').list('player')) do \
      out[#out + 1] = e.kind .. '@' .. tostring(e.steps_left) end; \
    return table.concat(out, ',') end)()";

/// HUD 是否真的画了中毒图标（`ui.image_get` 读回几何与贴图名；没画 = nil）。
/// 几何字段是 float，`tostring(16.0)` 会露出小数点，取整后再比。
const POISON_ICON: &str = "(function() local img = ui.image_get('hud:status_poison'); \
    if not img then return 'nil' end; \
    return tostring(math.floor(img.width)) .. 'x' .. tostring(math.floor(img.height)) .. ':' .. \
      tostring(string.match(tostring(img.texture), '([^/]+)$')) end)()";

/// 战败回庄的落点判据：玩家实际坐标 vs `data/maps.lua` 里 manor 的出生点世界坐标。
/// 拿数据自身当基准，避免硬编码坐标随 spawn 改动而"假绿"。
const SPAWN_VS_PLAYER: &str = "(function() local m = require('data.maps'); \
    local sx, sy = m.to_world({ to_cell = m.get('manor').spawn }); \
    local p = require('player.player').position(); \
    local same = math.abs(p.x - sx) < 1 and math.abs(p.y - sy) < 1; \
    return (same and 'same' or 'diff') .. '/' .. math.floor(p.x) .. ',' .. math.floor(p.y) \
      .. '/want=' .. math.floor(sx) .. ',' .. math.floor(sy) end)()";

/// 扫遍 `data/combat.lua` 的每个状态种类，报出"feedback 键在 HUD 没有中文映射"的那些。
/// HUD 的兜底会把未知键包成「未知反馈（键）」，所以这里能区分"有映射"与"露英文键"。
const UNMAPPED_FEEDBACK: &str = "(function() \
    local hud = require('ui.hud'); \
    local data = require('data.combat'); \
    local out = {}; \
    for _, kind in ipairs(data.status_kinds()) do \
      local def = data.status_entry(kind); \
      local key = def and def.feedback; \
      if type(key) ~= 'string' or key == '' then \
        out[#out + 1] = kind .. ':no_feedback_key'; \
      else \
        local text = hud.feedback(key); \
        if text == key or text:find('未知反馈') then out[#out + 1] = kind .. ':' .. key end; \
      end; \
    end; \
    return table.concat(out, ',') \
  end)()";

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
    let design_doc = project.join("docs/33-江湖有田-数值设计案.md");
    let design = fs::read_to_string(&design_doc)
        .map_err(|error| format!("cannot read {}: {error}", design_doc.display()))?;
    let save_path = fresh_save_path(&project, "jhyt_m34_status.sav");
    let project_arg = project.clone();

    let mut rt = new_runtime(&project_arg, &save_path)?;
    enter_main_menu(&mut rt, &main_script)?;
    click_button(&mut rt, "BtnStart")?;
    assert_scene_is(&mut rt, "manor")?;

    // ---------------- ① 文档 ↔ 数据 ↔ 消费方 ----------------
    banner("M34-03① docs/33 §10 状态效果表 == data/combat.lua 的 status 表（按 60Hz 固定步换算）");
    let rows = doc_status_rows(&design)?;
    if rows.len() != 5 {
        return Err(format!(
            "§10 状态效果表读到 {} 行，期望 5 行（中毒/坚定/轻功无敌/治疗/内息恢复）",
            rows.len()
        )
        .into());
    }
    let mut owned = 0;
    for row in &rows {
        if row.name == "中毒" {
            let (interval, amount, duration) = poison_seconds(row)?;
            let got = query_str(
                &mut rt,
                "(function() local d = require('data.combat').status.poison; \
                   return tostring(d.steps) .. '/' .. tostring(d.tick_steps) .. '/' .. \
                     tostring(d.damage) end)()",
            )?;
            let want = format!("{:.0}/{:.0}/{:.0}", duration * HZ, interval * HZ, amount);
            if got != want {
                return Err(format!(
                    "§10「中毒」写的是「{}」（{}），换算成 60Hz 固定步应为 {want}（步/步/点），实际 {got}",
                    row.effect, row.duration
                )
                .into());
            }
            owned += 1;
            println!(
                "  中毒：「{}」（{}）→ status.poison = {got}（总步/每跑步/每跳点）",
                row.effect, row.duration
            );
        } else {
            // 其余四行今天不进状态账本，但必须**说得出归谁**：文档新增一行却没人认领，
            // 就是下一批"读起来像已完成"的假事实的源头。
            let owner = match row.name.as_str() {
                "坚定" => "守心诀未入库；受击减伤属 M34-04（招式扩充与防御属性结算）",
                "轻功无敌" => {
                    "追风步未入库；今天的 48 步无敌帧是**被动受击**（data/combat.lua:player.iframes），不是招式位移"
                }
                "治疗" => {
                    "即时恢复走物品 use（data/items.lua 的回复药），不是持续状态；注意它今天回的是体力不是生命（§10 该行的待办）"
                }
                "内息恢复" => "即时恢复走物品 use（data/items.lua 的内力丹），不是持续状态",
                other => {
                    return Err(format!(
                        "§10 状态效果表新增了「{other}」但本示例没有声明归属：请补归属或移出表格"
                    )
                    .into());
                }
            };
            println!(
                "  {}（{}，{}）：{owner}",
                row.name, row.effect, row.duration
            );
            owned += 1;
        }
    }
    if owned != rows.len() {
        return Err("§10 表里有行没落到归属".into());
    }
    // "同类持续状态默认不可叠加，只刷新持续时间"是文档里的一句话，第⑤节把它钉成行为断言。
    if !design.contains("同类持续状态默认不可叠加") {
        return Err(
            "docs/33 §10 那句\"不可叠加，只刷新持续时间\"被改写了，第⑤节的口径要重读".into(),
        );
    }
    assert_str(
        &mut rt,
        "require('data.combat').status_kinds()[1]",
        "poison",
    )?;
    assert_num(&mut rt, "#require('data.combat').status_kinds()", 1.0)?;
    assert_str(
        &mut rt,
        "require('data.enemies').get('enemy_scorpion').status",
        "poison",
    )?;
    assert_str(
        &mut rt,
        "require('systems.enemy').status_of('enemy_scorpion')",
        "poison",
    )?;
    println!(
        "  毒蝎 data.enemies.status = poison ← enemy.status_of 读出同一份（种类写在数据里，不在 if 里）"
    );
    // 对照：没声明的敌人与玩家侧都给 nil —— "只有毒蝎会下毒"因此是可断言的事实。
    for id in [
        "enemy_bandit",
        "enemy_xuanming",
        "enemy_wild_boar",
        "enemy_wolf",
        "enemy_ape_boss",
        "player",
        "不存在的敌人",
    ] {
        assert_str(
            &mut rt,
            &format!("require('systems.enemy').status_of('{id}')"),
            "nil",
        )?;
    }
    println!("  其余敌人/玩家/未知 id → status_of = nil（只有毒蝎带状态）");
    // HUD 中文映射：数据里每个 feedback 键都得有文案，否则英文键名直接甩给玩家。
    assert_str(&mut rt, UNMAPPED_FEEDBACK, "")?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback('poisoned')",
        "中毒了，气血在流失",
    )?;
    println!("  HUD 中文映射齐备：poisoned → 中毒了，气血在流失（每个 feedback 键都被扫过）");

    // 账本 API 边界：非法入参必须拒绝，同类第二次 apply 是 refreshed 而不是叠加。
    banner("M34-03①′ 账本 API：非法入参拒绝、同类不叠加、remove 幂等、feedback 写进行为行");
    assert_str(
        &mut rt,
        "tostring(require('systems.status').apply('plague', 'player', 'x')) .. '/' .. \
           tostring(select(2, require('systems.status').apply('plague', 'player', 'x')))",
        "false/unknown_kind",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('systems.status').apply('poison', '', 'x')) .. '/' .. \
           tostring(select(2, require('systems.status').apply('poison', '', 'x')))",
        "false/bad_actor",
    )?;
    assert_str(
        &mut rt,
        "tostring(require('systems.status').apply('poison', 'player', '验收'))",
        "true",
    )?;
    assert_str(&mut rt, STATUS_LIST, "poison@600")?;
    // 挂上状态时写给玩家的那一行（数据里的 feedback 键）：站着不动时才轮得到它。
    assert_str(&mut rt, "require('core.state').last_action", "poisoned")?;
    assert_str(
        &mut rt,
        "require('ui.hud').feedback_line()",
        "> 中毒了，气血在流失",
    )?;
    // 同类再挂一次：只刷新持续时间（条目数仍是 1）。
    assert_str(
        &mut rt,
        "tostring(select(2, require('systems.status').apply('poison', 'player', '验收')))",
        "refreshed",
    )?;
    assert_str(&mut rt, STATUS_LIST, "poison@600")?;
    assert_num(&mut rt, "#require('systems.status').list('player')", 1.0)?;
    assert_str(
        &mut rt,
        "require('systems.status').remove('poison', 'player')",
        "true",
    )?;
    assert_str(
        &mut rt,
        "require('systems.status').remove('poison', 'player')",
        "false",
    )?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    assert_str(&mut rt, "require('core.state').last_action", "poisoned")?;
    println!(
        "  apply('plague') → unknown_kind、apply(空 actor) → bad_actor、第二次 apply → refreshed（条目仍 1 条）"
    );

    // ---------------- ② 真实一击 ----------------
    banner("M34-03② 山道真实被毒蝎咬中：账本挂上 + 日志带状态 + HUD 图标真的画出来");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    frames(&mut rt, 2)?;
    let spot = query_str(&mut rt, LURE_SCORPION)?;
    if !spot.starts_with("ok") {
        return Err(format!("找不到\"只有毒蝎够得着\"的站位：{spot}").into());
    }
    println!("  站位：{spot}");
    // 先把行为行占住：下面要断言"挂毒时写了 poisoned"，不能靠上一节的残留。
    lua(
        &mut rt,
        "require('core.state').last_action = 'verify_sentinel'",
    )?;
    assert_str(
        &mut rt,
        "require('core.state').last_action",
        "verify_sentinel",
    )?;
    let hp_before = player_hp(&mut rt)?;
    let mut bit = None;
    for step in 1..=900i64 {
        frames(&mut rt, 1)?;
        if query_str(&mut rt, "require('systems.status').has('poison', 'player')")? == "true" {
            bit = Some(step);
            break;
        }
    }
    let bite_step = bit.ok_or("等满 900 步毒蝎也没咬到人（追击/站位需要重看）")?;
    let frame_at_bite = query_num(&mut rt, "require('systems.combat').frame()")?;
    assert_str(&mut rt, POISON_REMAIN, "600/120/毒蝎")?;
    assert_str(&mut rt, STATUS_LIST, "poison@600")?;
    let damage = query_num(
        &mut rt,
        "require('data.enemies').get('enemy_scorpion').damage",
    )?;
    // M34-04b：命中走 docs/33 §6.1 的平减公式，所以这一口吃下的是
    // `max(1, 毒蝎 damage - 玩家 defense)`，不是 damage 本身。defense 从属性模型现读
    // （玩家基础 1、今天没有装备/Buff），写死数字的话防具一落地这里就是假绿灯。
    let defense = query_num(&mut rt, "require('player.stats').query('player','defense')")?;
    let bite = (damage - defense).max(1.0);
    let hp_after_bite = player_hp(&mut rt)?;
    if (hp_before - hp_after_bite - bite).abs() > f64::EPSILON {
        return Err(format!(
            "这一口应吃下 {bite}（毒蝎 damage {damage} - 玩家 defense {defense}，兜 1 点）：{hp_before} → {hp_after_bite}"
        )
        .into());
    }
    println!(
        "  第 {bite_step} 步被咬（战斗步 {frame_at_bite:.0}）：气血 {hp_before:.0} → {hp_after_bite:.0}（{damage:.0} 点扣掉 {defense:.0} 点防御），账本 600/120/毒蝎"
    );
    // 命中日志带上种类 → "带中毒"从此是可回读的事实。
    let hit_log = query_str(
        &mut rt,
        "(function() local last; \
           for _, e in ipairs(require('systems.combat').log()) do last = e end; \
           return tostring(last.event) .. '/' .. tostring(last.attacker) .. '/' .. \
             tostring(last.skill) .. '/' .. tostring(last.status) end)()",
    )?;
    let want_hit = "hit/毒蝎/strike/poison".to_owned();
    if hit_log != want_hit {
        return Err(format!("最后一条战斗日志不是 {want_hit}，实际 {hit_log}").into());
    }
    // 图标：HUD 每帧重绘，账本里有中毒就画出来（几何 + 真实贴图名）。
    assert_str(&mut rt, POISON_ICON, "16x16:ui_status_poison.png")?;
    println!(
        "  战斗日志 {hit_log}；HUD 图标 hud:status_poison = 16x16 ui_status_poison.png（第一次被真实驱动）"
    );
    // 诚实的边界：命中当步 input.lua 的硬直分支会占掉行为行（受击僵直优先于一句提示），
    // 持续状态的主反馈因此是**图标**，文字只是"挂上的那一刻"补一句（见 ①′ 的合成路径）。
    assert_str(&mut rt, "require('core.state').last_action", "hurt")?;
    assert_str(&mut rt, "require('ui.hud').feedback('hurt')", "受击僵直")?;
    println!("  边界如实记录：命中当步行为行是 hurt/受击僵直（硬直优先），中毒的可见反馈是图标");

    // ---------------- ③ 精确到步 ----------------
    banner("M34-03③ 跑回山庄：第 120 步才掉第一口（本步挂上的状态从下一步才走）");
    let spawn = manor_spawn(&mut rt)?;
    goto_map(&mut rt, "manor", spawn)?;
    assert_str(&mut rt, POISON_REMAIN, "600/120/毒蝎")?;
    // 图标是每帧重绘的：换图会把上一张图的动态面板收掉，所以要看"换图后的下一帧"而不是当帧。
    frames(&mut rt, 1)?;
    assert_str(&mut rt, POISON_ICON, "16x16:ui_status_poison.png")?;
    assert_str(&mut rt, POISON_REMAIN, "599/119/毒蝎")?;
    println!("  换图不清账本：manor 上仍读到 600/120，下一帧图标照旧画出来（毒跟人走，不跟图走）");
    let dots_at_bite = dots(&mut rt)?;
    let damage_at_bite = dot_damage(&mut rt)?;
    frames(&mut rt, 118)?;
    assert_str(&mut rt, POISON_REMAIN, "481/1/毒蝎")?;
    assert_num(&mut rt, DOT_COUNT, dots_at_bite)?;
    assert_str(
        &mut rt,
        "require('systems.combat').player_hurt_frames()",
        "0",
    )?;
    println!("  119 步：remaining = 481/1、结算条数没变（一口都没跳）");
    frames(&mut rt, 1)?;
    assert_str(&mut rt, POISON_REMAIN, "480/120/毒蝎")?;
    assert_num(&mut rt, DOT_COUNT, dots_at_bite + 1.0)?;
    assert_num(&mut rt, DOT_DAMAGE, damage_at_bite + 3.0)?;
    let hp_after_first = player_hp(&mut rt)?;
    if (hp_after_bite - hp_after_first - 3.0).abs() > f64::EPSILON {
        return Err(format!("第一跳扣的不是 3 点：{hp_after_bite} → {hp_after_first}").into());
    }
    // DOT 不是"被打了一下"：不给硬直、不给无敌帧（人还能操作）。
    assert_str(
        &mut rt,
        "require('systems.combat').player_hurt_frames()",
        "0",
    )?;
    assert_str(&mut rt, "require('systems.combat').player_iframes()", "0")?;
    assert_str(
        &mut rt,
        "require('systems.combat').player_can_act()",
        "true",
    )?;
    println!(
        "  第 120 步：480/120、正好一跳 3 点（气血 {hp_after_bite:.0} → {hp_after_first:.0}）；硬直 0 / 无敌帧 0 / 仍可操作"
    );

    // ---------------- ④ 生命周期 ----------------
    banner("M34-03④ 走完 600 步：正好 5 跳、共 15 点，到期图标消失、账本清空");
    let hp_before_cycle = player_hp(&mut rt)?;
    // 跳数基线取"咬中那一刻"（③里的第一跳也算进来），所以整段正好 5 跳。
    let mut seen = 0.0;
    for &target in &[120i64, 240, 360, 480, 600] {
        while remaining_steps(&mut rt)? > 600.0 - target as f64 {
            frames(&mut rt, 1)?;
        }
        seen += 1.0;
        let dots_now = dots(&mut rt)? - dots_at_bite;
        if dots_now != seen {
            return Err(format!(
                "第 {target} 步累计 {dots_now:.0} 跳，期望 {seen:.0} 跳（每 120 步正好一跳）"
            )
            .into());
        }
        assert_str(&mut rt, "require('systems.combat').player_iframes()", "0")?;
        println!("  第 {target} 步：累计 {dots_now:.0} 跳");
    }
    assert_str(&mut rt, POISON_REMAIN, "none")?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    let dots_total = dots(&mut rt)? - dots_at_bite;
    let damage_total = dot_damage(&mut rt)? - damage_at_bite;
    let hp_after_cycle = player_hp(&mut rt)?;
    if (hp_before_cycle - hp_after_cycle - 12.0).abs() > f64::EPSILON {
        return Err(format!(
            "剩余 4 跳（第 120 步那一口在③里已扣）共扣了 {:.0} 点，期望 12",
            hp_before_cycle - hp_after_cycle
        )
        .into());
    }
    if (damage_total - 15.0).abs() > f64::EPSILON {
        return Err(format!("日志累计 DOT 扣血 {damage_total}，期望 15").into());
    }
    if (dots_total - 5.0).abs() > f64::EPSILON {
        return Err(format!("整段中毒共 {dots_total:.0} 跳，期望 5 跳（600/120）").into());
    }
    println!(
        "  600 步 = {dots_total:.0} 跳 = 15 点（日志累计 {damage_total:.0}，气血实测同幅下降）；到期账本空"
    );
    assert_str(&mut rt, POISON_ICON, "nil")?;
    println!(
        "  到期那一帧之后图标被收掉（hud:status_poison 读回 nil，不再\"毒早过了图标还亮着\"）"
    );
    let dots_at_expiry = dots(&mut rt)?;
    frames(&mut rt, 120)?;
    assert_num(&mut rt, DOT_COUNT, dots_at_expiry)?;
    assert_str(&mut rt, POISON_REMAIN, "none")?;
    println!("  再走 120 步：一口也不掉（到期就是到期，没有\"残留每秒扣血\"）");

    // ---------------- ⑤ 不可叠加：只刷新持续时间 ----------------
    banner("M34-03⑤ 再被咬一口：持续时间刷回 600，但下一次结算的倒计时**不被重置**");
    goto_map(&mut rt, "mountain_path", MOUNTAIN_POS)?;
    frames(&mut rt, 2)?;
    let spot2 = query_str(&mut rt, LURE_SCORPION)?;
    if !spot2.starts_with("ok") {
        return Err(format!("第二次找不到站位：{spot2}").into());
    }
    wait_until_bitten(&mut rt, 900)?;
    assert_str(&mut rt, POISON_REMAIN, "600/120/毒蝎")?;
    let dots_before = dots(&mut rt)?;
    // 逐帧盯 steps_left：它只会往下走，唯一能回到 600 的时机就是"又挂了一次"。
    let mut refreshed_at = None;
    let mut next_at_refresh = 0.0;
    for step in 1..=200i64 {
        let before = remain(&mut rt)?;
        let dots_before_step = dots(&mut rt)?;
        frames(&mut rt, 1)?;
        let after = remain(&mut rt)?;
        if after.steps_left == 600.0 && before.steps_left < 600.0 {
            refreshed_at = Some(step);
            // 结算倒计时在这一帧只被"走了一步"改变（apply 不碰它）。
            next_at_refresh = after.next_in;
            assert_num(&mut rt, DOT_COUNT, dots_before)?;
            println!(
                "  第 {step} 步再次被咬：steps_left {} → 600（只刷新持续时间），next_in {} → {}（跳表照常走了一步）",
                before.steps_left, before.next_in, after.next_in
            );
            break;
        }
        // 常规一步：两个计数各减 1；唯一的例外是跳表归零的那一步（结算后重置为 120）。
        let settled = before.next_in == 1.0;
        let want_next = if settled { 120.0 } else { before.next_in - 1.0 };
        let want_dots = dots_before_step + if settled { 1.0 } else { 0.0 };
        if after.next_in != want_next || after.steps_left != before.steps_left - 1.0 {
            return Err(format!(
                "第 {step} 步账本反常：steps_left {} → {}、next_in {} → {}（期望 {} → {}）",
                before.steps_left,
                after.steps_left,
                before.next_in,
                after.next_in,
                before.next_in,
                want_next
            )
            .into());
        }
        assert_num(&mut rt, DOT_COUNT, want_dots)?;
    }
    let refreshed_at =
        refreshed_at.ok_or("等满 200 步没有被咬第二口（冷却 90 步，站位需要重看）")?;
    assert_num(&mut rt, "#require('systems.status').list('player')", 1.0)?;
    // 刷新不提前掉血，也不额外多掉一口：下一次结算仍按刷新后读到的 next_in 到来。
    frames(&mut rt, (next_at_refresh - 1.0) as usize)?;
    assert_num(&mut rt, DOT_COUNT, dots_before)?;
    frames(&mut rt, 1)?;
    assert_num(&mut rt, DOT_COUNT, dots_before + 1.0)?;
    assert_str(
        &mut rt,
        POISON_REMAIN,
        &format!("{}/120/毒蝎", 600.0 - next_at_refresh),
    )?;
    println!(
        "  刷新发生在第 {refreshed_at} 步；之后隔 {next_at_refresh:.0} 步才掉第一口（没有\"再咬一次就重置跳表\"）"
    );

    // ---------------- ⑥ 瞬时状态不入档 ----------------
    banner("M34-03⑥ 带着毒存一次档、再读回来：账本清空，气血照旧（与硬直/无敌帧同一裁定）");
    let hp_before_save = player_hp(&mut rt)?;
    let steps_before_save = remaining_steps(&mut rt)?;
    if steps_before_save <= 0.0 {
        return Err("存档前账本已经走完了（⑤之后剩余步数不足，带毒存档的前提不成立）".into());
    }
    assert_str(
        &mut rt,
        STATUS_LIST,
        &format!("poison@{steps_before_save:.0}"),
    )?;
    assert_str(
        &mut rt,
        "require('systems.status').has('poison', 'player')",
        "true",
    )?;
    assert_str(&mut rt, "require('core.save_manager').save()", "true")?;
    let provider_names = query_str(
        &mut rt,
        "table.concat(require('core.save_manager').provider_names(), ',')",
    )?;
    if provider_names.contains("status") {
        return Err(format!("存档 provider 名单里出现了 status：{provider_names}").into());
    }
    assert_str(&mut rt, "require('core.save_manager').load()", "true")?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    assert_str(
        &mut rt,
        "require('systems.status').has('poison', 'player')",
        "false",
    )?;
    assert_num(
        &mut rt,
        "require('systems.combat').player_hp()",
        hp_before_save,
    )?;
    let save_text = fs::read_to_string(&save_path).unwrap_or_default();
    for needle in ["steps_left", "next_in", "tick_steps", "last_status"] {
        if save_text.contains(needle) {
            return Err(format!(
                "存档正文里出现了 {needle:?}——瞬时状态不该入档（读档后站在原地继续掉血是凭空惩罚）"
            )
            .into());
        }
    }
    println!(
        "  同一次运行内 save→load：账本清空、气血 {:.0} 原样回来；provider 名单 = {provider_names}",
        hp_before_save
    );
    println!("  存档正文里没有 steps_left/next_in/tick_steps 任何账本字段");

    // ---------------- ⑦ 毒可以毒死人，但只有一条死亡流程 ----------------
    banner("M34-03⑦ 低气血时被咬：致死那一跳走的是同一套战败流程（DOT 致死 → systems/death 回庄）");
    // 气血刚好留"一跳 DOT"的量：被咬扣 `max(1, 毒蝎伤害 - 防御)` 后剩 DOT 单跳的伤害，
    // 于是"不死 → 被 DOT 跳死"这条链在数值动了之后仍然只由 DOT 收口。
    // 以前这里写死 5 点（咬掉 2 剩 3 = 一跳），M34-04b 防御进公式后 5 点会剩 4 点，
    // 一跳毒死不了人——所以两个数都从数据现读，不再靠"5"这个巧合。
    let dot = query_num(
        &mut rt,
        "require('data.combat').status_entry('poison').damage",
    )?;
    let hp_at_bite = bite + dot;
    lua(
        &mut rt,
        &format!("require('systems.combat').set_player_hp({hp_at_bite})"),
    )?;
    assert_str(
        &mut rt,
        "require('systems.status').has('poison', 'player')",
        "false",
    )?;
    wait_until_bitten(&mut rt, 900)?;
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "false",
    )?;
    assert_str(
        &mut rt,
        "require('systems.combat').player_hp()",
        &format!("{:.0}", dot),
    )?;
    let mut died_by_dot = false;
    for _ in 1..=140i64 {
        frames(&mut rt, 1)?;
        if query_str(&mut rt, "require('systems.combat').is_player_dead()")? == "true" {
            died_by_dot = true;
            break;
        }
    }
    if !died_by_dot {
        return Err("中毒的低气血玩家 140 步内没被 DOT 跳死（致死路径没接上？）".into());
    }
    let death_log = query_str(
        &mut rt,
        "(function() local last; \
           for _, e in ipairs(require('systems.combat').log()) do last = e end; \
           return tostring(last.event) .. '/' .. tostring(last.attacker) .. '/' .. \
             tostring(last.skill) .. '/' .. tostring(last.remaining) end)()",
    )?;
    if death_log != "death/毒蝎/poison/0" {
        return Err(format!(
            "最后一条日志不是 death/毒蝎/poison/0：{death_log}（DOT 致死必须记在 poison 上，否则玩家不知道死于何物）"
        )
        .into());
    }
    assert_str(&mut rt, STATUS_LIST, "")?;
    println!("  战败那一跳的日志 = {death_log}；死人身上不留账本（status.clear）");
    // 回庄是 `map.goto` 调度的切图：控制台里要自己把过渡跑完，再按"当前图 + 落点坐标"
    // 断言，而不是看一个标志位（scene.reload() 在这里会读回战败前那张图，说不了话）。
    assert_str(
        &mut rt,
        "tostring(select(1, require('systems.death').apply()))",
        "true",
    )?;
    rt.process_scene_transition()?;
    assert_str(&mut rt, "require('world.map_manager').current()", "manor")?;
    // 落点与"山庄注册表里的出生点"逐格比对（拿数据自身做基准，硬编码坐标会随 spawn 漂移而假绿）。
    let landing = query_str(&mut rt, SPAWN_VS_PLAYER)?;
    if !landing.starts_with("same") {
        return Err(format!(
            "战败后没有落在山庄出生点：{landing}（「回庄」若只是文案，这条会直接暴露）"
        )
        .into());
    }
    assert_str(&mut rt, "require('systems.combat').player_hp()", "100")?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "false",
    )?;
    let penalty_text = query_str(
        &mut rt,
        "tostring(require('systems.death').last_penalty().text)",
    )?;
    println!(
        "  回庄落点 = {landing}（与 data/maps 的 manor.spawn 同一格）；惩罚明细「{penalty_text}」；气血回满、毒也一起清了（同一条流程，没有第二套）"
    );
    // 调试置血（`set_player_hp(0)`，headless 验收常用那条）也必须走同一条收口：
    // 它是本批之前唯一的第二条死亡实现，现在只是收口的另一个调用方。
    assert_str(
        &mut rt,
        "tostring(require('systems.status').apply('poison', 'player', '验收'))",
        "true",
    )?;
    assert_str(&mut rt, STATUS_LIST, "poison@600")?;
    assert_str(&mut rt, "require('systems.combat').set_player_hp(0)", "0")?;
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "true",
    )?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    let debug_death_log = query_str(
        &mut rt,
        "(function() local last; \
           for _, e in ipairs(require('systems.combat').log()) do last = e end; \
           return tostring(last.event) .. '/' .. tostring(last.attacker) .. '/' .. \
             tostring(last.skill) end)()",
    )?;
    if debug_death_log != "death/debug/-" {
        return Err(format!(
            "调试置血的死亡日志不是 death/debug/-：{debug_death_log}（收口没被复用上）"
        )
        .into());
    }
    lua(&mut rt, "require('systems.combat').respawn()")?;
    assert_str(&mut rt, "require('systems.combat').player_hp()", "100")?;
    assert_str(
        &mut rt,
        "require('systems.combat').is_player_dead()",
        "false",
    )?;
    println!(
        "  调试置血 set_player_hp(0) 也是同一条收口：日志 {debug_death_log}、死人身上不留账本"
    );

    // ---------------- ⑧ 计时单位的确定性 ----------------
    banner(
        "M34-03⑧ 只有固定步：分片推进、快挂时钟、30/120Hz 的 delta 都是同样 5 跳；源码扫一遍不碰真实时间",
    );
    // 合成一次挂毒（账本机制本身的可重复性，不需要再爬一次山）。
    lua(&mut rt, "require('systems.status').clear('player')")?;
    assert_str(
        &mut rt,
        "tostring(require('systems.status').apply('poison', 'player', '验收'))",
        "true",
    )?;
    // 把游戏内时钟调到最快（clock.set_rate）：状态账本不吃它，因此跳数不变。
    lua(&mut rt, "clock.set_rate(8)")?;
    let dots_before_fast = dots(&mut rt)?;
    for _ in 0..4 {
        frames(&mut rt, 150)?;
        // 分片之间读一次账本与控制台：读不能改变推进（查询无副作用）。
        let _ = remain(&mut rt)?;
        let _ = query_str(&mut rt, "clock.now().day")?;
    }
    let dots_fast = dots(&mut rt)? - dots_before_fast;
    if (dots_fast - 5.0).abs() > f64::EPSILON {
        return Err(format!(
            "600 步分四片推进得到 {dots_fast:.0} 跳，期望 5 跳（与第④节的整段推进不同）"
        )
        .into());
    }
    lua(&mut rt, "clock.set_rate(2)")?;
    assert_str(&mut rt, STATUS_LIST, "")?;
    println!("  同样 600 步：4×150 分片 + 片间查询 + 游戏内时钟 8 倍速 → 仍是 5 跳 15 点");
    // 帧率的另一半：把每步喂给 Lua 的 `time.delta()` 拨成 30Hz / 120Hz 的数值再跑一遍。
    // 若结算写成 `damage * time.delta()`，这两轮会分别是 2 倍与 1/2 的点数。
    // 宿主侧真实的"30 FPS"是一帧跑两个固定步（`game_app.rs` 的 `FixedTimeStep::new(60, 8)`），
    // 那是步数不变的结构面；这里能证的是"Lua 侧没人乘 delta"。
    for (label, delta, delta_ms, want) in [
        ("30Hz", 1.0f32 / 30.0, 33.0f64, 5.0f64),
        ("120Hz", 1.0f32 / 120.0, 8.0, 5.0),
    ] {
        rt.set_delta(delta);
        lua(&mut rt, "require('systems.status').clear('player')")?;
        assert_str(
            &mut rt,
            "tostring(require('systems.status').apply('poison', 'player', '验收'))",
            "true",
        )?;
        let dots_at_delta = dots(&mut rt)?;
        let readback = query_str(&mut rt, "tostring(math.floor(time.delta() * 1000))")?;
        if readback != format!("{delta_ms:.0}") {
            return Err(format!(
                "{label} 这轮 `time.delta()` 读回 {readback}ms，注入没生效（断言就成了空转）"
            )
            .into());
        }
        frames(&mut rt, 600)?;
        let got = dots(&mut rt)? - dots_at_delta;
        if (got - want).abs() > f64::EPSILON {
            rt.set_delta(1.0 / 60.0);
            return Err(format!(
                "每步喂 {label} 的 delta 跑 600 步得到 {got:.0} 跳，期望 {want:.0} 跳（结算乘了 delta）"
            )
            .into());
        }
        assert_str(&mut rt, STATUS_LIST, "")?;
        println!(
            "  每步喂 time.delta() = {readback}ms（{label}）跑 600 步：仍是 {got:.0} 跳，账本正常到期"
        );
    }
    rt.set_delta(1.0 / 60.0);
    let (status_hits, combat_hits) = scan_wall_clock(&project)?;
    if status_hits != 0 || combat_hits != 0 {
        return Err(format!(
            "计时口径破了：systems/status.lua 命中 {status_hits} 处、systems/combat.lua 命中 {combat_hits} 处真实时间/随机源"
        )
        .into());
    }
    println!(
        "  源码扫描（去掉注释后）：status.lua 与 combat.lua 里 time.delta / clock.now / math.random 共 0 处"
    );

    // ---------------- ⑨ 负例 ----------------
    banner("M34-03⑨ 十条数据负例：写错必须在校验期报出，而不是静默变成\"没有中毒\"");
    for (label, chunk, needle) in NEGATIVE_CASES {
        let got = query_str(&mut rt, chunk)?;
        if !got.contains(needle) {
            return Err(
                format!("负例「{label}」未被报出，期望包含 {needle:?}，得到:\n{got}").into(),
            );
        }
        println!("  {label} → 报出「{needle}」");
    }
    // 配对检查：数据里新加一个 feedback 键、HUD 没配中文 → 露出兜底文案（不是英文键名）。
    let fallback = query_str(&mut rt, "return require('ui.hud').feedback('poisoned_two')")?;
    if !fallback.contains("未知反馈") {
        return Err(format!("未配中文的 feedback 键没有兜底文案，直接露出键名：{fallback}").into());
    }
    println!("  兜底如实：未配映射的键 → 「未知反馈（poisoned_two）」（宁可兜底也不露英文键名）");

    // ---------------- ⑩ 全量校验 ----------------
    banner("M34-03⑩ 改坏过那么多遍数据，最后整份内容校验必须是干净的");
    assert_str(
        &mut rt,
        "require('validate_data').check({ strict = true })",
        "",
    )?;
    let registered = query_str(
        &mut rt,
        "(function() local out = {}; \
           for _, e in ipairs(require('validate_data').registered()) do \
             out[#out + 1] = e.name end; \
           return table.concat(out, ',') end)()",
    )?;
    if !registered.contains("combat") {
        return Err(format!(
            "校验清单里没有 combat 一项（新加的状态数据不在校验面上）：{registered}"
        )
        .into());
    }
    println!(
        "  校验器名单（{} 项）：{registered}",
        registered.split(',').count()
    );

    println!(
        "jhyt_m34_status_ok project={} source=scorpion_real_hit first_dot_step=120 ticks=5 dot_damage=15 \
         refresh=duration_only dot_death=manor save_status_fields=0 wall_clock_reads=0 negatives=10",
        project.display()
    );
    let _ = fs::remove_file(&save_path);
    Ok(())
}

// ---------- §10 状态效果表解析 ----------

/// §10 的一行（原样保留四列；数字只在需要比对时解析，因为"招式持续期间/即时"根本没有秒数）。
struct StatusRow {
    name: String,
    effect: String,
    duration: String,
}

/// 取 `## 10. 状态效果` 小节里的 Markdown 表格行。
///
/// 为什么在 Rust 侧解析而不是写死数字：docs/33 是**唯一**数值口径（与 §6.3 受击反馈表同一
/// 手法，见 jhyt_m25_combat_feedback）。策划改了表，本门必须立刻红，而不是继续绿着跑旧数字。
fn doc_status_rows(doc: &str) -> Result<Vec<StatusRow>, Box<dyn std::error::Error>> {
    let start = doc
        .find("## 10. 状态效果")
        .ok_or("docs/33 里找不到 §10 状态效果小节")?;
    let mut rows = Vec::new();
    let mut seen_separator = false;
    for line in doc[start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if seen_separator {
                break;
            }
            continue;
        }
        if !trimmed.starts_with('|') {
            break;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_owned())
            .collect();
        if cells.len() < 4 {
            return Err(format!("§10 表行格式不符: {trimmed}").into());
        }
        if cells[0] == "状态" {
            continue;
        }
        if cells[0].chars().all(|c| c == '-' || c == ':') {
            seen_separator = true;
            continue;
        }
        rows.push(StatusRow {
            name: cells[0].clone(),
            effect: cells[2].clone(),
            duration: cells[3].clone(),
        });
    }
    if !seen_separator || rows.is_empty() {
        return Err("§10 状态效果表没解析出数据行".into());
    }
    Ok(rows)
}

/// "每 2 秒损失 3 生命" + "10 秒" → (周期秒 2.0, 每次点数 3.0, 总秒 10.0)。
/// 解析失败一律是错误：本门的比对靠"数字真从表里读出来"，不是靠注释里抄一份。
fn poison_seconds(row: &StatusRow) -> Result<(f64, f64, f64), Box<dyn std::error::Error>> {
    let interval = after_digit(&row.effect, '每')
        .ok_or(format!("§10 中毒行的周期读不出来: {}", row.effect))?;
    let amount = last_digit_before(&row.effect, '生')
        .ok_or(format!("§10 中毒行的每次点数读不出来: {}", row.effect))?;
    let duration = first_digits(&row.duration)
        .ok_or(format!("§10 中毒行的总时长读不出来: {}", row.duration))?;
    Ok((interval, amount, duration))
}

/// 锚点之后的第一段数字（"每 2 秒…" → 2.0）。
fn after_digit(text: &str, anchor: char) -> Option<f64> {
    let (_, rest) = text.split_once(anchor)?;
    first_digits(rest)
}

/// 锚点之前最后一段数字（"每 2 秒损失 3 生命" → 3.0）。
fn last_digit_before(text: &str, anchor: char) -> Option<f64> {
    let (head, _) = text.split_once(anchor)?;
    let mut found = None;
    let mut run = String::new();
    for ch in head.chars() {
        if ch.is_ascii_digit() {
            run.push(ch);
        } else if !run.is_empty() {
            found = run.parse::<f64>().ok();
            run.clear();
        }
    }
    if !run.is_empty() {
        found = run.parse::<f64>().ok();
    }
    found
}

/// 文本里第一段连续数字。
fn first_digits(text: &str) -> Option<f64> {
    let mut run = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            run.push(ch);
        } else if !run.is_empty() {
            break;
        }
    }
    if run.is_empty() {
        return None;
    }
    run.parse::<f64>().ok()
}

// ---------- 负例（每条先改坏数据、跑校验或扫描、再改回来） ----------

/// `(标题, Lua 代码块, 期望出现在报错文案里的片段)`。
const NEGATIVE_CASES: [(&str, &str, &str); 10] = [
    (
        "敌人引用未知状态种类",
        "local e = require('data.enemies').get('enemy_wolf'); local saved = e.status; e.status = 'plague'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, '\\n'); \
           e.status = saved; return issues",
        "unknown status kind",
    ),
    (
        "敌人状态键写成空串",
        "local e = require('data.enemies').get('enemy_wolf'); local saved = e.status; e.status = ''; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, '\\n'); \
           e.status = saved; return issues",
        "unknown status kind",
    ),
    (
        "状态条目写了没消费方的键",
        "local d = require('data.combat').status.poison; d.slow = 2; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.slow = nil; return issues",
        "unknown effect key",
    ),
    (
        "跳长超过总时长（一口都跳不到）",
        "local d = require('data.combat').status.poison; local s = d.tick_steps; d.tick_steps = 700; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.tick_steps = s; return issues",
        "一次都跳不到",
    ),
    (
        "0 伤害的假状态",
        "local d = require('data.combat').status.poison; local s = d.damage; d.damage = 0; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.damage = s; return issues",
        "expected >= 1",
    ),
    (
        "步长写成小数",
        "local d = require('data.combat').status.poison; local s = d.steps; d.steps = 600.5; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.steps = s; return issues",
        "expected non-negative integer",
    ),
    (
        "状态表整个空掉",
        "local c = require('data.combat'); local s = c.status; c.status = {}; \
           local issues = table.concat((select(2, c.validate_all())) or {}, '\\n'); \
           c.status = s; return issues",
        "non-empty table",
    ),
    (
        "feedback 键不是字符串",
        "local d = require('data.combat').status.poison; local s = d.feedback; d.feedback = 7; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.feedback = s; return issues",
        "expected non-empty string",
    ),
    (
        "feedback 键缺失（挂了毒不给一句话）",
        "local d = require('data.combat').status.poison; local s = d.feedback; d.feedback = nil; \
           local issues = table.concat((select(2, require('data.combat').validate_all())) or {}, '\\n'); \
           d.feedback = s; return issues",
        "required non-empty string",
    ),
    (
        "feedback 键在 HUD 没有中文映射",
        "local d = require('data.combat').status.poison; local s = d.feedback; d.feedback = 'poisoned_two'; \
           local out = (function() \
             local hud, data, res = require('ui.hud'), require('data.combat'), {}; \
             for _, kind in ipairs(data.status_kinds()) do \
               local key = data.status_entry(kind).feedback; \
               local text = hud.feedback(key); \
               if text == key or text:find('未知反馈') then res[#res + 1] = kind .. ':' .. key end; \
             end; \
             return table.concat(res, ',') end)(); \
           d.feedback = s; return out",
        "poison:poisoned_two",
    ),
];

// ---------- 辅助 ----------

/// 账本读数（steps_left / next_in）。
struct Remain {
    steps_left: f64,
    next_in: f64,
}

fn remain(runtime: &mut LuaRuntime) -> Result<Remain, Box<dyn std::error::Error>> {
    let raw = query_str(runtime, POISON_REMAIN)?;
    if raw == "none" {
        return Ok(Remain {
            steps_left: -1.0,
            next_in: -1.0,
        });
    }
    let mut parts = raw.split('/');
    let steps = parts
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or(format!("remaining 读不出步数: {raw}"))?;
    let next = parts
        .next()
        .and_then(|v| v.parse::<f64>().ok())
        .ok_or(format!("remaining 读不出倒计时: {raw}"))?;
    Ok(Remain {
        steps_left: steps,
        next_in: next,
    })
}

fn remaining_steps(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    Ok(remain(runtime)?.steps_left)
}

fn dots(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(runtime, DOT_COUNT)
}

fn dot_damage(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(runtime, DOT_DAMAGE)
}

/// `data/maps.lua` 里山庄出生点的世界坐标，**现算**而非写死：写死的坐标会随 spawn 改动
/// 变成"落在别的格子上但断言仍绿"（⑦ 章的"回庄"就靠这份读数当基准）。
fn manor_spawn(runtime: &mut LuaRuntime) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    let raw = query_str(
        runtime,
        "(function() local m = require('data.maps'); \
           local x, y = m.to_world({ to_cell = m.get('manor').spawn }); \
           return tostring(x) .. ',' .. tostring(y) end)()",
    )?;
    let mut parts = raw.split(',');
    let mut read = |label: &str| -> Result<f32, Box<dyn std::error::Error>> {
        parts
            .next()
            .and_then(|v| v.parse::<f32>().ok())
            .ok_or_else(|| format!("读不到 manor 出生点的{label}：{raw}").into())
    };
    let x = read("横坐标")?;
    let y = read("纵坐标")?;
    Ok((x, y))
}

fn player_hp(runtime: &mut LuaRuntime) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(runtime, "require('systems.combat').player_hp()")
}

/// 站着不动等毒蝎追上来咬一口（返回用了多少步；咬中判据是账本，不是血量）。
fn wait_until_bitten(
    runtime: &mut LuaRuntime,
    limit: i64,
) -> Result<i64, Box<dyn std::error::Error>> {
    for step in 1..=limit {
        frames(runtime, 1)?;
        if query_str(runtime, "require('systems.status').has('poison', 'player')")? == "true" {
            return Ok(step);
        }
    }
    Err(format!("等满 {limit} 步也没被毒蝎咬中（账本仍是空的）").into())
}

/// 计时口径的源码扫描：去掉注释行与行内注释后，`time.delta` / `clock.now` / `math.random`
/// 必须一次都不出现。这是"30/60/120 FPS 同结果"的结构面——运行时断言只能证明一次推进，
/// 证明不了没人在别处读真实时间。
fn scan_wall_clock(project: &Path) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let count = |rel: &str| -> Result<usize, Box<dyn std::error::Error>> {
        let path = project.join(rel);
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Ok(text
            .lines()
            .map(|line| {
                // 去掉整行注释与行内注释（本仓这两个文件里没有含 `--` 的字符串字面量）。
                let code = line.split("--").next().unwrap_or("");
                ["time.delta", "clock.now", "math.random"]
                    .iter()
                    .filter(|needle| code.contains(**needle))
                    .count()
            })
            .sum())
    };
    Ok((
        count("scripts/systems/status.lua")?,
        count("scripts/systems/combat.lua")?,
    ))
}

fn new_runtime(project: &Path, save_path: &Path) -> Result<LuaRuntime, Box<dyn std::error::Error>> {
    let mut runtime = LuaRuntime::new()?;
    runtime.set_project_root(project.to_path_buf());
    runtime.set_save_path(save_path.to_path_buf());
    // M32 横屏：设计分辨率 1280×720（Lua 动态 UI 是屏幕像素空间，视口必须与设计尺寸一致）。
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

/// 推进 n 个固定步（输入全空：站着不动）。
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

/// 通过引擎的地图跳转入口切图（与战败回庄、出口走的是同一个入口）。
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
