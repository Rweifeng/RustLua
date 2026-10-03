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
-- 表顺序无关；skills.lua 按 id 读取。
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
  return #issues == 0, issues
end

return M
