-- data/skills.lua：《江湖有田》武学/普攻注册表（M18-01 纯数据）。
--
-- 新增一个招式 = 本文件加一条记录，无需改动 skills.lua / combat.lua /
-- Rust 核心（与 data/npcs.lua 同约定）。
--
-- 字段：
--   name        显示名（战斗日志/HUD）
--   kind        "basic"（普攻：伤害随武器加成）| "martial"（武学：固定伤害+内力消耗）
--   damage      基础伤害（basic = 徒手基础值；martial = 固定伤害）
--   uses_weapon basic=true 时最终伤害 = damage + 已装备武器 damage（见 skills.lua）
--   inner_cost  武学消耗内力（basic 为 0）
--   cooldown    冷却固定步数（引擎 60Hz 固定步，逻辑不依赖真实时间）
--   windup      前摇步数：攻击注册后延迟多少步开始判定
--   active      判定窗步数：窗口内每步扫描 hitbox，同一攻击对同一目标只结算一次
--   range       hitbox 前伸长度（世界单位）
--   width       hitbox 宽度（垂直于朝向，世界单位）
--   hitstun     命中后目标的硬直步数（固定步；0 = 不打断）
--   knockback   命中后目标被推开的距离（世界单位，沿"攻击者 → 目标"方向；0 = 不推）
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

local VALID_KINDS = { basic = true, martial = true }

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
      if type(skill.name) ~= "string" or skill.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if not VALID_KINDS[skill.kind] then
        issues[#issues + 1] = path .. ".kind: expected basic|martial, got " .. tostring(skill.kind)
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
      for _, field in ipairs({ "cooldown", "windup", "active" }) do
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
    end
  end
  return #issues == 0, issues
end

return M
