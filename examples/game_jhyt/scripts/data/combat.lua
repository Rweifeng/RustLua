-- data/combat.lua：战斗受击反馈的**全局**调参（M25 / 计划 G2）。
--
-- 分工（避免同一个数字有两个来源）：
--   * **攻击方**的效果（本次攻击把目标推多远、打断多久）写在招式记录里：
--     `data/skills.lua` 的 `knockback`（世界单位）/`hitstun`（固定步数）。
--   * **受击方**的通用反应写在本文件：玩家的无敌帧、敌人的默认抵抗。
--     敌人可以在 `data/enemies.lua` 里按个体覆盖抵抗（例如训练木人是钉在地上的
--     木桩，完全不吃击退与硬直——这既是手感也是"练功房别把人推走"）。
--
-- 数值口径：`examples/game_jhyt/docs/33-江湖有田-数值设计案.md` §6.3（受击反馈），
-- 由 headless 样例 `jhyt_m25_combat_feedback` 解析该表并与本文件逐格比对。
--
-- 为什么需要无敌帧：狼的攻击间隔（`attack_cooldown` 72 步）在 60Hz 下是 1.2 秒，
-- 但敌人被击退/多只敌人同时贴身时，没有无敌帧就会出现"一次贴身连续掉血、
-- 玩家完全无法操作"的死亡螺旋。无敌帧只挡伤害与硬直，不挡位移与输入以外的系统。
local M = {}

M.version = 1

-- 玩家受击后的无敌帧（固定步数，60Hz 下 48 步 = 0.8 秒）。
-- 无敌期间：不再掉血、不再刷新硬直与击退；战斗日志记一条 dodge 事件便于追踪。
M.player = {
  iframes = 48
}

-- 敌人默认抵抗（0 = 完全吃效果，1 = 完全免疫）。可按敌人在 data/enemies.lua 覆盖。
M.enemy = {
  knockback_resist = 0,
  hitstun_resist = 0
}

function M.player_iframes()
  return math.max(0, math.floor(tonumber(M.player.iframes) or 0))
end

function M.enemy_knockback_resist()
  local value = tonumber(M.enemy.knockback_resist) or 0
  return math.max(0, math.min(1, value))
end

function M.enemy_hitstun_resist()
  local value = tonumber(M.enemy.hitstun_resist) or 0
  return math.max(0, math.min(1, value))
end

local function is_non_negative_number(value)
  return type(value) == "number" and value >= 0 and value == value
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/combat.lua: version: unsupported version " .. tostring(M.version)
  end
  if type(M.player) ~= "table" then
    issues[#issues + 1] = "data/combat.lua:player: expected object"
  else
    local iframes = M.player.iframes
    if not is_non_negative_number(iframes) or math.floor(iframes) ~= iframes then
      issues[#issues + 1] = "data/combat.lua:player.iframes: expected non-negative integer (fixed steps)"
    end
  end
  if type(M.enemy) ~= "table" then
    issues[#issues + 1] = "data/combat.lua:enemy: expected object"
  else
    for _, field in ipairs({ "knockback_resist", "hitstun_resist" }) do
      local value = M.enemy[field]
      if not is_non_negative_number(value) or value > 1 then
        issues[#issues + 1] = "data/combat.lua:enemy." .. field .. ": expected number in [0, 1]"
      end
    end
  end
  return #issues == 0, issues
end

return M
