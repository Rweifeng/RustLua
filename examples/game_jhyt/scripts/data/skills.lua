-- data/skills.lua：《江湖有田》武学/普攻注册表（M18-01 纯数据）。
--
-- 新增一个招式 = 本文件加一条记录，无需改动 skills.lua / combat.lua /
-- Rust 核心（与 data/npcs.lua 同约定）。
--
-- 字段：
--   name        显示名（战斗日志/HUD）
--   kind        "basic"（普攻：伤害随武器加成）| "martial"（武学：固定伤害+内力消耗）
--               | "movement"（轻功：位移，不出伤害）| "guard"（格挡：按住即持续减伤）
--   damage      基础伤害（basic = 徒手基础值；martial = 固定伤害；movement/guard 恒为 0）
--   uses_weapon basic=true 时最终伤害 = damage + 已装备武器 damage（见 skills.lua）
--   inner_cost  武学消耗内力（basic 为 0）；kind=guard 时是**每期**的维持消耗
--   cooldown    冷却固定步数（引擎 60Hz 固定步，逻辑不依赖真实时间）；
--               kind=guard 恒为 0（docs/33 §6.1 那格的口径是「持续」，不是冷却）
--   windup      前摇步数：攻击注册后延迟多少步开始判定
--   active      判定窗步数：窗口内每步扫描 hitbox，同一攻击对同一目标只结算一次
--   range       hitbox 前伸长度（世界单位）
--   width       hitbox 宽度（垂直于朝向，世界单位）
--   hitstun     命中后目标的硬直步数（固定步；0 = 不打断）
--   knockback   命中后目标被推开的距离（世界单位，沿"攻击者 → 目标"方向；0 = 不推）
--
-- kind=movement 的**专属**字段（docs/33 §6.1「追风步：位移 3 格，无敌 0.25 秒」）：
--   dash_tiles        一次轻功位移几格（1 格 = 32 世界单位，与 grid/tilemap 同一约定）
--   invincible_steps  位移结束后还保留多少步无敌（走 combat 既有的无敌帧通道，
--                     不另立计时器；docs/33 §10「轻功无敌 | 不受伤害和硬直」）
--   —— 这一类**不出判定窗**，所以 windup/active/range/width/hitstun/knockback 必须缺省
--   （写了就是没有消费方的假数字，validate_all 直接判错）。
--
-- kind=guard 的**专属**字段（docs/33 §6.1「守心诀：10/秒、持续、正面减伤 70%」）：
--   inner_period    每多少步收一次 inner_cost（60 步 = 1 秒 ↔ 文档的「10/秒」）
--   absorb          正面命中的减伤比例 [0,1)（0.7 = 只吃 30%）
--   front_degrees   正面扇形的圆心角 (0,360]：以玩家朝向为轴，攻击来源落在扇形内才算"正面"
--   —— 同样不出判定窗；格挡不禁止移动与出手（docs 没写这条限制，本仓就不发明它）。
--
-- 受击反馈的全局部分（玩家无敌帧、敌人默认抵抗）在 `data/combat.lua`；
-- 本文件只描述"这一招打出去造成什么"。
--
-- 表顺序无关；skills.lua 按 id 读取。**唯一例外是 `M.binding.cast`**（文件下方的
-- 玩家按键绑定）：那一项的顺序就是 HUD 招式卡与触屏命中测试的顺序，不是任意序。
local M = {}

M.version = 1

M.skills = {
  strike = {
    name = "普攻",
    kind = "basic",
    uses_weapon = true,
    damage = 2,
    inner_cost = 0,
    cooldown = 24,
    windup = 6,
    active = 12,
    range = 44,
    width = 40,
    hitstun = 10,
    knockback = 8
  },
  palm_gust = {
    name = "千风掌",
    kind = "martial",
    uses_weapon = false,
    damage = 6,
    inner_cost = 10,
    cooldown = 60,
    windup = 8,
    active = 12,
    range = 56,
    width = 48,
    hitstun = 20,
    knockback = 64
  },
  -- 弦月斩：横向扇形剑光；范围与宽度双扩，适合围攻；随武器加成。
  crescent_slash = {
    name = "弦月斩",
    kind = "martial",
    uses_weapon = false,
    damage = 8,
    inner_cost = 15,
    cooldown = 90,
    windup = 10,
    active = 14,
    range = 48,
    width = 96,
    hitstun = 16,
    knockback = 32
  },
  -- 铁砂掌：单体重击；伤害高、判定窗窄、击退远；用来破防或打断精英。
  iron_palm = {
    name = "铁砂掌",
    kind = "martial",
    uses_weapon = false,
    damage = 14,
    inner_cost = 25,
    cooldown = 120,
    windup = 14,
    active = 8,
    range = 40,
    width = 36,
    hitstun = 34,
    knockback = 96
  },
  -- 豹跃：短突进 + 命中击倒；冷却长、前摇小，用于突入或脱困。
  leopard_step = {
    name = "豹跃",
    kind = "martial",
    uses_weapon = false,
    damage = 5,
    inner_cost = 12,
    cooldown = 72,
    windup = 4,
    active = 10,
    range = 72,
    width = 32,
    hitstun = 24,
    knockback = 48
  },
  -- 追风步（docs/33 §6.1 的轻功行，M34-04 起从"未实现白名单"里删掉）：
  -- 位移 3 格 = 96 世界单位（沿当前朝向，被墙挡住就走到最后一格可走位），
  -- 无敌 15 步 = 0.25 秒，走 combat 既有的无敌帧通道（与受击后的 48 步同一计数器）。
  -- 没有 windup/active/range/width：这一招不出判定窗。
  chase_wind = {
    name = "追风步",
    kind = "movement",
    uses_weapon = false,
    damage = 0,
    inner_cost = 15,
    cooldown = 120,
    dash_tiles = 3,
    invincible_steps = 15
  },
  -- 山魈冲撞（M34-04b ⑤，Boss 二阶段）：前摇长（12 步 = 0.2 秒给玩家看出起手）、
  -- 判定窗长（48 步 = 冲撞全程都开着窗），宽 48 —— 那正是"一头撞过来"的形状。
  -- **为什么必须 kind=martial**：`enemy.lua` 的冲撞起步时把这一招注册进 `combat.attack`，
  -- 然后逐格搬动 Boss 的实体；`combat.resolve_hits` 每步按**攻击者当时的位置**重算 hitbox，
  -- 于是判定窗真的随位移扫过整条路径。kind=movement/guard 不出判定窗（底座直接拒收），
  -- 那样的"冲撞"只是一段位移，撞不疼任何人。
  -- 伤害不在这里：敌人的伤害源是 `data/enemies.lua` 的 damage 字段（docs/33 §6.2 那一格），
  -- 招式只提供判定窗形状与受击反馈（硬直/击退）。这一格的 8 与 Boss 那行的 `damage = 8`
  -- 是同一个数字的两个写法，`jhyt_m34_skills` ⑤″ 断言两者相等——想改"冲撞比平A更疼"要先把
  -- 敌方伤害通路扩一条，而不是在这里悄悄改一个没消费方的数。
  ape_charge = {
    name = "山魈冲撞",
    kind = "martial",
    uses_weapon = false,
    damage = 8,
    inner_cost = 0,
    cooldown = 180,
    windup = 12,
    active = 48,
    range = 40,
    width = 48,
    hitstun = 24,
    knockback = 80
  },
  -- 守心诀（docs/33 §6.1 的格挡行）：按住即进入格挡，每 60 步（= 1 秒）扣 10 点内力
  -- ——那正是文档那格的「10/秒」；内力不足会自动落防。正面 120° 内的命中只吃 30% 伤害，
  -- 背后挨打一分不减。冷却恒 0：文档那格写的是"持续"，不是冷却。
  guard_heart = {
    name = "守心诀",
    kind = "guard",
    uses_weapon = false,
    damage = 0,
    inner_cost = 10,
    cooldown = 0,
    inner_period = 60,
    absorb = 0.7,
    front_degrees = 120
  }
}

function M.get(id)
  if type(id) ~= "string" then
    return nil
  end
  return M.skills[id]
end

function M.ids()
  local list = {}
  for id, _ in pairs(M.skills) do
    list[#list + 1] = id
  end
  table.sort(list)
  return list
end

-- ---------- 玩家按键绑定：招式 ↔ 命名动作的唯一事实源 ----------
--
-- 三个消费方都读这里，不再各抄一份：
--   * `ui/input.lua`：玩法动作分派（按下哪条动作出哪一招）。
--   * `ui/hud.lua`：招式快捷栏 —— **这里的顺序就是卡片顺序**。
--   * `ui/virtual_pad.lua`：触屏命中测试（点第 i 张卡 = 注入第 i 项动作绑定的物理键）。
--
-- 为什么要在数据层立这张表：这三处原先各写各的。快捷栏按 `M.ids()` 的**字典序**排卡，
-- 于是「普攻」落到第 8 张、而敌人专属的「山魈冲撞」（ape_charge）也上了玩家的栏；
-- 触屏想表达"第 3 张 = 弦月斩"根本没有可信依据（字典序第 3 是弦月斩纯属巧合）。
-- 顺序进了数据之后，卡片序 = 动作序 = 命中测试序是**同一条事实**，加一招只改这一张表。
--
-- 未登记的招式 = 玩家打不出（`ape_charge` 正是：它是 Boss 的冲撞，没有玩家动作）。
M.binding = {
  -- 出手类（按下即出招，走 `combat.player_attack`）。顺序 = 快捷栏卡序。
  cast = {
    { action = "attack", skill = "strike" },
    { action = "skill", skill = "palm_gust" },
    { action = "skill_2", skill = "crescent_slash" },
    { action = "skill_3", skill = "iron_palm" },
    { action = "skill_4", skill = "leopard_step" },
  },
  -- 轻功：按下即位移，走 `combat.player_dash`（没有判定窗，所以不跟 cast 混一轨）。
  dash = { action = "dash", skill = "chase_wind" },
  -- 格挡：**按住**才站得住（判据是按住态而不是按下边沿），因此单独一条、放栏尾。
  guard = { action = "guard", skill = "guard_heart" },
}

--- 玩家能打出的招式序（快捷栏卡序 = 触屏卡序）：`cast` 之后接 `dash`、`guard`。
--- 返回的是新表（每次调用重新拼），调用方可以安全地遍历。
function M.player_slots()
  local out = {}
  for _, entry in ipairs(M.binding.cast) do
    out[#out + 1] = entry
  end
  out[#out + 1] = M.binding.dash
  out[#out + 1] = M.binding.guard
  return out
end

--- 招式 id → 命名动作（敌人专属/未登记的招式返回 nil：触屏据此知道这张卡不该上榜）。
function M.action_of(id)
  for _, entry in ipairs(M.player_slots()) do
    if entry.skill == id then
      return entry.action
    end
  end
  return nil
end

local VALID_KINDS = { basic = true, martial = true, movement = true, guard = true }

-- 招式记录里合法的键（写了别的键 = 这个键没有任何消费方，直接判错；与
-- `data/combat.lua` 的 status_keys 同一条不变量）。
M.skill_keys = {
  name = true, kind = true, damage = true, uses_weapon = true, inner_cost = true,
  cooldown = true, windup = true, active = true, range = true, width = true,
  hitstun = true, knockback = true,
  -- movement（轻功）专属
  dash_tiles = true, invincible_steps = true,
  -- guard（格挡）专属
  inner_period = true, absorb = true, front_degrees = true
}

-- 只有"打出去一击"的 kind（basic/martial）才有的判定窗字段。movement/guard 写了就是
-- 假数字：`combat.lua` 对这两类根本不注册攻击窗口（追风步只位移、守心诀只减伤），
-- 于是 range/active 谁都不读——宁可直接判错，也不留一个"改了没反应"的旋钮。
local HITBOX_FIELDS = { "windup", "active", "range", "width", "hitstun", "knockback" }

--- 按 kind 判断这条招式是否出判定窗（`combat.lua` 的分派也走这一个判据）。
function M.uses_hitbox(skill)
  return skill ~= nil and (skill.kind == "basic" or skill.kind == "martial")
end

-- 三类槽位各自允许的 kind：投错的槽位等于"这条动作根本不会出那一下"，
-- 而它在画面上只是"按了没反应"，没有任何报错 —— 所以只有数据门拦得住。
local SLOT_KINDS = {
  cast = "basic|martial",
  dash = "movement",
  guard = "guard",
}
local SLOT_ALLOWED = {
  cast = { basic = true, martial = true },
  dash = { movement = true },
  guard = { guard = true },
}

-- 校验一条 `{ action, skill }` 绑定（`M.binding` 的每个槽位走同一段）。
local function check_slot(issues, slot, entry, path, used_actions, used_skills)
  if type(entry) ~= "table" then
    issues[#issues + 1] = path .. ": expected object { action, skill }"
    return
  end
  local action = entry.action
  if type(action) ~= "string" or action == "" then
    issues[#issues + 1] = path .. ".action: required non-empty action name"
  elseif used_actions[action] then
    issues[#issues + 1] = path .. ".action: 动作 '" .. action
      .. "' 已被 " .. used_actions[action] .. " 占用（一键两招会让卡序与动作序脱钩）"
  else
    used_actions[action] = path
  end
  local def = M.skills[entry.skill]
  if type(def) ~= "table" then
    issues[#issues + 1] = path .. ".skill: unknown skill id " .. tostring(entry.skill)
  elseif not SLOT_ALLOWED[slot][def.kind] then
    issues[#issues + 1] = path .. ".skill: " .. tostring(entry.skill) .. " 是 kind="
      .. tostring(def.kind) .. "，不能进 " .. slot .. " 槽（该槽只收 " .. SLOT_KINDS[slot] .. "）"
  end
  if type(entry.skill) ~= "string" or entry.skill == "" then
    issues[#issues + 1] = path .. ".skill: required non-empty skill id"
  elseif used_skills[entry.skill] then
    issues[#issues + 1] = path .. ".skill: 招式 '" .. entry.skill
      .. "' 绑了两个动作（" .. used_skills[entry.skill] .. " 与本条），快捷栏会出现两张同一招"
  else
    used_skills[entry.skill] = path
  end
end

local function validate_binding(issues)
  if type(M.binding) ~= "table" then
    issues[#issues + 1] = "data/skills.lua:binding: expected object"
    return
  end
  local used_actions, used_skills = {}, {}
  if type(M.binding.cast) ~= "table" or #M.binding.cast == 0 then
    issues[#issues + 1] = "data/skills.lua:binding.cast: expected non-empty array"
  else
    for index, entry in ipairs(M.binding.cast) do
      check_slot(issues, "cast", entry,
        "data/skills.lua:binding.cast[" .. tostring(index) .. "]", used_actions, used_skills)
    end
  end
  for _, slot in ipairs({ "dash", "guard" }) do
    check_slot(issues, slot, M.binding[slot],
      "data/skills.lua:binding." .. slot, used_actions, used_skills)
  end
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/skills.lua: version: unsupported version " .. tostring(M.version)
  end
  for id, skill in pairs(M.skills) do
    local path = "data/skills.lua:skills." .. tostring(id)
    if type(skill) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      for field in pairs(skill) do
        if not M.skill_keys[field] then
          issues[#issues + 1] = path .. "." .. tostring(field) .. ": unknown skill key"
        end
      end
      if type(skill.name) ~= "string" or skill.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if not VALID_KINDS[skill.kind] then
        issues[#issues + 1] = path .. ".kind: expected basic|martial|movement|guard, got "
          .. tostring(skill.kind)
      end
      if type(skill.damage) ~= "number" or skill.damage < 0 then
        issues[#issues + 1] = path .. ".damage: expected non-negative number"
      end
      if skill.kind == "basic" and skill.uses_weapon ~= true then
        issues[#issues + 1] = path .. ".uses_weapon: basic skill must use weapon bonus"
      end
      local inner = tonumber(skill.inner_cost) or 0
      if inner < 0 then
        issues[#issues + 1] = path .. ".inner_cost: expected non-negative number"
      end
      -- 冷却对四类都必填（连格挡也不例外：0 = 可以一直按，正合文档的「持续」）。
      local cooldown = tonumber(skill.cooldown)
      if not cooldown or cooldown < 0 or math.floor(cooldown) ~= cooldown then
        issues[#issues + 1] = path .. ".cooldown: expected non-negative integer (fixed steps)"
      end
      if M.uses_hitbox(skill) then
        for _, field in ipairs({ "windup", "active" }) do
          local value = tonumber(skill[field])
          if not value or value < 0 or math.floor(value) ~= value then
            issues[#issues + 1] = path .. "." .. field .. ": expected non-negative integer (fixed steps)"
          end
        end
        if (tonumber(skill.active) or 0) <= 0 then
          issues[#issues + 1] = path .. ".active: expected positive window"
        end
        for _, field in ipairs({ "range", "width" }) do
          if type(skill[field]) ~= "number" or skill[field] <= 0 then
            issues[#issues + 1] = path .. "." .. field .. ": expected positive number"
          end
        end
        -- 受击反馈（M25）：两个字段都必填——缺字段会让"这一招有没有击退/硬直"
        -- 变成隐式默认值，正是 `crop.yield` 那类静默兜底的翻版。
        local hitstun = skill.hitstun
        if type(hitstun) ~= "number" or hitstun < 0 or math.floor(hitstun) ~= hitstun then
          issues[#issues + 1] = path .. ".hitstun: expected non-negative integer (fixed steps)"
        end
        local knockback = skill.knockback
        if type(knockback) ~= "number" or knockback < 0 then
          issues[#issues + 1] = path .. ".knockback: expected non-negative number (world units)"
        end
      else
        -- movement / guard：判定窗字段必须**缺省**（上面刚说过它们没有消费方）。
        for _, field in ipairs(HITBOX_FIELDS) do
          if skill[field] ~= nil then
            issues[#issues + 1] = path .. "." .. field
              .. ": kind=" .. tostring(skill.kind) .. " 不出判定窗，这个数字没有消费方"
          end
        end
        if (tonumber(skill.damage) or 0) ~= 0 then
          issues[#issues + 1] = path .. ".damage: kind=" .. tostring(skill.kind)
            .. " 不打伤害，应为 0"
        end
        if skill.uses_weapon == true then
          issues[#issues + 1] = path .. ".uses_weapon: kind=" .. tostring(skill.kind)
            .. " 不结算武器加成"
        end
        if skill.kind == "movement" then
          local tiles = tonumber(skill.dash_tiles)
          if not tiles or tiles < 1 or math.floor(tiles) ~= tiles then
            issues[#issues + 1] = path .. ".dash_tiles: expected positive integer (grid cells)"
          end
          local steps = tonumber(skill.invincible_steps)
          if not steps or steps < 1 or math.floor(steps) ~= steps then
            issues[#issues + 1] = path .. ".invincible_steps: expected positive integer (fixed steps)"
          end
        elseif skill.kind == "guard" then
          local period = tonumber(skill.inner_period)
          if not period or period < 1 or math.floor(period) ~= period then
            issues[#issues + 1] = path .. ".inner_period: expected positive integer (fixed steps per charge)"
          end
          local absorb = tonumber(skill.absorb)
          if not absorb or absorb < 0 or absorb >= 1 then
            issues[#issues + 1] = path .. ".absorb: expected number in [0, 1)（1 = 完全免伤，等于把攻击系统关掉）"
          end
          local arc = tonumber(skill.front_degrees)
          if not arc or arc <= 0 or arc > 360 then
            issues[#issues + 1] = path .. ".front_degrees: expected number in (0, 360]"
          end
          -- 格挡是"按住就一直在"的姿态，冷却必须是 0：给了冷却就等于悄悄把它
          -- 改成另一个招式，而文档那一格写的是「持续」。
          if (tonumber(skill.cooldown) or 0) ~= 0 then
            issues[#issues + 1] = path .. ".cooldown: kind=guard 的口径是「持续」，冷却应为 0"
          end
        end
      end
    end
  end
  validate_binding(issues)
  return #issues == 0, issues
end

return M
