//! Headless acceptance drive for M34-04：招式扩充的**轻功（追风步）与格挡（守心诀）**
//! （`data/skills.lua` 的四类 kind + `systems/combat.lua` 的 dash/guard 分支 + `ui/input.lua` 的 H/Z），
//! 以及 04b 的**山魈首领二阶段（冲撞与召唤）**（`data/enemies.lua` 的 phase2/summon 那组字段 +
//! `systems/enemy.lua` 的阶段分支，见下面的第⑤″节）。
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
//! **04b 加的第⑤″节**（山魈首领二阶段：冲撞、召唤、上界、不入档）与**第⑤‴节**（玄冥会徒的
//! 站防：正面减免、没举盾的对照、硬直/击退不吃减伤、落防、快照里不带姿态）合并在本门里跑，
//! 不另起
//! `jhyt_m34_boss` 样例——它用的仍是同一条 60Hz 固定步、同一个 `combat.attack` 底座（判定窗
//! 由 `resolve_hits` 按攻击者实时位置重算，所以"撞出去的那段位移"本身就是扫过去的判定窗），
//! 再加 9 条敌人数据负例。这一节同时把 docs/33 §6.2 新表逐格钉回 `data/enemies.lua`，
//! 并钉住"敌人出手不吃玩家内力"那条修正（旧行为：Boss 千风掌每次扣玩家 10 点内力，
//! 内力不足时 Boss 那一招还会被 `inner_power` 拒掉）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m34_skills [-- <game_jhyt-root>]

use rustlua_input::input::{InputSnapshot, KeyCode};
use rustlua_scripting::script::LuaRuntime;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// 本仓唯一的计时单位：`game_app.rs` 的 `FixedTimeStep::new(60, 8)`。
const HZ: f64 = 60.0;
/// 格边长（世界单位），与 `grid.world_to_cell` / `data/maps.lua` 同一约定。
const TILE: f64 = 32.0;
/// 门自己注册的一次敌人命中使用的伤害：**伤害由调用方给**（敌人 AI 用自己的 damage 字段）。
/// M34-04b 起这一格是"平减前"的原始值：玩家 defense 先扣，正面格挡再乘比例，
/// 期望值全部从 `player.stats` / `data/skills.lua` 现读现算（见第④节），这里不写结果数字。
const HIT_DAMAGE: f64 = 10.0;
/// 边界那一刀要落在**平减之后**剩 2 点上：2 × (1-0.7) 取整 = 0，再兜回 1 点，
/// 才是"盾不是无敌、减伤不把命中减成 0"那条口径的证明。注册时给 `defense + 2`，
/// 因为防御平减先跑——写死 2 的话平减就把这一刀削成 1 点，盾那一层根本轮不到取整。
const EDGE_FLAT_DAMAGE: f64 = 2.0;
/// 防御 A/B 用的临时 Buff 量（stats.add_buff 的 defense 增量）。
const DEFENSE_BUFF: f64 = 3.0;

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

/// 玩家身上最后一条 hit 日志里**防御平减掉的点数**（M34-04b 新字段，与 `absorbed` 分开记：
/// 后者只算盾的比例层，不含玩家天生那 1 点防御）。
const LAST_PLAYER_HIT_DEFENSE: &str = "(function() local last; \
    for _, e in ipairs(require('systems.combat').log()) do \
      if e.event == 'hit' and e.target == 'player' then last = e end end; \
    if not last then return 'none' end; return tostring(last.defense) end)()";

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

// ---------- ⑤″ Boss 二阶段的探针 ----------

/// Boss（山魈首领）**当前所在格**起、沿四个主轴方向扫出的第一段 6 格开阔地：
/// 返回 `x,y,方向序号`（1=东、2=西、3=北、4=南），扫不到返回 `no_span`。
/// 为什么现场扫：冲撞要沿一条直线撞出 `charge_speed*charge_steps/60` 个单位，路上不能有墙；
/// 而阶段切换之前 Boss 已经追了玩家一截，落点不是 home 格——写死坐标的那把尺会跟着地图重排
/// 一起过期（`WEST_EDGE` 那一格同理）。
const BOSS_SPAN: &str = "(function() local e = require('systems.enemy'); \
    local q = e.query('enemy_ape_boss'); if not q or not q.live then return 'no_boss' end; \
    local g = entity.find('GroundMap'); if not g then return 'no_ground' end; \
    local info = tilemap.get(g); local cx, cy = q.cell_x, q.cell_y; \
    local dirs = { {1, 0}, {-1, 0}, {0, 1}, {0, -1} }; \
    for index, d in ipairs(dirs) do \
      local open = true; \
      for k = 0, 5 do \
        local sx, sy = cx + d[1] * k, cy + d[2] * k; \
        if sx < 1 or sy < 1 or sx >= info.width - 1 or sy >= info.height - 1 \
          or tilemap.is_solid(g, sx, sy) then open = false; break end; \
      end; \
      if open then return tostring(cx) .. ',' .. tostring(cy) .. ',' .. tostring(index) end; \
    end; \
    return 'no_span' end)()";

/// 冲撞过程的一步读数：`是否在撞/战斗帧/Boss 的 x/Boss 的 y`（合成一段读，循环里每步只跑一次 exec）。
const BOSS_READOUT: &str = "(function() local e = require('systems.enemy'); \
    local q = e.query('enemy_ape_boss'); local id = entity.find_id('enemy_ape_boss'); \
    if not id or not q or not q.live then return 'gone/0/0/0' end; \
    local p = transform.get(id); \
    return (q.charging and 't' or 'f') .. '/' .. tostring(require('systems.combat').frame()) .. '/' .. \
      tostring(p.x) .. '/' .. tostring(p.y) end)()";

/// 场上敌人数量与冲撞姿态：`活着的召唤物/占用的槽位/本图活着的敌人总数/是否在冲撞`
/// （任务要点 5 要断言的就是前两格；合成一段读，采样循环里每步只跑一次 exec）。
const BOSS_POPULATION: &str = "(function() local e = require('systems.enemy'); \
    local live, occupied, total, charging = 0, 0, 0, 'f'; \
    for _, row in ipairs(require('data.enemies').list()) do \
      local q = e.query(row.persist_id); \
      if q and q.live then total = total + 1 end; \
      if row.summoned and q and q.summon_active then occupied = occupied + 1; \
        if q.live then live = live + 1 end end; \
      if q and row.persist_id == 'enemy_ape_boss' and q.charging then charging = 't' end; \
    end; \
    return tostring(live) .. '/' .. tostring(occupied) .. '/' .. tostring(total) .. '/' .. charging end)()";

/// 本图"天然存在的敌人"行数（非召唤槽位）：召唤上界的基数从数据现读，
/// 以后往寒泉洞再放一只常规敌人，`total` 那条上界会自己跟着长，不会留下一个过期常量。
const NATIVE_ROWS: &str = "(function() local d = require('data.enemies'); \
    local here = d.get('enemy_ape_boss').map; local n = 0; \
    for _, row in ipairs(d.list()) do \
      if not row.summoned and row.map == here then n = n + 1 end end; \
    return tostring(n) end)()";

/// `enemy.capture()` 的形状：`rows=N`，或 `leaks:<persist_id>`（槽位进了快照 = 残留）。
/// 判据用数据里的 `summoned` 标志，而不是 id 里有没有 `whelp` 字样——改名不会让这条断言变哑。
const ENEMY_CAPTURE_SHAPE: &str = "(function() local d = require('data.enemies'); \
    local snap = require('systems.enemy').capture(); local n = 0; \
    for _, row in ipairs(snap.enemies) do n = n + 1; \
      local def = d.get(row.persist_id); \
      if def and def.summoned then return 'leaks:' .. tostring(row.persist_id) end end; \
    return 'rows=' .. tostring(n) end)()";

/// 敌人出手**不吃玩家内力**（docs/33 §6.2 三条口径之③）：把玩家内力写成 5（低于千风掌的
/// `inner_cost = 10`）再直接注册一次 Boss 的那一招，期望 `true/ok/5`——修掉的旧行为是
/// `false/inner_power/5`（Boss 打不出这一招）或 `true/ok/-5`（把玩家的池子扣穿）。
const ENEMY_INNER_PROBE: &str = "(function() local c = require('systems.combat'); \
    local d = require('data.enemies'); local boss = d.get('enemy_ape_boss'); \
    require('core.state').inner_power = 5; \
    local ok, reason = c.attack(entity.find_id('enemy_ape_boss'), 'enemy_ape_boss', boss.name, \
      'enemy', boss.skill, 'right', boss.damage); \
    return tostring(ok) .. '/' .. tostring(reason) .. '/' .. \
      tostring(require('core.state').inner_power) end)()";

/// 召唤槽位这一组的三件事一次读回：`寿命/行数/寿命是否不统一`。行数就是"上界"，
/// 寿命段只在组内自比（不与文档数字预比较），所以 600 与 601 并存时第三段会报 1。
const SLOT_SHAPE: &str = "(function() local d = require('data.enemies'); \
    local group = d.get('enemy_ape_boss').summon_group; \
    local ttl, rows, differs = -1, 0, 0; \
    for _, row in ipairs(d.list()) do \
      if row.summoned and row.summon_group == group then \
        rows = rows + 1; \
        if ttl == -1 then ttl = row.summon_ttl or -1 elseif ttl ~= (row.summon_ttl or -1) then differs = 1 end \
      end end; \
    return tostring(ttl) .. '/' .. tostring(rows) .. '/' .. tostring(differs) end)()";

/// 敌人侧的数据负例（⑤″/⑤‴ 新加的两组校验）。这批的共同点是**改错了不会响**：
/// `phase2_below` 写得比 hp 大 → Boss 只是"从头到尾少一招"；`summon_ttl` 打错一个字母 →
/// 召唤物永远不死；`summon_group` 拼错 → 每 5 秒什么也没召出来。表现都是"Boss 好像变弱了"，
/// 所以只能在加载期报出来。每条改坏 → `validate_all` → **立刻改回**，跑完数据仍是干净的。
const ENEMY_NEGATIVE_CASES: [(&str, &str, &str); 9] = [
    (
        "二阶段阈值写得比满血还高（永远进不去）",
        "local e = require('data.enemies').get('enemy_ape_boss'); local keep = e.phase2_below; \
           e.phase2_below = e.hp + 10; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.phase2_below = keep; return issues",
        "（二阶段永远进不去）",
    ),
    (
        "冲撞招式选了一招轻功（注册不出判定窗）",
        "local e = require('data.enemies').get('enemy_ape_boss'); local keep = e.charge_skill; \
           e.charge_skill = 'chase_wind'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.charge_skill = keep; return issues",
        "kind=movement 不出判定窗，撞不到人",
    ),
    (
        "召唤槽位漏了寿命（小怪永远不会散）",
        "local w = require('data.enemies').get('enemy_ape_whelp_1'); local keep = w.summon_ttl; \
           w.summon_ttl = nil; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           w.summon_ttl = keep; return issues",
        ".summon_ttl: expected positive integer",
    ),
    (
        "槽位的组名和主人对不上（这一行永远召不出来）",
        "local w = require('data.enemies').get('enemy_ape_whelp_2'); local keep = w.summon_group; \
           w.summon_group = 'ape_whelp'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           w.summon_group = keep; return issues",
        ".summon_group: does not match owner",
    ),
    (
        "普通敌人行写了 phase2_below（没有消费方）",
        "local w = require('data.enemies').get('enemy_wolf'); w.phase2_below = 5; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           w.phase2_below = nil; return issues",
        "这个数字没有消费方",
    ),
    (
        "敌人记录里多写一个没人读的键",
        "local e = require('data.enemies').get('enemy_ape_boss'); e.roar = 1; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.roar = nil; return issues",
        ": unknown enemy key",
    ),
    (
        "格挡招式指到普攻（读不到 absorb，减伤静默变成 0）",
        "local e = require('data.enemies').get('enemy_xuanming'); local keep = e.guard_skill; \
           e.guard_skill = 'strike'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.guard_skill = keep; return issues",
        "不是格挡招式",
    ),
    (
        "格挡招式 id 打错字（盾永远举不起来）",
        "local e = require('data.enemies').get('enemy_xuanming'); local keep = e.guard_skill; \
           e.guard_skill = 'guard_hart'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.guard_skill = keep; return issues",
        ".guard_skill: unknown skill",
    ),
    (
        "不出刀的行写了 guard_skill（没有空档可站防 = 死键）",
        "local e = require('data.enemies').get('enemy_dummy'); e.guard_skill = 'guard_heart'; \
           local issues = table.concat((select(2, require('data.enemies').validate_all())) or {}, ';'); \
           e.guard_skill = nil; return issues",
        "永远站不起防",
    ),
];

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

    assert_num(&mut rt, "#require('data.skills').ids()", 8.0)?;
    assert_str(
        &mut rt,
        "tostring(require('data.skills').validate_all())",
        "true",
    )?;
    println!(
        "  8 条招式、validate_all 全绿（四类 kind 的专属字段各有消费方；第 8 条是 Boss 的山魈冲撞，见⑤″）"
    );
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

    // ---------------- ④ 伤害公式：平减在前（defense）+ 比例在后（守心诀） ----------------
    // docs/33 §6.1 的减伤顺序在 M34-04b 落进 `apply_damage`：期望值全部从属性模型和
    // 招式数据现读现算，任何一层改动（防具落地、absorb 调参）都会让这里自己跟上。
    let defense = query_num(&mut rt, "require('player.stats').query('player','defense')")?;
    let after_flat = (HIT_DAMAGE - defense).max(1.0);
    let front_expected = (after_flat * (1.0 - absorb)).floor().max(1.0);
    banner(&format!(
        "M34-04④ 伤害公式：玩家 defense = {defense:.0}，{HIT_DAMAGE:.0} 点平减后 {after_flat:.0}；正面格挡再乘 (1 - {absorb}) → {front_expected:.0}，背面与不格挡都是 {after_flat:.0}"
    ));
    for (case, facing, guarding, expect_hit, expect_absorbed) in [
        (
            "格挡 + 正面",
            "right",
            true,
            front_expected,
            after_flat - front_expected,
        ),
        ("格挡 + 背面", "left", true, after_flat, 0.0),
        ("不格挡 + 正面", "right", false, after_flat, 0.0),
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
                "{case}：吃下 {delta:.0} 点，期望 {expect_hit:.0}\
                 （原始 {HIT_DAMAGE:.0} - defense {defense:.0} = {after_flat:.0}，absorb={absorb}）"
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
    // 取整与"盾不是无敌"：平减之后剩 2 点，减 70% → floor 成 0，兜回 1 点。
    let edge_raw = defense + EDGE_FLAT_DAMAGE;
    prepare_guard(&mut rt, "right", 50.0)?;
    hold(&mut rt, &[guard_key], 2)?;
    register_dummy_hit(&mut rt, edge_raw)?;
    if !wait_for_player_damage(&mut rt, &[guard_key], 100.0)? {
        return Err(format!("边界那一刀（原始 {edge_raw:.0}）没打到玩家").into());
    }
    let edge_log = query_str(&mut rt, LAST_PLAYER_HIT)?;
    if edge_log != "1/1" {
        return Err(format!(
            "正面减伤把平减后 {EDGE_FLAT_DAMAGE:.0} 点的一刀减成 {edge_log}（damage/absorbed），期望 1/1：\
             floor(0.6) = 0 之后兜 1 点——盾不是无敌"
        )
        .into());
    }
    println!(
        "  边界：原始 {edge_raw:.0} 点 → 平减剩 {EDGE_FLAT_DAMAGE:.0} → 正面减伤 floor(0.6)=0 后兜 1 点，日志 {edge_log}（减伤不把命中减成 0）"
    );
    let guard_stats = query_str(
        &mut rt,
        "(function() local s = require('systems.combat').stats(); \
           return tostring(s.guard_hits) .. '/' .. tostring(s.guard_absorbed) end)()",
    )?;
    println!(
        "  本轮最后的 stats：guard_hits/guard_absorbed = {guard_stats}（只有正面被减的命中才计数；\
         `absorbed` 只算盾那一层，防御平减记在 `defense` 字段里）"
    );

    // ---------------- ④′ 防御结算 A/B：加成层每一点 defense 都换成等量减伤 ----------------
    banner(&format!(
        "M34-04④′ 防御结算：同一刀 {HIT_DAMAGE:.0} 点，defense {defense:.0} vs {defense:.0}+{DEFENSE_BUFF:.0} 两次吃下的差 = {DEFENSE_BUFF:.0}"
    ));
    // 先确认这一层的来源：`equipment_bonus` 的 stats 分支今天没有数据行可走
    // （docs/33 没有防具表、`data/items.lua`/`data/tools.lua` 里没有任何带 defense 的装备，
    // grep 全仓 `defense` 只有 stats.lua 与 registry_check 两处），所以 A/B 走 Buff 层。
    // 两层在 `stats.query` 里是同一句加法，来源不同而通路相同——装备层一落地，
    // 这条断言不用改（它比的是两次的**差**）。
    let equipped_defense = query_num(
        &mut rt,
        "(require('player.stats').equipment_bonus('player').defense) or 0",
    )?;
    println!("  装备层 defense 当前 = {equipped_defense:.0}（防具还没有数据行）");
    let mut flat_cases = Vec::new();
    let mut taken_without_buff = 0.0;
    let mut taken_with_buff = 0.0;
    for (label, buffed) in [("不穿（只有基础防御）", false), ("加成层 +3", true)] {
        prepare_guard(&mut rt, "right", 50.0)?;
        lua(&mut rt, "require('player.stats').reset()")?;
        if buffed {
            let added = query_str(
                &mut rt,
                &format!(
                    "tostring(require('player.stats').add_buff('player', \
                       {{ stat = 'defense', amount = {DEFENSE_BUFF}, days = 1 }}))"
                ),
            )?;
            if added != "true" {
                return Err(format!("加 defense Buff 被拒绝：{added}").into());
            }
        }
        let query = query_num(&mut rt, "require('player.stats').query('player','defense')")?;
        let want_query = defense + if buffed { DEFENSE_BUFF } else { 0.0 };
        if (query - want_query).abs() > 1e-6 {
            return Err(format!(
                "{label}：stats.query('player','defense') = {query:.0}，期望 {want_query:.0}"
            )
            .into());
        }
        register_dummy_hit(&mut rt, HIT_DAMAGE)?;
        if !wait_for_player_damage(&mut rt, &[], 100.0)? {
            return Err(format!("{label}：木人那一刀在 24 步内没打到玩家").into());
        }
        let delta = 100.0 - query_num(&mut rt, "require('systems.combat').player_hp()")?;
        let defense_log = query_str(&mut rt, LAST_PLAYER_HIT_DEFENSE)?;
        let want_taken = (HIT_DAMAGE - query).max(1.0);
        if (delta - want_taken).abs() > 1e-6 {
            return Err(format!(
                "{label}：吃下 {delta:.0} 点，期望 max(1, {HIT_DAMAGE:.0} - {query:.0}) = {want_taken:.0}"
            )
            .into());
        }
        if (defense_log.parse::<f64>().unwrap_or(-1.0) - (HIT_DAMAGE - want_taken)).abs() > 1e-6 {
            return Err(format!(
                "{label}：hit 日志的 defense 字段 = {defense_log}，期望扣掉的点数 {:.0}",
                HIT_DAMAGE - want_taken
            )
            .into());
        }
        if buffed {
            taken_with_buff = delta;
        } else {
            taken_without_buff = delta;
        }
        flat_cases.push(format!(
            "{label} defense={query:.0}→吃{delta:.0}/记{defense_log}"
        ));
    }
    lua(&mut rt, "require('player.stats').reset()")?;
    let restored = query_num(&mut rt, "require('player.stats').query('player','defense')")?;
    if (restored - defense).abs() > 1e-6 {
        return Err(
            format!("Buff 清掉之后 defense 应回到基础值 {defense:.0}，实际 {restored:.0}").into(),
        );
    }
    if (taken_without_buff - taken_with_buff - DEFENSE_BUFF).abs() > 1e-6 {
        return Err(format!(
            "两次吃下的差 = {:.0}，期望正好等于加成层那 {DEFENSE_BUFF:.0} 点防御",
            taken_without_buff - taken_with_buff
        )
        .into());
    }
    println!("  {}；差 = {DEFENSE_BUFF:.0} 点", flat_cases.join("；"));
    // 对照：敌方侧**没有**这一层——`data/enemies.lua` 没有 defense 字段，玩家那一刀吃满。
    let enemy_control = query_str(
        &mut rt,
        "(function() local e = require('data.enemies').get('enemy_dummy'); \
           return tostring(e.defense) end)()",
    )?;
    if enemy_control != "nil" {
        return Err(format!(
            "data/enemies.lua 出现了 defense 字段（{enemy_control}）：敌方侧的平减要一起进公式，\
             本门的对照断言与 docs/33 §6.2 的口径都得重写"
        )
        .into());
    }
    println!(
        "  对照：敌人数据没有 defense 字段，所以玩家那一刀不被平减（docs/33 §6.2 的防御列仍是 Alpha 目标）"
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

    // ---------------- ⑤′ 打坐：docs/33 §3 那一格走真实按键路径 ----------------
    // ⑤ 是按秒**扣**内力的那一半，打坐是**回**的那一半，两半都在同一条按键通路上结算，
    // 所以这里按的是 Q（`practice` 动作），不是直接调 Lua 函数——绕过输入层的 +5 说明不了玩家按得到。
    let meditate_rows = markdown_table(&design, "行为")?;
    let meditate = meditate_rows
        .iter()
        .find(|row| row.first().map(String::as_str) == Some("打坐"))
        .ok_or("docs/33 §3 的劳动表里没有「打坐」那一行")?;
    if meditate.len() < 4 {
        return Err(format!("§3 打坐行格式不符（期望 4 列）：{meditate:?}").into());
    }
    let meditate_gain = first_number(&meditate[3]).ok_or(format!(
        "§3 打坐「产出/效果」列里没有数字：{:?}",
        meditate[3]
    ))?;
    // 起点与上限也从 §2.2 那张表读：文档改数字，这一节跟着动（`ui/input.lua` 的
    // `min(100, …)` 与 `core/state.lua` 的 `inner_power = 50` 就是被这两格钉住的）。
    let attr_rows = markdown_table(&design, "属性")?;
    let inner_max = doc_row_number(&attr_rows, "最大内力", 1)?;
    let inner_start = doc_row_number(&attr_rows, "内力", 1)?;
    banner(&format!(
        "M34-04⑤′ 打坐（Q = practice 动作）：{:?} = 每次 +{meditate_gain:.0} 内力，封顶 {inner_max:.0}，不按不重复",
        meditate[3]
    ));
    let practice_key = KeyCode::from_char('q').expect("QWERTY 字符必须有对应物理键");
    lua(
        &mut rt,
        &format!("require('core.state').inner_power = {inner_start}"),
    )?;
    hold(&mut rt, &[practice_key], 1)?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        inner_start + meditate_gain,
    )?;
    // 再空走两步：一次按键只结算一次（`action_pressed` 是边沿，不是"按着就一直回"）。
    frames(&mut rt, 2)?;
    assert_num(
        &mut rt,
        "require('core.state').inner_power",
        inner_start + meditate_gain,
    )?;
    println!(
        "  内力 {inner_start:.0} → {:.0}（按一步 +{meditate_gain:.0}，松手后再走两步不再往上加）",
        inner_start + meditate_gain
    );
    // 封顶：离上限只差 2 点时也只能加到上限。
    lua(
        &mut rt,
        &format!("require('core.state').inner_power = {}", inner_max - 2.0),
    )?;
    hold(&mut rt, &[practice_key], 1)?;
    assert_num(&mut rt, "require('core.state').inner_power", inner_max)?;
    println!(
        "  内力 {:.0} 再按一次 = {inner_max:.0}（夹在上限，不会加过头）",
        inner_max - 2.0
    );
    // HUD 侧的同一格：打坐把提示写成中文句子（m24 锁的是文档单元格文本，这里锁玩家看到的句子）。
    let line = query_str(&mut rt, "require('ui.hud').feedback_line()")?;
    if !line.contains("打坐") {
        return Err(format!(
            "按过 practice 之后提示区没有打坐的中文句子：{line}（state.last_action = {:?}）",
            query_str(&mut rt, "require('core.state').last_action")?
        )
        .into());
    }
    println!("  提示区：{line}");

    // ---------------- ⑤″ 山魈首领二阶段：冲撞、召唤、上界、不入档 ----------------
    // `docs/30` §6 承诺的"重击、冲撞、召唤小怪"里后两样的落地证明。整节在 Boss 所在那张图跑
    // （地图 id 从 `data/enemies.lua` 现读，不写死图名），跑完切回演武场——⑥ 以后的用例要用训练木人。
    let boss_rows = markdown_table(&design, "Boss 行为")?;
    let doc_below = doc_row_number(&boss_rows, "phase2_below", 1)?;
    let doc_charge_speed = doc_row_number(&boss_rows, "charge_speed", 1)?;
    let doc_charge_steps = doc_row_number(&boss_rows, "charge_steps", 1)?;
    let doc_charge_cd = doc_row_number(&boss_rows, "charge_cooldown", 1)?;
    let doc_interval = doc_row_number(&boss_rows, "summon_interval", 1)?;
    let doc_ttl = doc_row_number(&boss_rows, "summon_ttl", 1)?;
    let doc_cap = doc_row_number(&boss_rows, "summon_cap", 1)?;

    let enemy_def = "require('data.enemies')";
    let boss_ref = format!("{enemy_def}.get('enemy_ape_boss')");
    let boss_map = query_str(&mut rt, &format!("{boss_ref}.map"))?;
    let boss_name = query_str(&mut rt, &format!("{boss_ref}.name"))?;
    let boss_hp = query_num(&mut rt, &format!("{boss_ref}.hp"))?;
    let boss_damage = query_num(&mut rt, &format!("{boss_ref}.damage"))?;
    let boss_below = query_num(&mut rt, &format!("{boss_ref}.phase2_below"))?;
    let boss_charge_speed = query_num(&mut rt, &format!("{boss_ref}.charge_speed"))?;
    let boss_charge_steps = query_num(&mut rt, &format!("{boss_ref}.charge_steps"))?;
    let boss_interval = query_num(&mut rt, &format!("{boss_ref}.summon_interval"))?;
    let charge_cooldown = skill_num(&mut rt, "ape_charge", "cooldown")?;
    let charge_damage = skill_num(&mut rt, "ape_charge", "damage")?;
    let charge_windup = skill_num(&mut rt, "ape_charge", "windup")?;
    let charge_active = skill_num(&mut rt, "ape_charge", "active")?;
    // 上界**不另立一个 `max_alive` 数字**：组里有几行槽位 = 场上同时存在几只，一份事实。
    // 寿命一次读回"组里第一行的值 + 行数是几 + 后面有没有行跟它不一样"：拿文档的 600 去数
    // "等于 600 的行数"是自我循环（写错的那个数字反而不会被数进去），所以判据只认数据自己。
    let slot_shape = probe_fields(&mut rt, SLOT_SHAPE)?;
    if slot_shape.len() != 3 {
        return Err(format!("召唤槽位读数的形状变了：{slot_shape:?}（期望 3 段）").into());
    }
    let (slot_ttl, slot_rows, slot_ttl_differs) = (slot_shape[0], slot_shape[1], slot_shape[2]);
    if slot_ttl_differs > 0.5 {
        return Err(
            "同一组召唤槽位的 summon_ttl 不是同一个数字：「上界 2 只」底下藏着两套寿命计时器"
                .into(),
        );
    }
    let enemy_rows = query_num(&mut rt, &format!("#{enemy_def}.list()"))?;
    let native_rows = query_num(&mut rt, NATIVE_ROWS)?;

    banner(&format!(
        "M34-04b⑤″ docs/33 §6.2「Boss 行为」表 == 数据：{boss_name}（{boss_map}）二阶段的冲撞与召唤，上界 {doc_cap:.0} 只"
    ));
    let boss_pairs: [(&str, f64, f64); 7] = [
        ("phase2_below（气血阈值）", doc_below, boss_below),
        (
            "charge_speed（冲撞速度）",
            doc_charge_speed,
            boss_charge_speed,
        ),
        (
            "charge_steps（冲撞步数）",
            doc_charge_steps,
            boss_charge_steps,
        ),
        (
            "charge_cooldown（= data/skills.lua:ape_charge.cooldown，AI 不自建计时器）",
            doc_charge_cd,
            charge_cooldown,
        ),
        ("summon_interval（补槽间隔）", doc_interval, boss_interval),
        ("summon_ttl（召唤物寿命）", doc_ttl, slot_ttl),
        ("summon_cap（= 组里的槽位数）", doc_cap, slot_rows),
    ];
    for (label, doc_value, data_value) in &boss_pairs {
        if (doc_value - data_value).abs() > 1e-9 {
            return Err(format!("{label}：docs/33 = {doc_value}，数据 = {data_value}").into());
        }
        println!("  {label} = {data_value}");
    }
    if (boss_below * 2.0 - boss_hp).abs() > 1e-9 {
        return Err(format!(
            "文档那一格写的是「满血 {boss_hp:.0} 的一半」，实际阈值 = {boss_below:.0}"
        )
        .into());
    }
    if (charge_damage - boss_damage).abs() > 1e-9 {
        return Err(format!(
            "口径①破了：ape_charge.damage = {charge_damage}，Boss 的 damage = {boss_damage}——\
             敌人的伤害源只有 `data/enemies.lua` 那一处，招式那格必须是相等的镜像"
        )
        .into());
    }
    println!(
        "  镜像与文案：冲撞伤害 = Boss damage = {boss_damage:.0}；阈值 {boss_below:.0} = 满血 {boss_hp:.0} 的一半"
    );
    println!(
        "  判定窗形状（文档没有对应列，是实现口径）：前摇 {charge_windup:.0} 步 + 开窗 {charge_active:.0} 步 = {:.0} 步，\
         而冲撞最多走 {boss_charge_steps:.0} 步——窗比腿长，撞到底也不会提前关门",
        charge_windup + charge_active
    );

    open_scene(&mut rt, &boss_map)?;
    lua(&mut rt, "require('systems.combat').reset()")?;
    // 一阶段：新进图 Boss 满血，二阶段的两样本事都该被 hp 挡住。玩家站在洞口出生点不动，
    // Boss 自己追过来——这 200 步同时用来证明**AI 一直在决策**，否则"没召唤"只是"没醒着"。
    let mut phase1_live = 0.0;
    let mut phase1_charged = false;
    for _ in 0..200 {
        frames(&mut rt, 1)?;
        let pop = probe_fields(&mut rt, BOSS_POPULATION)?;
        if pop[0] > phase1_live {
            phase1_live = pop[0];
        }
        if pop[3] > 0.5 {
            phase1_charged = true;
        }
    }
    assert_num(
        &mut rt,
        "require('systems.enemy').query('enemy_ape_boss').phase",
        1.0,
    )?;
    if phase1_live != 0.0 || phase1_charged {
        return Err(format!(
            "hp 还没掉过阈值就出现了召唤/冲撞（幼崽最多 {phase1_live:.0} 只、起冲过 {} 次）：阶段判据漏了",
            if phase1_charged { "1" } else { "0" }
        )
        .into());
    }
    let phase1_state = query_str(
        &mut rt,
        "require('systems.enemy').query('enemy_ape_boss').state",
    )?;
    let boss_attacks = query_num(&mut rt, "require('systems.combat').stats().attacks")?;
    if phase1_state != "chase" && phase1_state != "attack" {
        return Err(format!(
            "一阶段这 200 步里 Boss 的 state={phase1_state}（既没追也没打）：\
             \"没召唤\"这条观察是假绿，得先让 AI 醒着"
        )
        .into());
    }
    println!(
        "  一阶段 200 步：召唤 0 只、冲撞 0 次，而 Boss 一直在决策（state={phase1_state}、这一局出手 {boss_attacks:.0} 次）"
    );

    // 口径③：敌人出手不吃玩家内力。内力写成 5（低于千风掌的 inner_cost=10）再直接注册一次。
    lua(&mut rt, "require('systems.combat').reset()")?;
    assert_str(&mut rt, ENEMY_INNER_PROBE, "true/ok/5")?;
    println!(
        "  口径③：玩家内力只有 5 点（< 千风掌的 inner_cost）时 Boss 照打，池子还是 5——\
         旧的错误读法要么是 false/inner_power（Boss 打不出这一招），要么是内力被扣穿"
    );

    // 开阔带：从 Boss 当前格沿主轴方向现场扫出 6 格无墙的一条（Boss 刚追过玩家，落点不是 home 格）。
    let (stand, dir, dir_name) = boss_span(&mut rt)?;
    let wall = (stand.0 + dir.0 * 2.0, stand.1 + dir.1 * 2.0);
    println!(
        "  冲撞用的开阔带 = 格({:.0},{:.0}) 沿{dir_name} 6 格无墙（现场扫的；玩家钉在同方向第 4 格 = 128 单位外，够得着但打不着）",
        stand.0, stand.1
    );
    anchor_boss_witness(&mut rt, dir, 4.0)?;
    lua(&mut rt, "require('systems.combat').reset()")?;
    let after_hit = query_str(
        &mut rt,
        &format!(
            "(function() local e = require('systems.enemy'); \
               local q = e.query('enemy_ape_boss'); \
               return tostring(e.hit(entity.find_id('enemy_ape_boss'), q.hp - {boss_below})) end)()"
        ),
    )?;
    if after_hit != format!("{boss_below:.0}") {
        return Err(format!(
            "把 Boss 打到阈值那一格失败：enemy.hit 返回 {after_hit:?}，期望 {boss_below:.0}（后面的观察全都不成立）"
        )
        .into());
    }
    assert_num(
        &mut rt,
        "require('systems.enemy').query('enemy_ape_boss').phase",
        2.0,
    )?;
    // 二阶段的**第一步**：补满空槽位 + 注册冲撞（同一格里两件事都发生，而注册那一步不位移）。
    frames(&mut rt, 1)?;
    let (started, start_frame, x0, y0) = charge_readout(&mut rt)?;
    if !started {
        return Err("掉到阈值之后第一步没起冲：距离/冷却/阶段至少一条不对".into());
    }
    let cells = summon_slot_cells(&mut rt, stand)?;
    if cells != "ok" {
        return Err(format!("召唤落点与数据里的 summon_offset 对不上：{cells}").into());
    }
    let born = probe_fields(&mut rt, BOSS_POPULATION)?;
    if born[0] != doc_cap || born[1] != doc_cap {
        return Err(format!(
            "进二阶段的第一步该一次召满 {doc_cap:.0} 只，实际活着的 {:.0} 只、占用的槽位 {:.0} 个",
            born[0], born[1]
        )
        .into());
    }
    println!(
        "  二阶段第一步：一次召满 {doc_cap:.0} 只幼崽（落点 = 主人格 ± 数据的 summon_offset），同时起冲"
    );

    // 撞墙提前停：在路径第 2 格现造一堵墙（`tilemap.set_solid`，`9277524` 给的那条 API）。
    // 为什么要现造：开阔带里本来没有墙，"撞出去 100 单位"这一路撞不到东西，那半条口径就没证据。
    set_cell_solid(&mut rt, wall, true)?;
    if !cell_is_solid(&mut rt, wall)? {
        return Err(format!(
            "tilemap.set_solid 没把格({:.0},{:.0}) 变成 solid：撞墙观察不成立",
            wall.0, wall.1
        )
        .into());
    }
    let mut wall_run = ChargeRun::at(start_frame, x0, y0);
    charge_finish(
        &mut rt,
        dir,
        4.0,
        &mut wall_run,
        (boss_charge_steps + 20.0) as usize,
    )?;
    let full_moved = boss_speed_distance(boss_charge_speed, boss_charge_steps);
    if wall_run.moved >= 2.0 * TILE {
        return Err(format!(
            "撞到 solid 之后还在往前走：位移 {:.1} 单位，而墙就在第 2 格（{:.0} 单位外）——不穿墙那条破了",
            wall_run.moved, 2.0 * TILE
        )
        .into());
    }
    if wall_run.steps as f64 >= boss_charge_steps {
        return Err(format!(
            "撞墙那次走满了 {boss_charge_steps:.0} 步（位移 {:.1}）：提前结束没实现",
            wall_run.moved
        )
        .into());
    }
    let landed = boss_cell(&mut rt)?;
    if cell_is_solid(&mut rt, landed)? {
        return Err(format!(
            "冲撞结束时 Boss 停在 solid 格({:.0},{:.0}) 里（穿墙）",
            landed.0, landed.1
        )
        .into());
    }
    println!(
        "  撞墙提前停：{} 步 / {:.1} 单位就收住（满打满算该走 {full_moved:.1} 单位），落点格({:.0},{:.0}) 不是 solid",
        wall_run.steps, wall_run.moved, landed.0, landed.1
    );
    set_cell_solid(&mut rt, wall, false)?;
    if cell_is_solid(&mut rt, wall)? {
        return Err(format!(
            "墙没拆掉：格({:.0},{:.0}) 还是 solid，后面的观察会被它挡住",
            wall.0, wall.1
        )
        .into());
    }

    // 满程冲撞：没有墙时走满 charge_steps，位移 = charge_speed × charge_steps / 60。
    lua(&mut rt, "require('systems.combat').reset()")?;
    let (stand2, dir2, dir2_name) = boss_span(&mut rt)?;
    let free_run = charge_measure(
        &mut rt,
        dir2,
        4.0,
        (boss_charge_steps + 60.0) as usize,
        (boss_charge_steps + 40.0) as usize,
    )?;
    if (free_run.moved - full_moved).abs() > 0.05 {
        return Err(format!(
            "满程冲撞走了 {:.1} 单位 / {} 步（每步均摊 {:.4}），期望 {full_moved:.1} \
             （= charge_speed {boss_charge_speed:.0} × charge_steps {boss_charge_steps:.0} / 60Hz，\
             每步 {:.4}）",
            free_run.moved,
            free_run.steps,
            free_run.moved / free_run.steps.max(1) as f64,
            boss_charge_speed / HZ,
        )
        .into());
    }
    if free_run.steps as f64 != boss_charge_steps {
        return Err(format!(
            "满程冲撞走了 {} 步，期望 {boss_charge_steps:.0} 步",
            free_run.steps
        )
        .into());
    }
    if free_run.drift.abs() > 1e-6 {
        return Err(format!(
            "冲撞偏离了起手定下的直线（横向漂 {:.3} 单位）：方向该钉在注册那一刻",
            free_run.drift
        )
        .into());
    }
    println!(
        "  满程冲撞 = {} 步 / {:.1} 单位（沿{dir2_name}一条直线，横向漂移 {:.4}）；起点格({:.0},{:.0})",
        free_run.steps, free_run.moved, free_run.drift, stand2.0, stand2.1
    );
    if free_run.start_frame <= start_frame {
        return Err(format!(
            "第二次起冲的时刻（帧 {:.0}）不比第一次（{start_frame:.0}）晚：帧计数倒退了",
            free_run.start_frame
        )
        .into());
    }

    // 口径②：冲撞的节流只有招式冷却这一份——从上一次**起冲那一帧**算起，下一次正好晚 cooldown 步。
    let throttled = charge_start(&mut rt, dir2, 4.0, (charge_cooldown + 30.0) as usize)?;
    let gap = throttled.start_frame - free_run.start_frame;
    if (gap - charge_cooldown).abs() > 1e-9 {
        return Err(format!(
            "两次起冲间隔 {gap:.0} 步，而 ape_charge.cooldown = {charge_cooldown:.0}：\
             AI 里还藏了第二个计时器（或者冷却没生效）"
        )
        .into());
    }
    println!(
        "  口径②：两次起冲正好间隔 {gap:.0} 步 = data/skills.lua 那一条 cooldown（AI 没有第二个计时器）"
    );

    // 上界、寿命与"退场是把槽位还回去"：沿整条时间轴逐步采样。
    // 为什么要走满 `interval*2 + ttl` 这一截：召唤物的一生要有**三次状态变化**才说明白——
    // 召满（上界真的到得了）、寿命到集体退场（ttl 真的在扣）、空槽位重新召满（退场不是永久烧掉机会）。
    let mut max_live: f64 = 0.0;
    let mut max_total: f64 = 0.0;
    let mut saw_full = false;
    let mut drop_step = -1.0;
    let mut rise_step = -1.0;
    let sample_limit = (boss_interval * 2.0 + doc_ttl) as usize + 180;
    for step in 0..sample_limit {
        if step % 5 == 0 {
            anchor_boss_witness(&mut rt, dir2, 4.0)?;
        }
        frames(&mut rt, 1)?;
        let pop = probe_fields(&mut rt, BOSS_POPULATION)?;
        let live = pop[0];
        let total = pop[2];
        if live > doc_cap {
            return Err(format!(
                "第 {step} 步场上有 {live:.0} 只幼崽，越过上界 {doc_cap:.0}（= 组里的槽位数）"
            )
            .into());
        }
        if total > native_rows + doc_cap {
            return Err(format!(
                "第 {step} 步场上敌人总数 {total:.0}，越过 本图天然敌人 {native_rows:.0} + 上界 {doc_cap:.0}"
            )
            .into());
        }
        max_live = max_live.max(live);
        max_total = max_total.max(total);
        if live >= doc_cap {
            if drop_step >= 0.0 {
                rise_step = step as f64;
                break;
            }
            saw_full = true;
        } else if saw_full && drop_step < 0.0 && live == 0.0 {
            drop_step = step as f64;
        }
    }
    if drop_step < 0.0 || rise_step < 0.0 {
        return Err(format!(
            "采满 {sample_limit} 步没看到完整的\"召满→集体退场→再召满\"（saw_full={saw_full} \
             drop={drop_step:.0} rise={rise_step:.0}；ttl={doc_ttl:.0}、interval={boss_interval:.0}）：\
             寿命没在扣，或槽位没还回去"
        )
        .into());
    }
    println!(
        "  上界与寿命：采样 {sample_limit} 步内最多同时 {max_live:.0} 只（上界 {doc_cap:.0}）、\
         场上敌人总数最多 {max_total:.0}（天然 {native_rows:.0} + 上界）；第 {drop_step:.0} 步寿命到集体退场，\
         第 {rise_step:.0} 步又召满——槽位是还回去的，不是烧掉的"
    );

    // 不入档：槽位正在场上的时候快照里也不能有它们（读档回来就成了没有主人的孤儿实体）。
    assert_str(
        &mut rt,
        ENEMY_CAPTURE_SHAPE,
        &format!("rows={:.0}", enemy_rows - slot_rows),
    )?;
    println!(
        "  enemy.capture() 只有 {:.0} 行（= 全部 {enemy_rows:.0} 行 − 槽位 {slot_rows:.0} 行），召唤物不入档",
        enemy_rows - slot_rows
    );

    // 切图不残留：寒泉洞 → 演武场 → 寒泉洞，两次都要是零。
    open_scene(&mut rt, "combat_arena")?;
    frames(&mut rt, 2)?;
    let arena_pop = probe_fields(&mut rt, BOSS_POPULATION)?;
    if arena_pop[0] != 0.0 || arena_pop[1] != 0.0 {
        return Err(format!(
            "切到演武场之后召唤物还在（活 {:.0} 只、槽位占着 {:.0} 个）",
            arena_pop[0], arena_pop[1]
        )
        .into());
    }
    open_scene(&mut rt, &boss_map)?;
    frames(&mut rt, 2)?;
    let back_pop = probe_fields(&mut rt, BOSS_POPULATION)?;
    if back_pop[0] != 0.0 || back_pop[1] != 0.0 {
        return Err(format!(
            "切回{boss_map}当步就挂着上一局的幼崽（活 {:.0} 只、槽位 {:.0} 个）：残留",
            back_pop[0], back_pop[1]
        )
        .into());
    }
    assert_num(
        &mut rt,
        "require('systems.enemy').query('enemy_ape_boss').phase",
        2.0,
    )?;
    println!(
        "  切图（{boss_map}→演武场→{boss_map}）：召唤物清零、二阶段还在（阶段从 hp 推，切图/存读档都退不回一阶段）"
    );

    // Boss 倒下 = 它召出来的东西一起退场（小怪不能比主人活得久）。
    let mut reborn = 0usize;
    let live_now = loop {
        if reborn.is_multiple_of(5) {
            anchor_boss_witness(&mut rt, dir2, 4.0)?;
        }
        frames(&mut rt, 1)?;
        reborn += 1;
        let live = probe_fields(&mut rt, BOSS_POPULATION)?[0];
        if live >= 1.0 || reborn > (boss_interval + 60.0) as usize {
            break live;
        }
    };
    if live_now < 1.0 {
        return Err(format!(
            "切图回来等满 {reborn} 步 Boss 都没再召出一只：summon_interval={boss_interval:.0} 或槽位归还不成立"
        )
        .into());
    }
    let killed = query_str(
        &mut rt,
        "(function() local e = require('systems.enemy'); local q = e.query('enemy_ape_boss'); \
           return tostring(e.hit(entity.find_id('enemy_ape_boss'), q.hp)) end)()",
    )?;
    if killed != "0" {
        return Err(format!(
            "这一刀没把 Boss 打死（enemy.hit 返回 {killed:?}）：\"倒下即散\"观察不到"
        )
        .into());
    }
    let after_boss_death = probe_fields(&mut rt, BOSS_POPULATION)?;
    if after_boss_death[0] != 0.0 || after_boss_death[1] != 0.0 {
        return Err(format!(
            "Boss 倒下后场上还剩 {:.0} 只幼崽（槽位占着 {:.0} 个）：小怪比主人活得久",
            after_boss_death[0], after_boss_death[1]
        )
        .into());
    }
    assert_str(
        &mut rt,
        "require('systems.enemy').query('enemy_ape_boss').alive",
        "false",
    )?;
    assert_str(
        &mut rt,
        ENEMY_CAPTURE_SHAPE,
        &format!("rows={:.0}", enemy_rows - slot_rows),
    )?;
    println!(
        "  Boss 倒下即散：召着的 {live_now:.0} 只一起退场、槽位全部归还，快照仍是 {:.0} 行（死人入档、活小怪不入档）",
        enemy_rows - slot_rows
    );
    println!(
        "  （这一刀会走既有的击杀掉落通路：Boss 的铜钱/物品入账本门不复查，掉落表由 jhyt_m28_loop 钉）"
    );

    // 敌人侧数据负例：这六条改错了都不会响，只能靠加载期报错抓住。
    for (label, code, want) in ENEMY_NEGATIVE_CASES {
        let issues = query_str(&mut rt, code)?;
        if issues.is_empty() {
            return Err(format!(
                "{label}：data/enemies.lua 的 validate_all 没报出来（这条静默通过了）"
            )
            .into());
        }
        if !issues.contains(want) {
            return Err(format!("{label}：报错文案里没有 {want:?}，实际 {issues}").into());
        }
        println!("  {label} → {want}");
    }
    assert_str(
        &mut rt,
        &format!("tostring({enemy_def}.validate_all())"),
        "true",
    )?;
    println!(
        "  {} 条敌人数据负例跑完，data/enemies.lua 仍然全绿（每条都改了又改回去）",
        ENEMY_NEGATIVE_CASES.len()
    );

    // ---------------- ⑤‴ 玄冥会徒的站防（docs/33 §6.2：把"更远判定"的近似换成真 guard 分支） ----------------
    // 这一节先做文档↔数据的镜像（那条决定全是引用、没有新数字），再当场打两刀：
    // 举着盾的正面一刀 10 → 3，同一只敌人**没举盾**的一刀 10 → 10，中间夹一条"硬直/击退不吃减伤"。
    let xuanming = "enemy_xuanming";
    let xuanming_ref = format!("{enemy_def}.get('{xuanming}')");
    let guard_map = query_str(&mut rt, &format!("{xuanming_ref}.map"))?;
    let guard_name = query_str(&mut rt, &format!("{xuanming_ref}.name"))?;
    let guard_range = query_num(&mut rt, &format!("{xuanming_ref}.attack_range"))?;
    let guard_aggro = query_num(&mut rt, &format!("{xuanming_ref}.aggro_radius"))?;
    let guard_cd = query_num(&mut rt, &format!("{xuanming_ref}.attack_cooldown"))?;
    let guard_skill_data = query_str(&mut rt, &format!("{xuanming_ref}.guard_skill"))?;
    let guard_kind = query_str(
        &mut rt,
        &format!("require('data.skills').skills.{guard_skill_data}.kind"),
    )?;
    // 敌人那份 absorb / front_degrees 读的是**它举着的那条招式记录**；而 `absorb`/`guard_arc`
    // 两个变量来自 §6.1 玩家守心诀那一行（第④⑥节用的就是它们）。两组数字相等 =
    // "敌人侧不存在第二份挡多少"这件事在数据层成立，而不只是文档里的一句承诺。
    let guard_absorb_data = skill_num(&mut rt, &guard_skill_data, "absorb")?;
    let guard_arc_data = skill_num(&mut rt, &guard_skill_data, "front_degrees")?;
    let doc_guard_skill = doc_kv(&design, "guard_skill")?;
    let doc_absorb = doc_kv(&design, "absorb")?;

    banner(&format!(
        "M34-04b⑤‴ 站防：{guard_name}（{guard_map}）举的是 {guard_skill_data}（kind={guard_kind}），absorb={absorb} / 正面 {guard_arc:.0}° 与玩家守心诀同一份"
    ));
    if doc_guard_skill != guard_skill_data {
        return Err(format!(
            "docs/33 写的 guard_skill = {doc_guard_skill:?}，数据 = {guard_skill_data:?}"
        )
        .into());
    }
    if guard_kind != "guard" {
        return Err(format!(
            "{guard_skill_data}.kind = {guard_kind:?}：站防只能举 kind=guard 的招（那里才有 absorb 可减）"
        )
        .into());
    }
    let stance_pairs: [(&str, f64, f64); 4] = [
        ("absorb（减伤比例）", doc_number(&design, "absorb")?, absorb),
        (
            "front_degrees（正面张角）",
            doc_number(&design, "front_degrees")?,
            guard_arc,
        ),
        (
            "attack_range（贴进多远它才停下来出刀）",
            doc_number(&design, "attack_range")?,
            guard_range,
        ),
        (
            "attack_cooldown（空档来自这一格）",
            doc_number(&design, "attack_cooldown")?,
            guard_cd,
        ),
    ];
    for (label, doc_value, data_value) in &stance_pairs {
        if (doc_value - data_value).abs() > 1e-9 {
            return Err(format!("{label}：docs/33 = {doc_value}，数据 = {data_value}").into());
        }
        println!("  {label} = {data_value}");
    }
    for (label, player_side, enemy_side) in [
        ("absorb", absorb, guard_absorb_data),
        ("front_degrees", guard_arc, guard_arc_data),
    ] {
        if (player_side - enemy_side).abs() > 1e-9 {
            return Err(format!(
                "{label}：玩家守心诀 = {player_side}，敌人站防那条记录 = {enemy_side}——\
                 两边读的不是同一条 data/skills.lua，等于敌人侧藏着第二份数字"
            )
            .into());
        }
        println!("  {label}：玩家守心诀 == 敌人站防 == {enemy_side}（同一张招式记录）");
    }
    let expected_guarded = (HIT_DAMAGE * (1.0 - absorb)).floor().max(1.0);
    let want_absorbed = HIT_DAMAGE - expected_guarded;
    let doc_math = format!("floor({HIT_DAMAGE:.0} × (1 - {doc_absorb})) = {expected_guarded:.0}");
    if !design.contains(&doc_math) {
        return Err(format!(
            "docs/33 的算式与实现不同形：文中应当出现 {doc_math:?}（改 absorb 要连那一格一起改）"
        )
        .into());
    }
    println!(
        "  文档算式与代码同形：`{doc_math}`（10 点那一格 = 练习剑一刀，来源见 jhyt_m18_stats）"
    );

    open_scene(&mut rt, &guard_map)?;
    lua(&mut rt, "require('systems.combat').reset()")?;
    // 内力写成一个显眼的整数：站防期间它一格都不该动（§6.2 口径③的推论）。
    lua(&mut rt, "require('core.state').inner_power = 50")?;

    // 其一 正面 + 举盾：10 → 3。
    let mut stance_steps = 0usize;
    let readout = loop {
        anchor_east_of(&mut rt, xuanming, TILE)?;
        frames(&mut rt, 1)?;
        let line = strike_watched(&mut rt, xuanming, Some(&guard_skill_data), HIT_DAMAGE)?;
        if line.split('/').nth(1) == Some("true") {
            break line;
        }
        stance_steps += 1;
        if stance_steps > (guard_cd * 4.0) as usize {
            return Err(format!(
                "{stance_steps} 步都没能在它举着 {guard_skill_data} 的那一步出手（最后一次读数 {line}）：\
                 空档只有 attack_cooldown={guard_cd:.0} 步减去它自己那一刀的判定窗，抬起时机或姿态读数断了"
            )
            .into());
        }
    };
    let fields: Vec<&str> = readout.split('/').collect();
    if fields.len() != 10 {
        return Err(format!("站防出手读数的形状变了：{readout:?}（期望 10 段）").into());
    }
    if fields[0] != guard_skill_data || fields[9] != "guard" {
        return Err(format!(
            "举着盾的那一步读数：姿态 {}、state {}，期望 {guard_skill_data}/guard",
            fields[0], fields[9]
        )
        .into());
    }
    let (hp_before, dealt_before, inner_before, guard_hits_before, f0) = (
        fields[3]
            .parse::<f64>()
            .map_err(|error| format!("敌人气血读数 {r:?}：{error}", r = fields[3]))?,
        fields[4].parse::<f64>()?,
        fields[5].parse::<f64>()?,
        fields[6].parse::<f64>()?,
        fields[8].parse::<f64>()?,
    );
    if fields[7] != "nil" {
        return Err(format!(
            "举着 {guard_skill_data} 的那一刻 `enemy.capture()` 里带着姿态（{r}）：\
             盾进快照 = 读档后身上还挂着一面没人举的盾",
            r = fields[7]
        )
        .into());
    }
    println!(
        "  抬起时机（等 {stance_steps} 步就站进空档）：姿态 = {}、state = {}，快照里这一行没有 guarding 这一格（{}）——与硬直/无敌帧/中毒同一类瞬时状态",
        fields[0], fields[9], fields[7]
    );
    let Some(vitals) = wait_for_enemy_damage(&mut rt, xuanming, hp_before)? else {
        return Err(format!(
            "那一刀没打在举着盾的{guard_name}身上（30 步内气血没动，日志最后一条 = {}）：\
             判定窗没开窗，还是它走出了玩家的矩形",
            last_enemy_hit(&mut rt, &guard_name)?
        )
        .into());
    };
    let (hp_after, stun_front, knock_front, blocked_front) =
        (vitals[0], vitals[1], vitals[2], vitals[3]);
    let delta_front = hp_before - hp_after;
    if (delta_front - expected_guarded).abs() > 1e-6 {
        return Err(format!(
            "正面打在举盾的{guard_name}身上吃下 {delta_front:.0} 点，期望 {expected_guarded:.0}\
             （原始 {HIT_DAMAGE:.0} × (1 - {absorb}) 取整、兜 1 点）"
        )
        .into());
    }
    let want_log = format!("{expected_guarded:.0}/{want_absorbed:.0}/{hp_after:.0}");
    let log_front = last_enemy_hit(&mut rt, &guard_name)?;
    if log_front != want_log {
        return Err(format!(
            "举盾那一条 hit 日志 = {log_front}，期望 damage/absorbed/remaining = {want_log}"
        )
        .into());
    }
    let window = enemy_log_window(&mut rt, f0)?;
    let dealt_delta = window[2] - dealt_before;
    if (dealt_delta - window[1]).abs() > 1e-6 {
        return Err(format!(
            "`stats().dealt` 的差 = {dealt_delta:.0}，但日志里这几步打在敌人身上的伤害之和 = {:.0}：\
             dealt 记的不是落到血上的那一份",
            window[1]
        )
        .into());
    }
    if window[0] != 1.0 {
        return Err(format!(
            "这几步里有 {:.0} 条命中落在敌人身上（普攻判定窗 44×40 挤进了第二只）：\
             dealt 的账核不到单独这一刀，先把场地让开",
            window[0]
        )
        .into());
    }
    let inner_after = query_num(&mut rt, "require('core.state').inner_power")?;
    if (inner_after - inner_before).abs() > 1e-6 {
        return Err(format!(
            "敌人站防的减伤顺手扣了玩家内力（{inner_before:.0} → {inner_after:.0}）：\
             inner_cost/inner_period 只对 attacker_kind=player 生效（§6.2 口径③）"
        )
        .into());
    }
    let guard_hits_after = query_num(&mut rt, "require('systems.combat').stats().guard_hits")?;
    if (guard_hits_after - guard_hits_before).abs() > 1e-6 {
        return Err(format!(
            "玩家的 guard_hits 被敌人那面盾加了 {:.0} 次：guard_hits 记的是**玩家**正面格挡的命中",
            guard_hits_after - guard_hits_before
        )
        .into());
    }
    println!(
        "  举盾 + 正面：气血 {:.0} → {:.0}（吃下 {delta_front:.0}），日志 {log_front}；dealt 只记 {dealt_delta:.0}、guard_hits 与内力都不动（内力 {inner_before:.0} → {inner_after:.0}）",
        hp_before, hp_after
    );

    // 其二 对照：把 guard_skill 这一个键摘掉，**同一只敌人、同一套几何**就吃满 10 点。
    // 为什么不换一只不站防的敌人来打：它在 `attack_range` 内除了出刀那一步之外基本一直举着盾
    // （空档 = 整个 `attack_cooldown`），而玩家普攻的判定窗最远只到 range 44 + 身体半径 14 = 58
    // 单位，比它的 `attack_range`（56）只多 2 单位——想"打一个没举盾的玄冥会徒"就得先退出它的
    // 判定距离，而那同时也就退出了我们自己的判定窗。摘键对照把这两个变量都省了：几何一模一样，
    // 差的只有那一条数据，于是"减免来自 guard_skill 这条记录"与"state 也从它来"一次钉死。
    lua(&mut rt, &format!("{xuanming_ref}.guard_skill = nil"))?;
    let mut control_steps = 0usize;
    let control = loop {
        anchor_east_of(&mut rt, xuanming, TILE)?;
        frames(&mut rt, 1)?;
        let line = strike_watched(&mut rt, xuanming, None, HIT_DAMAGE)?;
        if line.split('/').nth(1) == Some("true") {
            break line;
        }
        control_steps += 1;
        if control_steps > (guard_cd * 4.0) as usize {
            return Err(format!(
                "{control_steps} 步都没能在摘了 guard_skill 的{guard_name}身上出手（最后一次读数 {line}）"
            )
            .into());
        }
    };
    let control_fields: Vec<&str> = control.split('/').collect();
    if control_fields.len() != 10 {
        return Err(format!("对照那一刀的读数形状变了：{control:?}（期望 10 段）").into());
    }
    let hp_open = control_fields[3].parse::<f64>()?;
    let dealt_open = control_fields[4].parse::<f64>()?;
    let f1 = control_fields[8].parse::<f64>()?;
    if hp_open <= HIT_DAMAGE {
        return Err(format!(
            "摘键对照没法跑：它只剩 {hp_open:.0} 点气血，吃不下 {HIT_DAMAGE:.0} 点的满值一刀——\
             死在结算之前会被读成\"没减伤\"，先把血量处理干净"
        )
        .into());
    }
    let Some(vitals) = wait_for_enemy_damage(&mut rt, xuanming, hp_open)? else {
        return Err(format!(
            "对照那一刀（姿态 {}）没打中：30 步内气血没动",
            control_fields[0]
        )
        .into());
    };
    // 摘键要一直摘到**这一刀结算完**为止：减免读的是命中那一刻的姿态，注册之后马上改回去的话，
    // 判定窗开的时候盾已经举回来了——那会把"摘键对照"跑成"又一次正面减伤"（本门第一版就栽在
    // 这里：注册那一步姿态确实是 false，落下来却是 3 点）。
    lua(
        &mut rt,
        &format!("{xuanming_ref}.guard_skill = '{guard_skill_data}'"),
    )?;
    let (hp_closed, stun_open, knock_open, blocked_open) =
        (vitals[0], vitals[1], vitals[2], vitals[3]);
    let delta_open = hp_open - hp_closed;
    if (delta_open - HIT_DAMAGE).abs() > 1e-6 {
        return Err(format!(
            "摘了 guard_skill 的{guard_name}吃下 {delta_open:.0} 点，期望满值 {HIT_DAMAGE:.0}：\
             减伤漏到了没有 guard 记录的目标身上"
        )
        .into());
    }
    let log_open = last_enemy_hit(&mut rt, &guard_name)?;
    let want_open_log = format!("{HIT_DAMAGE:.0}/0/{hp_closed:.0}");
    if log_open != want_open_log {
        return Err(format!(
            "摘键那一条 hit 日志 = {log_open}，期望 damage/absorbed/remaining = {want_open_log}"
        )
        .into());
    }
    let window = enemy_log_window(&mut rt, f1)?;
    if (window[2] - dealt_open - window[1]).abs() > 1e-6 || window[0] != 1.0 {
        return Err(format!(
            "对照那一刀的 dealt 对不上账：窗口内 {:.0} 条命中、伤害之和 {:.0}，dealt 差 {:.0}",
            window[0],
            window[1],
            window[2] - dealt_open
        )
        .into());
    }
    let stance_open = control_fields[0];
    if stance_open != "false" {
        return Err(
            format!("摘掉 guard_skill 之后姿态还读得到 {stance_open}：盾不是从数据来的").into(),
        );
    }
    if control_fields[9] != "attack" {
        return Err(format!(
            "摘掉 guard_skill 之后空档里 state = {}，期望 attack：\
             `enemy.lua` 那一句 `def.guard_skill and \"guard\" or \"attack\"` 不再从数据取招式了",
            control_fields[9]
        )
        .into());
    }
    println!(
        "  摘掉 guard_skill（同一只敌人、同一套几何）：姿态 {stance_open}、state={}，同一刀吃满 {:.0} 点，日志 {log_open}（等 {control_steps} 步）",
        control_fields[9], delta_open
    );
    assert_str(
        &mut rt,
        &format!("tostring({enemy_def}.validate_all())"),
        "true",
    )?;

    // 其三 硬直与击退不吃减伤：同一招、同一只敌人、同一套几何，只有摘没摘 guard_skill 不同。
    if (stun_front - stun_open).abs() > 1e-6 {
        return Err(format!(
            "减伤把硬直也减了：举盾那一刀之后还剩 {stun_front:.0} 步，摘键那刀之后还剩 {stun_open:.0} 步"
        )
        .into());
    }
    if blocked_front == blocked_open {
        if (knock_front - knock_open).abs() > 1e-6 {
            return Err(format!(
                "减伤把击退也减了：举盾那一刀推走 {knock_front:.1} 单位，摘键那刀推走 {knock_open:.1}"
            )
            .into());
        }
        println!(
            "  盾扛得住伤害，扛不住推力：两刀的硬直都是 {stun_front:.0} 步、击退都是 {knock_front:.1} 单位"
        );
    } else {
        println!(
            "  硬直两刀都是 {stun_front:.0} 步；击退这次没比（两次里有一次被墙挡住，`push_actor` 不穿墙 → 位移本来就是 0）"
        );
    }

    // 其四 落防：退出 `attack_range` 当步就举不起盾（姿态每步重算，不是"举起就一直挡着"）。
    let standoff = guard_range + TILE;
    if standoff >= guard_aggro {
        return Err(format!(
            "落防那一步要把玩家挪到 {standoff:.0} 单位外，已经超出 aggro_radius（{guard_aggro:.0}）：\
             state 会是 idle 而不是 chase，换个落点或改判据"
        )
        .into());
    }
    let waited = approach_until_guarding(
        &mut rt,
        xuanming,
        &guard_skill_data,
        (guard_cd * 4.0) as usize,
    )?;
    let state_up = query_str(
        &mut rt,
        &format!("require('systems.enemy').query('{xuanming}').state"),
    )?;
    anchor_east_of(&mut rt, xuanming, standoff)?;
    frames(&mut rt, 1)?;
    let stance_down = query_str(
        &mut rt,
        &format!("tostring(require('systems.enemy').query('{xuanming}').guarding)"),
    )?;
    let state_down = query_str(
        &mut rt,
        &format!("require('systems.enemy').query('{xuanming}').state"),
    )?;
    if stance_down != "false" || state_down != "chase" {
        return Err(format!(
            "退出 attack_range（{guard_range:.0}）一步之后：姿态 {stance_down}、state {state_down}，\
             期望 false/chase——盾还举着就是\"离开距离也在减伤\"，走回待机就是落防没落到 state 上"
        )
        .into());
    }
    println!(
        "  抬起→落防：等 {waited} 步站进空档（state={state_up}），挪到 {:.0} 单位外一步就落（姿态 {stance_down}、state={state_down}）",
        standoff
    );

    // ⑥ 以后的用例要用演武场的训练木人，先把场景切回去。
    open_scene(&mut rt, "combat_arena")?;

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
         dash_iframes={iframes:.0} clip={} front={front_expected:.0} back={after_flat:.0} \
         defense={defense:.0} meditate=+{meditate_gain:.0}/{inner_max:.0} \
         absorb={absorb} period={guard_period:.0} edge_log={edge_log} hud={} map_lines={} \
         boss={boss_name}@{boss_map} phase2={boss_below:.0}/{boss_hp:.0} \
         charge={:.1}px/{boss_charge_steps:.0}steps gap={gap:.0} \
         summon_cap={doc_cap:.0} summon_ttl={doc_ttl:.0} live_max={max_live:.0}/{max_total:.0} \
         drop={drop_step:.0} rise={rise_step:.0} \
         guard={xuanming}:{guard_skill_data} front=10->{expected_guarded:.0}/{want_absorbed:.0} \
         wait={stance_steps}/{control_steps}/{waited} enemy_neg={}",
        project.display(),
        clip_report.join("/"),
        hud_texts.join(","),
        lines,
        free_run.moved,
        ENEMY_NEGATIVE_CASES.len(),
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

// ---------- ⑤″ Boss 二阶段的驱动 ----------

/// `BOSS_SPAN` 认的四个主轴方向（探针返回的序号 1..4 对应这里）。
const SPAN_DIRS: [((f64, f64), &str); 4] = [
    ((1.0, 0.0), "东"),
    ((-1.0, 0.0), "西"),
    ((0.0, 1.0), "北"),
    ((0.0, -1.0), "南"),
];

/// 把 `a/b/c` 形态的一次性探针读数拆成数字（`t`/`f` 记 1/0）。
/// 合成一段读是为了采样循环里每步只跑一次 exec；形状不对就当失败，不猜。
fn probe_fields(
    runtime: &mut LuaRuntime,
    expr: &str,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let raw = query_str(runtime, expr)?;
    let mut out = Vec::new();
    for field in raw.split('/') {
        let value = match field {
            "t" => 1.0,
            "f" => 0.0,
            other => other
                .parse::<f64>()
                .map_err(|error| format!("{expr} → {raw:?}：{other:?} 不是数字（{error}）"))?,
        };
        out.push(value);
    }
    Ok(out)
}

/// `BOSS_READOUT`：`(是否在冲撞, 战斗帧, Boss 的 x, Boss 的 y)`。
fn charge_readout(
    runtime: &mut LuaRuntime,
) -> Result<(bool, f64, f64, f64), Box<dyn std::error::Error>> {
    let fields = probe_fields(runtime, BOSS_READOUT)?;
    if fields.len() != 4 {
        return Err(format!("冲撞读数的形状变了：{fields:?}（期望 4 段）").into());
    }
    Ok((fields[0] > 0.5, fields[1], fields[2], fields[3]))
}

/// Boss 当前所在格（`enemy.query` 走的是实体实时位置，不是数据里的 home）。
fn boss_cell(runtime: &mut LuaRuntime) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let x = query_num(
        runtime,
        "require('systems.enemy').query('enemy_ape_boss').cell_x",
    )?;
    let y = query_num(
        runtime,
        "require('systems.enemy').query('enemy_ape_boss').cell_y",
    )?;
    Ok((x, y))
}

/// `BOSS_SPAN` 的读数 → `（Boss 当前格, 开阔方向的单位向量, 方向名）`。
type BossSpan = ((f64, f64), (f64, f64), &'static str);

fn boss_span(runtime: &mut LuaRuntime) -> Result<BossSpan, Box<dyn std::error::Error>> {
    let raw = query_str(runtime, BOSS_SPAN)?;
    let fields: Vec<f64> = raw
        .split(',')
        .map(|part| {
            part.parse::<f64>()
                .map_err(|_| format!("开阔带没扫出来：{raw:?}（期望 x,y,方向序号）"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if fields.len() != 3 {
        return Err(format!("开阔带读数形状变了：{raw:?}").into());
    }
    let index = fields[2] as usize;
    let Some((dir, name)) = index
        .checked_sub(1)
        .and_then(|slot| SPAN_DIRS.get(slot))
        .map(|(dir, name)| (*dir, *name))
    else {
        return Err(format!("开阔带的方向序号越界：{raw:?}").into());
    };
    Ok(((fields[0], fields[1]), dir, name))
}

/// 冲撞的理论满程位移：`charge_speed`（世界单位/秒）× `charge_steps` / 60Hz 固定步。
fn boss_speed_distance(speed: f64, steps: f64) -> f64 {
    speed * steps / HZ
}

/// 把玩家钉在 Boss **实时位置**沿 `dir` 方向 `witness` 格处，并补满气血。
/// 为什么用实时位置而不是所在格格心：`start_charge` 把方向钉成"注册那一步指向玩家的
/// 单位向量"，而 Boss 的实际 y 从来不在格心上（差 15.9 单位）——按格心钉人会让冲撞斜着走，
/// "沿一条直线、位移 = 速度×步数"这两条断言量的就不是 `BOSS_SPAN` 扫出来的那排格子了。
/// 为什么要每步重钉：`witness` 格决定 AI 选哪一手——第 4 格（128 单位）既大于
/// `attack_range`（48，否则这一步就走去打人了）又小于 `aggro_radius`（300，否则它回待机）。
/// 气血也必须补：玩家一死全部敌人停手（`tick_actor` 的 is_player_dead 分支），
/// 那会把"没越过上界"读成一条假绿。
fn anchor_boss_witness(
    runtime: &mut LuaRuntime,
    dir: (f64, f64),
    witness: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let code = format!(
        "(function() local e = require('systems.enemy'); local id = entity.find_id('enemy_ape_boss'); \
           local q = e.query('enemy_ape_boss'); \
           if not q or not q.live or not id then return end; \
           local p = transform.get(id); \
           require('player.player').teleport(p.x + {} * {} * 32, p.y + {} * {} * 32); \
           local c = require('systems.combat'); c.set_player_hp(c.player_max_hp()) end)()",
        dir.0, witness, dir.1, witness
    );
    lua(runtime, &code)
}

/// 把某一格的 solid 标志改掉（`9277524` 为 WebGL 补的那条 API，本门用它现造一堵墙）。
fn set_cell_solid(
    runtime: &mut LuaRuntime,
    cell: (f64, f64),
    solid: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    lua(
        runtime,
        &format!(
            "tilemap.set_solid(entity.find('GroundMap'), {}, {}, {})",
            cell.0, cell.1, solid
        ),
    )
}

/// 某一格是不是 solid（越界按"不是墙"算，与 `enemy.lua` 的 `cell_solid` 同一口径）。
fn cell_is_solid(
    runtime: &mut LuaRuntime,
    cell: (f64, f64),
) -> Result<bool, Box<dyn std::error::Error>> {
    let raw = query_str(
        runtime,
        &format!(
            "(function() local ok, v = pcall(tilemap.is_solid, entity.find('GroundMap'), {}, {}); \
               return tostring(ok and v == true) end)()",
            cell.0, cell.1
        ),
    )?;
    match raw.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("solid 读数不是 true/false：{other:?}").into()),
    }
}

// ---------- ⑤‴ 站防的驱动 ----------

/// 把玩家钉在指定敌人**实时位置**东侧 `distance` 单位处，并补满气血（`anchor_boss_witness` 的
/// 普通敌版）。东侧一格（32 单位）同时管住两件事：距离落在 `attack_range` 内 → 它会停下来出刀，
/// 于是才有"两刀之间的空档"；而它每步把自己转向玩家 → 举盾那一刻朝向 = 东，我们从东边打回去
/// 必然落在正面张角里（绕背 docs/33 明说不断言：敌人每步转身，要卡的是它转身之前的那一步）。
/// 气血每步补满：玩家一死全部敌人停手（`tick_actor` 的 is_player_dead 分支），姿态也就不再被
/// 算出来——"没格挡"会被读成一条假绿。
fn anchor_east_of(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    distance: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let moved = query_str(
        runtime,
        &format!(
            "(function() local id = entity.find_id('{persist_id}'); \
               if not id then return 'gone' end; \
               local p = transform.get(id); \
               require('player.player').teleport(p.x + {distance}, p.y); \
               local c = require('systems.combat'); c.set_player_hp(c.player_max_hp()); return 'ok' end)()"
        ),
    )?;
    if moved != "ok" {
        return Err(format!("{persist_id} 不在这张图上（读数 {moved}）").into());
    }
    Ok(())
}

/// 一次 exec 里读完姿态、按条件注册玩家那一刀、并带回出手**之前**的账本：
/// `姿态/成没成/为什么/敌人气血/dealt/玩家内力/玩家 guard_hits/快照里的姿态/战斗帧/state`（10 段）。
/// 三组东西必须同一次 exec 拿：
/// ① 姿态与注册——空档只有 `attack_cooldown` 减去它自己那一刀的判定窗那么长，中间还夹着硬直，
///    先查姿态再单独注册会落在它换手出刀的那一步，"减伤没生效"其实是"没打在举盾的敌人身上"；
/// ② before 值与注册——出手之后再读气血/`dealt` 就已经把这一刀算进去了；
/// ③ 快照与姿态——`capture()` 要在**举着盾的那一刻**取，否则"瞬时状态不入档"什么也没观察。
/// `want_stance = None` 是不设姿态门槛（调用方自己保证此刻没有减免：摘掉 guard_skill 的对照），
/// 但**仍要求不在硬直中**——否则"吃满 10 点"的功劳可能落在硬直上而不是姿态上。
fn strike_watched(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    want_stance: Option<&str>,
    damage: f64,
) -> Result<String, Box<dyn std::error::Error>> {
    let gate = match want_stance {
        Some(stance) => format!(
            "if stance ~= '{stance}' then return stance .. '/false/stance/-/-/-/-/-/-/-' end; "
        ),
        None => "if q.guarding ~= false or (q.stun_frames or 0) > 0 then \
             return stance .. '/false/not_open/-/-/-/-/-/-/-' end; "
            .to_owned(),
    };
    query_str(
        runtime,
        &format!(
            "(function() local e = require('systems.enemy'); local c = require('systems.combat'); \
               local q = e.query('{persist_id}'); \
               if not q or not q.live then return 'not_live/-/-/-/-/-/-/-/-/-' end; \
               local stance = tostring(q.guarding); {gate} \
               local pl = require('player.player').ensure(); \
               if not pl then return stance .. '/false/no_player/-/-/-/-/-/-/-' end; \
               local snap = 'missing'; \
               for _, row in ipairs(e.capture().enemies) do \
                 if row.persist_id == '{persist_id}' then snap = tostring(row.guarding) end end; \
               local s = c.stats(); \
               local ok, reason = c.attack(pl, 'player', 'player', 'player', 'strike', 'left', {damage}); \
               return stance .. '/' .. tostring(ok) .. '/' .. tostring(reason) .. '/' .. tostring(q.hp) \
                 .. '/' .. tostring(s.dealt) .. '/' .. tostring(require('core.state').inner_power) .. '/' \
                 .. tostring(s.guard_hits) .. '/' .. snap .. '/' .. tostring(c.frame()) .. '/' \
                 .. tostring(q.state) end)()"
        ),
    )
}

/// 该敌人此刻的 `气血/剩余硬直/上次击退位移/是否被墙挡住`（四格一次读回，比四次 exec 稳）。
fn enemy_vitals(
    runtime: &mut LuaRuntime,
    persist_id: &str,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    probe_fields(
        runtime,
        &format!(
            "(function() local q = require('systems.enemy').query('{persist_id}'); \
               if not q or not q.live then return '-1/-1/-1/f' end; \
               return tostring(q.hp) .. '/' .. tostring(q.stun_frames) .. '/' .. \
                 tostring(q.last_knockback) .. '/' .. (q.knockback_blocked and 't' or 'f') end)()"
        ),
    )
}

/// 推进到该敌人气血真的掉下来（最多 30 步：strike 前摇 6 + 判定窗 12 的两倍不到）。
/// 每步重钉玩家：判定窗是按玩家**实时**位置算的矩形，而敌人被击退正在挪窝——不重钉的话
/// "没打到"其实是"人走开了、打偏了"。返回掉血那一刻的四格读数（`enemy_vitals`）。
fn wait_for_enemy_damage(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    before: f64,
) -> Result<Option<Vec<f64>>, Box<dyn std::error::Error>> {
    for _ in 0..30 {
        anchor_east_of(runtime, persist_id, TILE)?;
        frames(runtime, 1)?;
        let vitals = enemy_vitals(runtime, persist_id)?;
        if vitals[0] < before {
            return Ok(Some(vitals));
        }
    }
    Ok(None)
}

/// 只读姿态、不出手：推进到它**举着** `want` 那一招（落防那一步要先确认真的举起来了，
/// 否则"盾掉了"可能从头到尾就是"没举起来"）。返回等了多少步。
fn approach_until_guarding(
    runtime: &mut LuaRuntime,
    persist_id: &str,
    want: &str,
    limit: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut stance = String::new();
    for step in 0..limit {
        anchor_east_of(runtime, persist_id, TILE)?;
        frames(runtime, 1)?;
        stance = query_str(
            runtime,
            &format!("tostring(require('systems.enemy').query('{persist_id}').guarding)"),
        )?;
        if stance == want {
            return Ok(step);
        }
    }
    Err(format!(
        "{limit} 步里它一次都没举着 {want}（最后一次姿态 {stance}）：抬起时机读的是 attack_cooldown，\
         举不起来就说明空档那一支没走到"
    )
    .into())
}

/// 打在某个敌人身上的最后一条 hit 日志：`伤害/被盾吃掉的部分/剩余气血`。
/// 按显示名过滤（日志记的是 `name_of(target_id)`，不是 persist_id），只看 `event == 'hit'`
/// 所以击杀那条（`kill`）不会冒充这里的读数。
fn last_enemy_hit(
    runtime: &mut LuaRuntime,
    target_name: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    query_str(
        runtime,
        &format!(
            "(function() local last; \
               for _, entry in ipairs(require('systems.combat').log()) do \
                 if entry.event == 'hit' and entry.target == '{target_name}' then last = entry end end; \
               if not last then return 'none' end; \
               return tostring(last.damage) .. '/' .. tostring(last.absorbed) .. '/' .. \
                 tostring(last.remaining) end)()"
        ),
    )
}

/// `frame > f0` 之后**打在敌人身上**的条目：`条数/伤害之和/dealt 现值`。
/// 为什么不直接把 `stats().dealt` 的差当"这一刀的 3 点"：玩家普攻的判定窗是 44×40 的矩形，
/// 同一招可以同时打中挤在窗里的第二只敌人（`resolve_hits` 按目标去重、不按招式去重），
/// 那时 `dealt` 的差本来就是两个目标之和——它没错，是场地没管住。所以这里先跟日志对账，
/// 再要求"这几步只有一条命中"，两条同时成立才算核到了单独这一刀。
fn enemy_log_window(
    runtime: &mut LuaRuntime,
    f0: f64,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    probe_fields(
        runtime,
        &format!(
            "(function() local n, sum = 0, 0; \
               for _, entry in ipairs(require('systems.combat').log()) do \
                 if (entry.event == 'hit' or entry.event == 'kill') \
                   and entry.target ~= 'player' and entry.frame > {f0} then \
                   n = n + 1; sum = sum + (entry.damage or 0) end end; \
               return tostring(n) .. '/' .. tostring(sum) .. '/' .. \
                 tostring(require('systems.combat').stats().dealt) end)()"
        ),
    )
}

/// 每只幼崽的实际落格 vs「主人起冲那一步的格子 + 数据里的 `summon_offset`」；
/// 偏移格是 solid 时期望退回主人脚下那一格（`summon_slots` 的兜底，与击退/轻功同一口径）。
/// 全部对上返回 `ok`，否则带回每一行的差异。
fn summon_slot_cells(
    runtime: &mut LuaRuntime,
    stand: (f64, f64),
) -> Result<String, Box<dyn std::error::Error>> {
    let expr = format!(
        "(function() local e = require('systems.enemy'); local d = require('data.enemies'); \
           local g = entity.find('GroundMap'); local bad = {{}}; \
           for _, row in ipairs(d.list()) do \
             if row.summon_owner == 'enemy_ape_boss' then \
               local q = e.query(row.persist_id); \
               if not q or not q.live then bad[#bad + 1] = row.persist_id .. '=不在场' \
               else \
                 local tx, ty = {sx} + row.summon_offset.x, {sy} + row.summon_offset.y; \
                 local ok_solid, solid = pcall(tilemap.is_solid, g, tx, ty); \
                 if ok_solid and solid == true then tx, ty = {sx}, {sy} end; \
                 if q.cell_x ~= tx or q.cell_y ~= ty then \
                   bad[#bad + 1] = row.persist_id .. '=在 ' .. tostring(q.cell_x) .. ',' .. tostring(q.cell_y) \
                     .. '，该落在 ' .. tostring(tx) .. ',' .. tostring(ty) \
                 end \
               end \
             end \
           end; \
           if #bad == 0 then return 'ok' end \
           return table.concat(bad, ' | ') end)()",
        sx = stand.0,
        sy = stand.1
    );
    query_str(runtime, &expr)
}

/// 一次冲撞的实测结果。
struct ChargeRun {
    /// 起冲那一帧的 `combat.frame()`（节流按这一格算，所以两次结果的差就是冷却）。
    start_frame: f64,
    /// 沿冲撞方向的净位移（世界单位）。
    moved: f64,
    /// 垂直于冲撞方向的漂移：应当恒为 0（方向钉在注册那一刻，不跟着玩家挪）。
    drift: f64,
    /// 实际走到的步数（撞墙提前结束 < `charge_steps`）。
    steps: usize,
    x0: f64,
    y0: f64,
}

impl ChargeRun {
    /// 从"已经起冲的那一步"的读数建一条记录（那一步不位移，所以读数就是起点）。
    fn at(start_frame: f64, x0: f64, y0: f64) -> Self {
        Self {
            start_frame,
            moved: 0.0,
            drift: 0.0,
            steps: 0,
            x0,
            y0,
        }
    }
}

/// 推进到 Boss **起冲那一帧**（每步把玩家钉在 witness 格）。注册那一步不位移，
/// 所以返回的读数就是这条冲撞的起点。
fn charge_start(
    runtime: &mut LuaRuntime,
    dir: (f64, f64),
    witness: f64,
    wait_limit: usize,
) -> Result<ChargeRun, Box<dyn std::error::Error>> {
    for _ in 0..wait_limit {
        anchor_boss_witness(runtime, dir, witness)?;
        frames(runtime, 1)?;
        let (charging, frame, x, y) = charge_readout(runtime)?;
        if charging {
            return Ok(ChargeRun::at(frame, x, y));
        }
    }
    Err(
        format!("等满 {wait_limit} 步 Boss 都没起冲：阶段/冷却/距离三者之一不对（冲撞的观察全断）")
            .into(),
    )
}

/// 从"已经起冲"推进到它熄灭，把位移/步数写回 `run`。
fn charge_finish(
    runtime: &mut LuaRuntime,
    dir: (f64, f64),
    witness: f64,
    run: &mut ChargeRun,
    move_limit: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..move_limit {
        anchor_boss_witness(runtime, dir, witness)?;
        frames(runtime, 1)?;
        run.steps += 1;
        let (charging, _, x, y) = charge_readout(runtime)?;
        if !charging {
            let dx = x - run.x0;
            let dy = y - run.y0;
            run.moved = dx * dir.0 + dy * dir.1;
            run.drift = dx * dir.1 - dy * dir.0;
            return Ok(());
        }
    }
    Err(
        format!("冲撞走满 {move_limit} 步还没结束（charge_steps 与 solid 判定至少一个坏了）")
            .into(),
    )
}

/// 完整量一次冲撞：等它起冲 → 推到结束。
fn charge_measure(
    runtime: &mut LuaRuntime,
    dir: (f64, f64),
    witness: f64,
    wait_limit: usize,
    move_limit: usize,
) -> Result<ChargeRun, Box<dyn std::error::Error>> {
    let mut run = charge_start(runtime, dir, witness, wait_limit)?;
    charge_finish(runtime, dir, witness, &mut run, move_limit)?;
    Ok(run)
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
    numbers_in(cell).get(1).copied()
}

/// 单元格里的**第一个**数字（"恢复 5 点内力" → 5）。`number_from_cell` 只认开头就是
/// 数字的格子，而 §3 的产出列是中文在前——那一列要用这个。
fn first_number(cell: &str) -> Option<f64> {
    numbers_in(cell).first().copied()
}

/// 单元格里按出现顺序排好的全部数字。
fn numbers_in(cell: &str) -> Vec<f64> {
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
    seen
}

/// 正文里以 `` `key = value` `` 形态写着的值（§6.2 的站防段就是这么写的）。
/// 为什么这一段不走 `markdown_table`：那条决定**不引入新数字**，全部引用既有记录，
/// 所以它在文档里是一段话；写成表反而诱导后来人往里加"新数字列"。
/// 读不到就报错、不兜默认值：文档换了说法（把 `absorb = 0.7` 改写成"减伤 70%"）应该让本门红，
/// 而不是悄悄丢掉这条镜像。值取到第一个空格 / 反引号 / 度数符为止，引号剥掉。
fn doc_kv(markdown: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let needle = format!("`{key} = ");
    for line in markdown.lines() {
        let Some((_, rest)) = line.split_once(&needle) else {
            continue;
        };
        let token = rest
            .split(['`', ' ', '°', '，', ')', '）'])
            .next()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| format!("docs/33 里 {needle}… 后面读不到值"))?;
        return Ok(token.trim_matches('"').to_owned());
    }
    Err(format!("docs/33 里没有 {needle}… 这一格").into())
}

/// `doc_kv` 的读数转数字（转不动就是文档那一格被改成了文字，让本门红）。
fn doc_number(markdown: &str, key: &str) -> Result<f64, Box<dyn std::error::Error>> {
    let token = doc_kv(markdown, key)?;
    token
        .parse::<f64>()
        .map_err(|error| format!("docs/33 的 `{key} = {token}` 不是数字：{error}").into())
}

/// 按第一列取一行，再读第 `column`（0 起）列里的第一个数字。
fn doc_row_number(
    rows: &[Vec<String>],
    first_cell: &str,
    column: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let row = rows
        .iter()
        .find(|row| row.first().map(String::as_str) == Some(first_cell))
        .ok_or(format!("docs/33 的表里没有「{first_cell}」那一行"))?;
    let cell = row
        .get(column)
        .ok_or(format!("「{first_cell}」行缺第 {} 列：{row:?}", column + 1))?;
    first_number(cell)
        .ok_or(format!("「{first_cell}」第 {} 列没有数字：{cell:?}", column + 1).into())
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
