//! Headless acceptance drive for M34-04a：招式扩充的**轻功（追风步）与格挡（守心诀）**
//! （`data/skills.lua` 的四类 kind + `systems/combat.lua` 的 dash/guard 分支 + `ui/input.lua` 的 H/Z）。
//!
//! 为什么要这一门：`docs/33` §6.1 的「追风步 = 位移 3 格、无敌 0.25 秒」与「守心诀 = 正面减伤
//! 70%」两行一直挂在 `jhyt_m24_numeric_alignment` ⑤ 的"未实现白名单"里，而 `docs/30` §14 的
//! MVP 锁定写的是"1 个近战招式、**1 个轻功**"——轻功至今没有实现，等于承诺过的那一条是空的。
//! 本样例把它钉成七件事：
//!
//! 1. **文档 ↔ 数据**：解析 §6.1 招式表，把两行的中文单元格按 60Hz 固定步换算后与
//!    `data/skills.lua` 逐格比对（"2.0 秒"→ 120 步、"位移 3 格"→ dash_tiles=3、"70%"→ absorb=0.7）。
//! 2. **真实按键**：位移与格挡都由物理键 H/Z 驱动（`settings.toml` 动作表里那两个动作因此是
//!    被行为证明的，不是被字符串证明的）。
//! 3. **走得动才收费**：贴墙按轻功**夹到可走格**（1 格 / 2 格），一格都走不了时返回 `dash_blocked`、
//!    内力与冷却分文不动——这是 `9277524`（WebGL 补固体格碰撞）那条语义在轻功侧的对应物。
//! 4. **无敌与减伤是两条通路**：轻功的 15 步走**既有的无敌帧计数器**（整次命中被吸收、记 dodge），
//!    格挡的 70% 只减**正面**命中的伤害（背面一分不减），且减伤不把命中减成 0（盾不是无敌）。
//! 5. **持续消耗按步扣**：每 `inner_period`（60）步扣一次 `inner_cost`（10）——那正是文档那一格
//!    「10/秒」的字面实现；内力不够自动落防，不报错也不扣成负数。
//! 6. **瞬时状态**：格挡与硬直/无敌帧/中毒同类——战败当场落防、不入档、读档必清。
//! 7. **只有固定步**：同一串按键在 30Hz / 120Hz 的 `time.delta()` 下，位移格数与扣内力节奏一致。
//!
//! 另有 11 条数据负例（每类 kind 的专属字段写错、或写了没有消费方的字段，都必须报出来）与
//! 4 条分派负例（伤害招式不能当轻功放、轻功不能走攻击底座）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m34_skills [-- <game_jhyt-root>]

use engine::core::{
    input::{InputSnapshot, KeyCode},
    script::LuaRuntime,
};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 本仓唯一的计时单位：`game_app.rs` 的 `FixedTimeStep::new(60, 8)`。
const HZ: f64 = 60.0;
/// 格边长（世界单位），与 `grid.world_to_cell` / `data/maps.lua` 同一约定。
const TILE: f64 = 32.0;
/// 门自己注册的一次敌人命中使用的伤害：**伤害由调用方给**（敌人 AI 用自己的 damage 字段），
/// 于是"减伤 70%"能对上整数：10 → 正面 3、背面 10；2 → 正面 1（向下取整后兜 1）。
const HIT_DAMAGE: f64 = 10.0;
const EDGE_DAMAGE: f64 = 2.0;

/// 演武场里"西墙 + 可走坪"的分界格：第一个"本格可走、西邻是 solid"的 `x,y`。
/// 行号取训练木人的 home.y（`tools/gen_arena_scene.py` 声明的战斗锚点行），
/// 判据用 `tilemap.is_solid` 现场扫出来——**不假设院墙在第几格**，几何挪了本门会自己找到新界。
const WEST_EDGE: &str = "(function() \
    local g = entity.find('GroundMap'); \
    if not g then return 'no_ground' end; \
    local y = require('data.enemies').get('enemy_dummy').home.y; \
    for x = 1, 19 do \
      if not tilemap.is_solid(g, x, y) and tilemap.is_solid(g, x - 1, y) then \
        return tostring(x) .. ',' .. tostring(y) end; \
    end; \
    return 'no_wall' end)()";

/// 玩家所在格（x 取整；轻功按整格位移，读数用格比用像素稳）。
const PLAYER_CELL_X: &str = "math.floor(require('player.player').position().x / 32)";

/// 玩家身上最后一条 hit 日志：`伤害/被吃掉的部分`（`absorbed` 是 M34-04 新加的字段，
/// 只有打在玩家身上的条目才有）。
const LAST_PLAYER_HIT: &str = "(function() local last; \
    for _, e in ipairs(require('systems.combat').log()) do \
      if e.event == 'hit' and e.target == 'player' then last = e end end; \
    if not last then return 'none' end; \
    return tostring(last.damage) .. '/' .. tostring(last.absorbed) end)()";

/// 轻功结算一行：`招式/格数/像素/无敌步数`（`last_dash` 是 M34-04 新查询，与 `last_knockback` 同形）。
const LAST_DASH: &str = "(function() local d = require('systems.combat').last_dash(); \
    if not d then return 'nil' end; \
    return d.skill .. '/' .. tostring(d.tiles) .. '/' .. tostring(math.floor(d.distance)) .. '/' .. \
      tostring(d.iframes) end)()";

/// 格挡三读数一行带回：`姿态/离下一次扣内力的步数/内力余额`。
const GUARD_SNAPSHOT: &str = "(function() local c = require('systems.combat'); \
    return tostring(c.is_guarding()) .. '/' .. tostring(c.guard_charge()) .. '/' .. \
      tostring(require('core.state').inner_power) end)()";

/// `combat.capture()` 的存档形状：字段数 + 有没有 guard 字样（瞬时状态不得入档）。
const CAPTURE_SHAPE: &str = "(function() local t = require('systems.combat').capture(); \
    local n = 0; \
    for k in pairs(t) do n = n + 1; \
      if tostring(k):find('guard') then return 'leaks:' .. tostring(k) end end; \
    return 'fields=' .. tostring(n) end)()";

/// 动作表里 dash/guard 两行（文本由 `input.action_map_text()` 渲染；键位真不真由第②④节的行为证明）。
const MAP_LINES: &str = "(function() local out = {}; \
    for line in (input.action_map_text() .. '\\n'):gmatch('([^\\n]*)\\n') do \
      if line:sub(1, 5) == 'dash=' or line:sub(1, 6) == 'guard=' then out[#out + 1] = line end; \
    end; \
    return table.concat(out, ',') end)()";

/// 数据负例：`(标题, Lua 片段, 期望片段)`。每条先把 `data/skills.lua` 改坏、跑 `validate_all`、
/// **再改回来**，所以本门跑完之后数据仍是干净的（第⑨节末尾会再验一次）。
const NEGATIVE_CASES: [(&str, &str, &str); 11] = [
    (
        "轻功写了判定窗字段（range 没有任何消费方）",
        "local s = require('data.skills').skills.chase_wind; local keep = s.range; s.range = 40; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.range = keep; return issues",
        "chase_wind.range: kind=movement 不出判定窗",
    ),
    (
        "格挡减伤写成 1（等于把攻击系统关掉）",
        "local s = require('data.skills').skills.guard_heart; local keep = s.absorb; s.absorb = 1; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.absorb = keep; return issues",
        "guard_heart.absorb",
    ),
    (
        "格挡给了冷却（文档那一格写的是「持续」）",
        "local s = require('data.skills').skills.guard_heart; local keep = s.cooldown; s.cooldown = 30; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.cooldown = keep; return issues",
        "kind=guard 的口径是「持续」",
    ),
    (
        "轻功缺 dash_tiles",
        "local s = require('data.skills').skills.chase_wind; local keep = s.dash_tiles; s.dash_tiles = nil; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.dash_tiles = keep; return issues",
        "dash_tiles: expected positive integer",
    ),
    (
        "招式记录里多写一个没人读的键",
        "local s = require('data.skills').skills.chase_wind; s.hair = 1; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.hair = nil; return issues",
        "chase_wind.hair: unknown skill key",
    ),
    (
        "轻功带伤害（这一类不打伤害）",
        "local s = require('data.skills').skills.chase_wind; local keep = s.damage; s.damage = 5; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.damage = keep; return issues",
        "damage: kind=movement",
    ),
    (
        "格挡吃武器加成",
        "local s = require('data.skills').skills.guard_heart; s.uses_weapon = true; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.uses_weapon = nil; return issues",
        "uses_weapon: kind=guard",
    ),
    (
        "正面扇形 0 度（谁都不算正面 = 减伤永不生效）",
        "local s = require('data.skills').skills.guard_heart; local keep = s.front_degrees; s.front_degrees = 0; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.front_degrees = keep; return issues",
        "front_degrees: expected number in (0, 360]",
    ),
    (
        "格挡缺 inner_period（「10/秒」就没法换算）",
        "local s = require('data.skills').skills.guard_heart; local keep = s.inner_period; s.inner_period = nil; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.inner_period = keep; return issues",
        "inner_period: expected positive integer",
    ),
    (
        "轻功无敌步数不是整数步",
        "local s = require('data.skills').skills.chase_wind; local keep = s.invincible_steps; s.invincible_steps = 0.5; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.invincible_steps = keep; return issues",
        "invincible_steps: expected positive integer",
    ),
    (
        "普攻不吃武器加成（basic 的定义就断了）",
        "local s = require('data.skills').skills.strike; local keep = s.uses_weapon; s.uses_weapon = nil; \
           local issues = table.concat((select(2, require('data.skills').validate_all())) or {}, ';'); \
           s.uses_weapon = keep; return issues",
        "basic skill must use weapon bonus",
    ),
];

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
    let design_doc = project.join("docs/33-江湖有田-数值设计案.md");
    let design = fs::read_to_string(&design_doc)
        .map_err(|error| format!("cannot read {}: {error}", design_doc.display()))?;
    let save_path = fresh_save_path(&project, "jhyt_m34_skills.sav");

    let mut rt = new_runtime(&project, &save_path)?;
    rt.execute_project_script(&main_script)?;
    open_scene(&mut rt, "main")?;

    // ---------------- ① 文档 ↔ 数据 ↔ kind 分派 ----------------
    banner("M34-04① docs/33 §6.1 追风步/守心诀 == data/skills.lua（按 60Hz 固定步换算）");
    let rows = markdown_table(&design, "招式")?;
    let chase = table_row(&rows, "追风步")?;
    let guard = table_row(&rows, "守心诀")?;
    // 列序：招式 | 伤害 | 内力消耗 | 冷却/恢复 | 其他。
    if chase.len() < 5 || guard.len() < 5 {
        return Err(
            format!("§6.1 两行格式不符（期望 5 列）：追风步 {chase:?} / 守心诀 {guard:?}").into(),
        );
    }
    let dash_doc_damage = number_from_cell(&chase[1]).unwrap_or(-1.0);
    let dash_doc_cost = number_from_cell(&chase[2]).unwrap_or(-1.0);
    let dash_doc_cooldown_sec = number_from_cell(&chase[3]).unwrap_or(-1.0);
    let dash_doc_tiles = number_from_cell(&chase[4]).ok_or("追风步「其他」列里没有位移格数")?;
    let dash_doc_iframe_sec = second_number(&chase[4]).ok_or("追风步「其他」列里没有无敌秒数")?;
    let guard_doc_damage = number_from_cell(&guard[1]).unwrap_or(-1.0);
    let guard_doc_cost = number_from_cell(&guard[2]).ok_or("守心诀「内力消耗」列没有数字")?;
    let guard_doc_absorb_pct =
        number_from_cell(&guard[4]).ok_or("守心诀「其他」列没有减伤百分比")?;

    let dash_tiles = skill_num(&mut rt, "chase_wind", "dash_tiles")?;
    let iframes = skill_num(&mut rt, "chase_wind", "invincible_steps")?;
    let inner_cost = skill_num(&mut rt, "chase_wind", "inner_cost")?;
    let dash_cooldown = skill_num(&mut rt, "chase_wind", "cooldown")?;
    let absorb = skill_num(&mut rt, "guard_heart", "absorb")?;
    let guard_cost = skill_num(&mut rt, "guard_heart", "inner_cost")?;
    let guard_period = skill_num(&mut rt, "guard_heart", "inner_period")?;
    let guard_cooldown = skill_num(&mut rt, "guard_heart", "cooldown")?;
    let guard_arc = skill_num(&mut rt, "guard_heart", "front_degrees")?;
    let dash_distance = dash_tiles * TILE;

    let pairs: [(&str, f64, f64); 9] = [
        (
            "追风步 伤害",
            dash_doc_damage,
            skill_num(&mut rt, "chase_wind", "damage")?,
        ),
        ("追风步 内力消耗", dash_doc_cost, inner_cost),
        (
            &format!("追风步 冷却（{dash_doc_cooldown_sec} 秒 × 60 = 固定步）"),
            dash_doc_cooldown_sec * HZ,
            dash_cooldown,
        ),
        ("追风步 位移格数", dash_doc_tiles, dash_tiles),
        (
            &format!("追风步 无敌（{dash_doc_iframe_sec} 秒 × 60 = 固定步）"),
            dash_doc_iframe_sec * HZ,
            iframes,
        ),
        (
            "守心诀 伤害",
            guard_doc_damage,
            skill_num(&mut rt, "guard_heart", "damage")?,
        ),
        ("守心诀 每期内力消耗", guard_doc_cost, guard_cost),
        (
            "守心诀 冷却（文档那一格写「持续」= 0 步）",
            0.0,
            guard_cooldown,
        ),
        (
            &format!("守心诀 正面减伤（文档「{guard_doc_absorb_pct}%」）"),
            guard_doc_absorb_pct / 100.0,
            absorb,
        ),
    ];
    for (label, doc_value, data_value) in &pairs {
        if (doc_value - data_value).abs() > 1e-9 {
            return Err(format!(
                "{label}：文档换算后 = {doc_value}，data/skills.lua = {data_value}"
            )
            .into());
        }
        println!("  {label} = {data_value}");
    }
    if chase[3].contains("持续") || guard[3].contains("秒") {
        return Err(format!(
            "§6.1 的「冷却/恢复」列语义变了（追风步 {:?} / 守心诀 {:?}）：本门的换算口径要重读",
            chase[3], guard[3]
        )
        .into());
    }
    println!("  正面扇形 = {guard_arc:.0} 度是实现口径（文档那一格只有减伤比例，没有角度列）");
    println!(
        "  维持节奏 = 每 {guard_period:.0} 步扣 {guard_cost:.0} 点内力 = 文档那一格「10/秒」在 60Hz 下的唯一读法"
    );

    assert_num(&mut rt, "#require('data.skills').ids()", 7.0)?;
    assert_str(
        &mut rt,
        "tostring(require('data.skills').validate_all())",
        "true",
    )?;
    println!("  7 条招式、validate_all 全绿（四类 kind 的专属字段各有消费方）");
    // kind 分派的唯一判据：伤害类出判定窗，轻功/格挡不出（底座因此拒收后两类，见第⑧节）。
    assert_str(
        &mut rt,
        "tostring(require('player.skills').uses_hitbox(require('data.skills').get('strike')))",
        "true",
    )?;
    for id in ["chase_wind", "guard_heart"] {
        assert_str(
            &mut rt,
            &format!(
                "tostring(require('player.skills').uses_hitbox(require('data.skills').get('{id}')))"
            ),
            "false",
        )?;
    }
    // 文档里那句"追风步/守心诀为 Alpha 未实现"必须已经改掉——留着就是假事实的第二个副本。
    if design.contains("追风步/守心诀/回风掌为 Alpha 未实现") {
        return Err(
            "docs/33 §6.1 的口径说明仍写着追风步/守心诀未实现（本门已把它们钉成行为）".into(),
        );
    }
    // 要点 3：减伤与防御不得双重扣——顺序必须写在设计案里，由本门钉住。
    if !design.contains("平减在前、比例在后") {
        return Err("docs/33 没写「平减在前、比例在后」的顺序（减伤与防御的先后无据可依）".into());
    }
    println!("  docs/33 已同步：未实现口径那句已改、减伤与防御的顺序已写进 §6.1");

    // ---------------- 进演武场 ----------------
    open_scene(&mut rt, "combat_arena")?;
    let (edge_x, edge_y) = parse_cell(&mut rt, WEST_EDGE)?
        .ok_or("演武场找不到「本格可走 + 西邻是墙」的分界格（几何变了？）")?;
    println!(
        "\n  演武坪西界 = 格({edge_x:.0},{edge_y:.0})（西邻就是院墙；这格是现场扫出来的，不是硬编码坐标）"
    );

    // ---------------- ② 真实按键 H：位移 3 格 + 扣 15 + 无敌 15 步 ----------------
    banner("M34-04② H（dash 动作）= 追风步：位移 3 格、扣 15 内力、15 步无敌挡掉一整刀");
    let dash_key = KeyCode::from_char('h').expect("QWERTY 字符必须有对应物理键");
    let guard_key = KeyCode::from_char('z').expect("QWERTY 字符必须有对应物理键");
    prepare_dash(&mut rt, edge_x + 4.0, edge_y)?;
    let (x_before, y_before) = player_pos(&mut rt)?;
    let inner_before = query_num(&mut rt, "require('core.state').inner_power")?;
    let attacks_before = query_num(&mut rt, "require('systems.combat').stats().attacks")?;
    hold(&mut rt, &[dash_key], 1)?;
    let (x_after, y_after) = player_pos(&mut rt)?;
    let inner_after = query_num(&mut rt, "require('core.state').inner_power")?;
    let want_dash = format!("chase_wind/{dash_tiles:.0}/{dash_distance:.0}/{iframes:.0}");
    assert_str(&mut rt, LAST_DASH, &want_dash)?;
    if (x_before - x_after - dash_distance).abs() > 1e-3 {
        return Err(format!(
            "位移 {x_before:.1} → {x_after:.1}（向西 {:.1} 像素），期望 {dash_distance:.1}（{dash_tiles:.0} 格）",
            x_before - x_after
        )
        .into());
    }
    if (y_after - y_before).abs() > 1e-3 {
        return Err(format!("轻功只该沿朝向走，y 从 {y_before:.1} 动到了 {y_after:.1}").into());
    }
    assert_num(&mut rt, PLAYER_CELL_X, edge_x + 1.0)?;
    assert_num(
        &mut rt,
        "require('systems.combat').player_iframes()",
        iframes,
    )?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        inner_before - inner_cost,
    )?;
    assert_num(
        &mut rt,
        "require('systems.combat').stats().attacks",
        attacks_before,
    )?;
    assert_str(&mut rt, "require('systems.combat').stats().dashes", "1")?;
    println!(
        "  {x_before:.0},{y_before:.0} → {x_after:.0},{y_after:.0}（走满 {dash_tiles:.0} 格）；内力 {inner_before:.0} → {inner_after:.0}；无敌帧读回 {iframes:.0} 步；stats.attacks 没被轻功记成一次出手（dashes=1）"
    );
    // 冷却真的挂上了：底座直读第二次必须被拒（同一条 reason 也是 HUD 的键）。
    assert_str(
        &mut rt,
        "select(2, require('systems.combat').player_dash('chase_wind'))",
        "cooldown",
    )?;
    println!(
        "  紧接着再放一次：player_dash 返回 cooldown（冷却进的是同一张 attacker_key:skill_id 键表）"
    );

    // 无敌 15 步 = 这一刀整次被吸收：伤害、硬直、击退一样都不生效，记 dodge。
    // 轻功已经把主角带离木人 3 格，判定窗够不着——搬回西侧一格再注册（不 reset，无敌帧还在走）。
    stand_west_of_dummy(&mut rt, "right")?;
    let hp_before_iframe = query_num(&mut rt, "require('systems.combat').player_hp()")?;
    let ignored_before = query_num(&mut rt, "require('systems.combat').stats().ignored")?;
    register_dummy_hit(&mut rt, HIT_DAMAGE)?;
    if !wait_for_player_log(&mut rt, &[], "dodge")? {
        return Err("轻功无敌期间木人那一刀没有被整次吸收（24 步内没等到 dodge 日志）".into());
    }
    assert_num(
        &mut rt,
        "require('systems.combat').player_hp()",
        hp_before_iframe,
    )?;
    assert_num(
        &mut rt,
        "require('systems.combat').player_hurt_frames()",
        0.0,
    )?;
    assert_num(
        &mut rt,
        "require('systems.combat').stats().ignored",
        ignored_before + 1.0,
    )?;
    println!(
        "  无敌期间挨的那一刀：气血 {hp_before_iframe:.0} 不动、硬直 0、stats.ignored +1（docs/33 §10「轻功无敌 | 不受伤害和硬直」）"
    );

    // ---------------- ③ 贴墙：夹到可走格 / 一格不通不收费 ----------------
    banner("M34-04③ 贴墙按轻功：位移夹到可走格；一格都走不了则 dash_blocked，内力与冷却分文不动");
    let mut clip_report = Vec::new();
    for (offset, want_tiles) in [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)] {
        prepare_dash(&mut rt, edge_x + offset, edge_y)?;
        let (x_before, _) = player_pos(&mut rt)?;
        let inner_step_before = query_num(&mut rt, "require('core.state').inner_power")?;
        hold(&mut rt, &[dash_key], 1)?;
        let (x_after, _) = player_pos(&mut rt)?;
        let moved = x_before - x_after;
        if want_tiles == 0.0 {
            assert_str(&mut rt, "require('core.state').last_action", "dash_blocked")?;
            assert_str(&mut rt, LAST_DASH, "nil")?;
            assert_num(
                &mut rt,
                "require('core.state').inner_power",
                inner_step_before,
            )?;
            assert_num(&mut rt, "require('systems.combat').player_iframes()", 0.0)?;
            assert_num(&mut rt, "require('systems.combat').stats().dashes", 0.0)?;
            // 没收钱也就不该进冷却：底座直读一次，仍然只会被墙挡住（而不是 cooldown）。
            assert_str(
                &mut rt,
                "select(2, require('systems.combat').player_dash('chase_wind'))",
                "dash_blocked",
            )?;
            println!(
                "  站格 {edge_x:.0}（西邻就是墙）：last_action = dash_blocked，内力 {inner_step_before:.0} 不动、无敌帧 0、dashes 不计、不进冷却"
            );
        } else {
            let want = want_tiles * TILE;
            if (moved - want).abs() > 1e-3 {
                return Err(format!(
                    "从格 {:.0} 向西轻功走了 {moved:.0} 像素，期望 {want:.0}（{want_tiles:.0} 格）",
                    edge_x + offset
                )
                .into());
            }
            assert_num(
                &mut rt,
                "require('core.state').inner_power",
                inner_step_before - inner_cost,
            )?;
            let final_cell = (x_after / TILE).floor();
            let west_solid = query_str(
                &mut rt,
                &format!(
                    "tostring(tilemap.is_solid(entity.find('GroundMap'), {:.0}, {:.0}))",
                    final_cell - 1.0,
                    edge_y
                ),
            )?;
            if west_solid != "true" {
                return Err(format!(
                    "夹到 {want_tiles:.0} 格后紧西一格（格 {:.0}）不是 solid：停下的理由不成立",
                    final_cell - 1.0
                )
                .into());
            }
            clip_report.push(format!("{want_tiles:.0}"));
            println!(
                "  站格 {:.0}：夹到 {want_tiles:.0} 格（{moved:.0} 像素）就停，紧西一格是 solid；内力照扣 {inner_cost:.0}",
                edge_x + offset
            );
        }
    }

    // ---------------- ④ 守心诀：正面减伤 / 背面不减 / 不格挡对照 ----------------
    banner(&format!(
        "M34-04④ Z（guard 动作）= 守心诀：正面 {HIT_DAMAGE:.0} → 3、背面 {HIT_DAMAGE:.0} → {HIT_DAMAGE:.0}、不格挡 {HIT_DAMAGE:.0} → {HIT_DAMAGE:.0}"
    ));
    let front_expected = (HIT_DAMAGE * (1.0 - absorb)).floor().max(1.0);
    for (case, facing, guarding, expect_hit, expect_absorbed) in [
        (
            "格挡 + 正面",
            "right",
            true,
            front_expected,
            HIT_DAMAGE - front_expected,
        ),
        ("格挡 + 背面", "left", true, HIT_DAMAGE, 0.0),
        ("不格挡 + 正面", "right", false, HIT_DAMAGE, 0.0),
    ] {
        prepare_guard(&mut rt, facing, 50.0)?;
        if guarding {
            hold(&mut rt, &[guard_key], 2)?;
            assert_str(&mut rt, "require('systems.combat').is_guarding()", "true")?;
        }
        register_dummy_hit(&mut rt, HIT_DAMAGE)?;
        let keys: &[KeyCode] = if guarding { &[guard_key] } else { &[] };
        if !wait_for_player_damage(&mut rt, keys, 100.0)? {
            return Err(format!("{case}：木人那一刀在 24 步内没打到玩家").into());
        }
        let hp = query_num(&mut rt, "require('systems.combat').player_hp()")?;
        let delta = 100.0 - hp;
        let log_line = query_str(&mut rt, LAST_PLAYER_HIT)?;
        if (delta - expect_hit).abs() > 1e-6 {
            return Err(format!(
                "{case}：吃下 {delta:.0} 点，期望 {expect_hit:.0}（absorb={absorb}、伤害 {HIT_DAMAGE:.0}）"
            )
            .into());
        }
        let want_log = format!("{expect_hit:.0}/{expect_absorbed:.0}");
        if log_line != want_log {
            return Err(format!(
                "{case}：最后一条 hit 日志 = {log_line}，期望 damage/absorbed = {want_log}"
            )
            .into());
        }
        println!("  {case}：日志 {log_line}（气血 100 → {hp:.0}）");
    }
    // 取整与"盾不是无敌"：2 点减 70% → floor 成 0，兜回 1 点。
    prepare_guard(&mut rt, "right", 50.0)?;
    hold(&mut rt, &[guard_key], 2)?;
    register_dummy_hit(&mut rt, EDGE_DAMAGE)?;
    if !wait_for_player_damage(&mut rt, &[guard_key], 100.0)? {
        return Err("边界那一刀（伤害 2）没打到玩家".into());
    }
    let edge_log = query_str(&mut rt, LAST_PLAYER_HIT)?;
    if edge_log != "1/1" {
        return Err(format!(
            "正面减伤把 {EDGE_DAMAGE:.0} 点减成 {edge_log}（damage/absorbed），期望 1/1：floor(0.6) = 0 之后兜 1 点——盾不是无敌"
        )
        .into());
    }
    println!(
        "  边界：正面吃下 {EDGE_DAMAGE:.0} 点的一刀 → 日志 {edge_log}（向下取整后兜 1 点，减伤不把命中减成 0）"
    );
    let guard_stats = query_str(
        &mut rt,
        "(function() local s = require('systems.combat').stats(); \
           return tostring(s.guard_hits) .. '/' .. tostring(s.guard_absorbed) end)()",
    )?;
    println!(
        "  本轮最后的 stats：guard_hits/guard_absorbed = {guard_stats}（只有正面被减的命中才计数）"
    );

    // ---------------- ⑤ 维持消耗按步扣 + 自动落防 ----------------
    banner(&format!(
        "M34-04⑤ 守心诀的维持消耗：按下晚一步生效、每 {guard_period:.0} 步扣一笔、内力不够自动落防"
    ));
    prepare_guard(&mut rt, "right", 50.0)?;
    hold(&mut rt, &[guard_key], 1)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "false")?;
    assert_num(&mut rt, "require('core.state').inner_power", 50.0)?;
    println!(
        "  按下当步还没起来（is_guarding=false、内力 50）：与攻击前摇同一口径，操作晚一个固定步才落到世界里"
    );
    hold(&mut rt, &[guard_key], 1)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "true")?;
    assert_num(&mut rt, "require('core.state').inner_power", 40.0)?;
    assert_num(
        &mut rt,
        "require('systems.combat').guard_charge()",
        guard_period,
    )?;
    println!(
        "  第 2 步起来：第一笔 {guard_cost:.0} 点当步就扣（50 → 40），下一笔还剩 {guard_period:.0} 步"
    );
    hold(&mut rt, &[guard_key], (guard_period - 1.0) as usize)?;
    assert_num(&mut rt, "require('systems.combat').guard_charge()", 1.0)?;
    assert_num(&mut rt, "require('core.state').inner_power", 40.0)?;
    hold(&mut rt, &[guard_key], 1)?;
    assert_num(&mut rt, "require('core.state').inner_power", 30.0)?;
    assert_num(
        &mut rt,
        "require('systems.combat').guard_charge()",
        guard_period,
    )?;
    println!(
        "  起手后正好 {guard_period:.0} 步（= 1 秒 @60Hz）扣第二笔：40 → 30，进度回到 {guard_period:.0}"
    );
    // 松手就落防（姿态不"记住"上一次按过）。落下要**两步**：步序里 combat.tick 在
    // input.update 之前，所以本步结算用的是上一步报上来的"按没按"——和按下晚一步生效同一口径。
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "false")?;
    assert_num(&mut rt, "require('systems.combat').guard_charge()", 0.0)?;
    println!(
        "  松开 Z 两步后落防、扣费进度归零（再按才重新起，第一笔重新扣；与按下晚一步生效对称）"
    );
    // 内力不够 → 自动落防：不报错、不扣成负数、补足之后能重新起。
    prepare_guard(&mut rt, "right", 5.0)?;
    hold(&mut rt, &[guard_key], 2)?;
    assert_str(&mut rt, GUARD_SNAPSHOT, "false/0/5")?;
    println!(
        "  内力 5 < {guard_cost:.0}：按满两步也起不来，也没被扣成负数（docs/33 §2「不允许负数消耗」）"
    );
    lua(&mut rt, "require('core.state').inner_power = 20")?;
    hold(&mut rt, &[guard_key], 1)?;
    assert_str(&mut rt, GUARD_SNAPSHOT, "true/60/10")?;
    println!("  内力补到 20 再按：起得来（20 → 10），落防不是永久的");

    // ---------------- ⑥ 瞬时状态 ----------------
    banner("M34-04⑥ 格挡与硬直/无敌帧/中毒同类：倒地即落防、capture 里没有 guard、读档必清");
    prepare_guard(&mut rt, "right", 50.0)?;
    hold(&mut rt, &[guard_key], 2)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "true")?;
    lua(&mut rt, "require('systems.combat').set_player_hp(0)")?;
    let snapshot = query_str(&mut rt, GUARD_SNAPSHOT)?;
    if !snapshot.starts_with("false/0/") {
        return Err(format!("战败之后格挡还挂着：{snapshot}").into());
    }
    println!(
        "  set_player_hp(0) 走 fall_player：**当步**就落防（不等下一个 tick），死人身上不挂着 70% 减伤（{snapshot}）"
    );
    // 战败期间补刀：不鞭尸（底座 reason = player_dead），也没有盾在偷偷减伤。
    assert_str(
        &mut rt,
        "(function() local c = require('systems.combat'); \
           local ok, reason = c.attack(nil, 'x', 'x', 'enemy', 'strike', 'left', 10); \
           return tostring(ok) .. '/' .. tostring(reason) end)()",
        "false/player_dead",
    )?;
    lua(&mut rt, "require('systems.combat').respawn()")?;
    frames(&mut rt, 2)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "false")?;
    println!("  重开（R）之后不会自动站回盾上：姿态由此刻按没按键决定");
    assert_str(&mut rt, CAPTURE_SHAPE, "fields=3")?;
    println!(
        "  combat.capture() 只有 version/hp/dead 三个字段（**guard 不入档**，与中毒同一裁定）"
    );
    prepare_guard(&mut rt, "right", 50.0)?;
    hold(&mut rt, &[guard_key], 2)?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "true")?;
    lua(
        &mut rt,
        "require('systems.combat').restore({ version = 1, hp = 50, dead = false })",
    )?;
    assert_str(&mut rt, "require('systems.combat').is_guarding()", "false")?;
    assert_str(&mut rt, "require('systems.combat').guard_charge()", "0")?;
    println!("  读档：盾跟着硬直/无敌帧/毒素一起清掉（读档回来站在安全位置，不继承上一局的减伤）");

    // ---------------- ⑦ 帧率无关 ----------------
    banner("M34-04⑦ 帧率无关：同一串按键在 30Hz/120Hz 的 time.delta() 下位移与扣费节奏一致");
    for (label, delta) in [("30Hz", 1.0f32 / 30.0), ("120Hz", 1.0f32 / 120.0)] {
        rt.set_delta(delta);
        prepare_dash(&mut rt, edge_x + 4.0, edge_y)?;
        hold(&mut rt, &[dash_key], 1)?;
        let dash_here = query_str(
            &mut rt,
            "(function() local d = require('systems.combat').last_dash(); \
               return d.skill .. '/' .. tostring(d.tiles) .. '/' .. tostring(d.iframes) end)()",
        )?;
        let want_here = format!("chase_wind/{dash_tiles:.0}/{iframes:.0}");
        if dash_here != want_here {
            return Err(format!(
                "{label} 这轮轻功 = {dash_here}，与 60Hz 的 {want_here} 不同（位移或无敌在吃 time.delta()）"
            )
            .into());
        }
        // 起手 1 步晚生效 + 起手段 + 第 60 步扣第二笔 = period + 2 步。
        prepare_guard(&mut rt, "right", 50.0)?;
        hold(&mut rt, &[guard_key], (guard_period + 2.0) as usize)?;
        let here = query_str(&mut rt, GUARD_SNAPSHOT)?;
        let want = format!("true/{guard_period:.0}/30");
        if here != want {
            return Err(format!(
                "{label} 这轮格挡 = {here}，期望 {want}（两笔消耗、第二笔落在起手后第 {guard_period:.0} 步）"
            )
            .into());
        }
        println!(
            "  {label}（time.delta() 读回 {:.0}ms）：轻功仍走 {dash_tiles:.0} 格 / {iframes:.0} 步无敌，格挡仍在第 {guard_period:.0} 步扣第二笔（50 → 30）",
            delta as f64 * 1000.0
        );
    }
    rt.set_delta(1.0 / 60.0);

    // ---------------- ⑧ 底座分派 ----------------
    banner("M34-04⑧ 底座只收判定窗招式：轻功走 M.player_dash、格挡走 M.player_guard");
    for (label, expr, want) in [
        (
            "把追风步交给攻击底座（没有判定窗可注册）",
            "(function() local c = require('systems.combat'); local pid = require('player.player').ensure(); \
               local ok, reason = c.attack(pid, 'player', 'player', 'player', 'chase_wind', 'left', 0); \
               return tostring(ok) .. '/' .. tostring(reason) end)()",
            "false/not_hitbox",
        ),
        (
            "把普攻当轻功放（伤害招式没有位移字段）",
            "tostring(select(2, require('systems.combat').player_dash('strike')))",
            "unknown_skill",
        ),
        (
            "把普攻当格挡按（kind 不是 guard 就不该有盾）",
            "tostring(select(2, require('systems.combat').player_guard(true, 'strike')))",
            "unknown_skill",
        ),
        (
            "招式不存在时不留下任何姿态",
            "(function() local c = require('systems.combat'); c.player_guard(true, 'not_a_skill'); \
               c.tick(); return tostring(c.is_guarding()) end)()",
            "false",
        ),
    ] {
        assert_str(&mut rt, expr, want)?;
        println!("  {label} → {want}");
    }

    // ---------------- ⑨ 数据负例 ----------------
    banner("M34-04⑨ 数据负例：每类 kind 的专属字段写错都必须报出来（11 条，全部改完即还原）");
    for (label, code, want) in NEGATIVE_CASES {
        let issues = query_str(&mut rt, code)?;
        if issues.is_empty() {
            return Err(format!("{label}：validate_all 没有报出问题（这条改动静默通过了）").into());
        }
        if !issues.contains(want) {
            return Err(format!("{label}：报错文案里没有 {want:?}，实际 {issues}").into());
        }
        println!("  {label} → {want}");
    }
    assert_str(
        &mut rt,
        "tostring(require('data.skills').validate_all())",
        "true",
    )?;
    println!("  11 条负例跑完，数据仍然全绿（每条都改了又改回去）");

    // ---------------- ⑩ HUD 与动作表 ----------------
    banner("M34-04⑩ HUD：两个新原因都有中文；dash/guard 在动作表里（键位由第②④节的行为证明）");
    let mut hud_texts = Vec::new();
    for key in ["dash_blocked", "not_hitbox"] {
        let text = query_str(&mut rt, &format!("require('ui.hud').feedback('{key}')"))?;
        if text == key || text.contains("未知反馈") {
            return Err(format!("hud.feedback('{key}') 露出英文键或兜底文案：{text}").into());
        }
        hud_texts.push(format!("{key}={text}"));
        println!("  {key} → {text}");
    }
    // 贴墙那次轻功走的是同一条渲染路径（玩家真正看到的句子）。
    let shown = query_str(
        &mut rt,
        "(function() require('core.state').last_action = 'dash_blocked'; \
           return require('ui.hud').feedback_line() end)()",
    )?;
    if !shown.contains("落脚点") {
        return Err(format!("提示区没把 dash_blocked 渲染成中文：{shown}").into());
    }
    println!("  提示区照读 last_action：{shown}");
    let lines = query_str(&mut rt, MAP_LINES)?;
    if !lines.contains("dash=") || !lines.contains("guard=") {
        return Err(format!("动作表里少了 dash/guard 行：{lines}").into());
    }
    println!("  动作表：{lines}");

    println!(
        "\njhyt_m34_skills_ok project={} dash_tiles={dash_tiles:.0} dash_cost={inner_cost:.0} \
         dash_iframes={iframes:.0} clip={} front={front_expected:.0} back={HIT_DAMAGE:.0} \
         absorb={absorb} period={guard_period:.0} edge_log={edge_log} hud={} map_lines={}",
        project.display(),
        clip_report.join("/"),
        hud_texts.join(","),
        lines
    );
    Ok(())
}

// ---------- 场景与按键 ----------

/// 按住若干键推进 n 个固定步（`keys` 为空等价于 `frames`）。
fn hold(
    runtime: &mut LuaRuntime,
    keys: &[KeyCode],
    n: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..n {
        let mut input = InputSnapshot::default();
        for key in keys {
            input.key_event(*key, true);
        }
        runtime.set_input(input);
        runtime.call_update()?;
    }
    Ok(())
}

/// 空输入推进 n 个固定步（站着不动）。
fn frames(runtime: &mut LuaRuntime, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    runtime.set_input(InputSnapshot::default());
    for _ in 0..n {
        runtime.call_update()?;
    }
    Ok(())
}

/// 轻功站位：把玩家摆在格 (x, y)、朝向 left（向西走），底座账本清零、内力写成 50。
/// 内力显式给 50 而不是"读当时是多少"：连续几次轻功的扣费要能逐次核对。
fn prepare_dash(
    runtime: &mut LuaRuntime,
    x: f64,
    y: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    lua(runtime, "require('systems.combat').reset()")?;
    lua(runtime, "require('core.state').inner_power = 50")?;
    lua(
        runtime,
        &format!(
            "require('player.player').teleport({}, {})",
            (x + 0.5) * TILE,
            (y + 0.5) * TILE
        ),
    )?;
    set_facing(runtime, "left")?;
    frames(runtime, 1)
}

/// 格挡站位：站在训练木人**西侧一格**（木人在东），朝向决定它算正面还是背面。
/// 木人是 `aggro_radius = 0 / attack_range = 0` 的练功靶，永远不动也不主动打——
/// 命中由本门直接注册到 `combat.attack`，于是"这一刀从哪来、多少伤害"完全可控。
fn prepare_guard(
    runtime: &mut LuaRuntime,
    facing: &str,
    inner: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    lua(runtime, "require('systems.combat').reset()")?;
    lua(
        runtime,
        &format!("require('core.state').inner_power = {inner}"),
    )?;
    let moved = query_str(
        runtime,
        "(function() local id = entity.find_id('enemy_dummy'); \
           if not id then return 'no_dummy' end; \
           local d = transform.get(id); \
           require('player.player').teleport(d.x - 32, d.y); return 'ok' end)()",
    )?;
    if moved != "ok" {
        return Err(format!("演武场的训练木人不可用：{moved}").into());
    }
    set_facing(runtime, facing)?;
    frames(runtime, 1)
}

/// 只搬人、不动账本：轻功的无敌帧还在倒计时，`combat.reset()` 会把它一起抹掉。
/// 轻功本身把人带走了 3 格，木人的判定窗够不着，所以吸收那一条要先把人搬回木人西侧一格。
fn stand_west_of_dummy(
    runtime: &mut LuaRuntime,
    facing: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let moved = query_str(
        runtime,
        "(function() local id = entity.find_id('enemy_dummy'); \
           if not id then return 'no_dummy' end; \
           local d = transform.get(id); \
           require('player.player').teleport(d.x - 32, d.y); return 'ok' end)()",
    )?;
    if moved != "ok" {
        return Err(format!("演武场的训练木人不可用：{moved}").into());
    }
    set_facing(runtime, facing)
}

/// 用木人当攻击者注册一次 strike（伤害由调用方给定 = 底座本来支持的形态）。
/// 先确认注册成功，免得下面的等待循环把"没注册上"等成"没打到"。
fn register_dummy_hit(
    runtime: &mut LuaRuntime,
    damage: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let registered = query_str(
        runtime,
        &format!(
            "(function() local c = require('systems.combat'); \
               local ok, reason = c.attack(entity.find_id('enemy_dummy'), 'enemy_dummy', '训练木人', \
                 'enemy', 'strike', 'left', {damage}); \
               return tostring(ok) .. '/' .. tostring(reason) end)()"
        ),
    )?;
    if registered != "true/ok" {
        return Err(format!("木人那一刀没注册上：{registered}").into());
    }
    Ok(())
}

/// 推进到玩家气血真的掉下来（最多 24 步：strike 前摇 6 + 判定窗 12 的两倍不到）。
fn wait_for_player_damage(
    runtime: &mut LuaRuntime,
    keys: &[KeyCode],
    before: f64,
) -> Result<bool, Box<dyn std::error::Error>> {
    for _ in 0..24 {
        hold(runtime, keys, 1)?;
        if query_num(runtime, "require('systems.combat').player_hp()")? < before {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 推进到战斗日志尾部出现指定事件（用于 dodge：那种命中不掉血，只能看日志）。
fn wait_for_player_log(
    runtime: &mut LuaRuntime,
    keys: &[KeyCode],
    event: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    for _ in 0..24 {
        hold(runtime, keys, 1)?;
        let last = query_str(
            runtime,
            "(function() local last; for _, e in ipairs(require('systems.combat').log()) do last = e end; \
               return last and last.event or 'none' end)()",
        )?;
        if last == event {
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_facing(runtime: &mut LuaRuntime, facing: &str) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!("require('core.state').facing = '{facing}'"),
    )
}

fn player_pos(runtime: &mut LuaRuntime) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(runtime, "require('player.player').position().x")?;
    let y = query_num(runtime, "require('player.player').position().y")?;
    Ok((x, y))
}

/// 解析 `WEST_EDGE` 的 `x,y`（`no_wall` / `no_ground` 一律当失败）。
fn parse_cell(
    runtime: &mut LuaRuntime,
    expr: &str,
) -> Result<Option<(f64, f64)>, Box<dyn std::error::Error>> {
    let raw = query_str(runtime, expr)?;
    let mut parts = raw.split(',');
    match (
        parts.next().and_then(|v| v.parse::<f64>().ok()),
        parts.next().and_then(|v| v.parse::<f64>().ok()),
    ) {
        (Some(x), Some(y)) => Ok(Some((x, y))),
        _ => Ok(None),
    }
}

// ---------- 文档解析 ----------

fn table_row<'a>(
    rows: &'a [Vec<String>],
    name: &str,
) -> Result<&'a Vec<String>, Box<dyn std::error::Error>> {
    rows.iter()
        .find(|row| row.first().map(String::as_str) == Some(name))
        .ok_or_else(|| format!("设计案招式表里找不到「{name}」行：{rows:?}").into())
}

/// 单元格里的第一个数字（"2.0 秒" → 2.0、"10/秒" → 10、"70%" → 70、"位移 3 格…" → 3）。
fn number_from_cell(cell: &str) -> Option<f64> {
    let mut run = String::new();
    for ch in cell.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            run.push(ch);
        } else if !run.is_empty() {
            break;
        }
    }
    run.parse::<f64>().ok()
}

/// 单元格里的第二个数字（"位移 3 格，无敌 0.25 秒" → 0.25）。
fn second_number(cell: &str) -> Option<f64> {
    let mut seen: Vec<f64> = Vec::new();
    let mut run = String::new();
    for ch in cell.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || ch == '.' {
            run.push(ch);
        } else {
            if !run.is_empty() {
                if let Ok(value) = run.parse::<f64>() {
                    seen.push(value);
                }
                run.clear();
            }
        }
    }
    seen.get(1).copied()
}

/// 以 `first_cell` 为表头的第一张 markdown 表（跳过对齐分隔行）。
fn markdown_table(
    markdown: &str,
    first_cell: &str,
) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            if inside {
                break;
            }
            continue;
        }
        let cells: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_owned())
            .collect();
        if !inside {
            if cells.first().map(String::as_str) == Some(first_cell) {
                inside = true;
            }
            continue;
        }
        if cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':'))
        {
            continue;
        }
        rows.push(cells);
    }
    if rows.is_empty() {
        return Err(format!("设计案里找不到以「{first_cell}」为表头的表格").into());
    }
    Ok(rows)
}

// ---------- 运行时脚手架（与 jhyt_m25 / jhyt_m34_status 同一套） ----------

fn skill_num(
    runtime: &mut LuaRuntime,
    id: &str,
    field: &str,
) -> Result<f64, Box<dyn std::error::Error>> {
    query_num(
        runtime,
        &format!("require('data.skills').skills.{id}.{field}"),
    )
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

fn open_scene(runtime: &mut LuaRuntime, scene: &str) -> Result<(), Box<dyn std::error::Error>> {
    lua(runtime, &format!("scene.load('{scene}')"))?;
    runtime.process_scene_transition()?;
    let current = query_str(runtime, "scene.reload()")?;
    if current != scene {
        return Err(format!("scene = {current:?}, want {scene:?}").into());
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
