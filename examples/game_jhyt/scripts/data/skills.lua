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
    width = 40
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
    width = 48
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
    end
  end
  return #issues == 0, issues
end

return M
