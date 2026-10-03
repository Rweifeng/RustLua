-- data/combat.lua：战斗受击反馈的**全局**调参（M25 / 计划 G2）。
--
-- 分工（避免同一个数字有两个来源）：
--   * **攻击方**的效果（本次攻击把目标推多远、打断多久）写在招式记录里：
--     `data/skills.lua` 的 `knockback`（世界单位）/`hitstun`（固定步数）。
--   * **受击方**的通用反应写在本文件：玩家的无敌帧、敌人的默认抵抗、
--     以及"命中之后还在走的东西"（瞬时状态 `M.status`：中毒的步长与每跳伤害）。
--     敌人可以在 `data/enemies.lua` 里按个体覆盖抵抗（例如训练木人是钉在地上的
--     木桩，完全不吃击退与硬直——这既是手感也是"练功房别把人推走"），
--     也可以按个体声明"打中会给目标挂哪个状态种类"（引用本文件 `M.status` 的键）。
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

-- 瞬时状态（M34-03）：按**固定步**结算的状态效果的全局定义。
--
-- 数值口径唯一来源是 `docs/33-江湖有田-数值设计案.md` §10 状态效果表——
-- 那里写的是"中毒 | 毒蝎、毒草误用 | 每 2 秒损失 3 生命 | 10 秒"。本仓的计时单位是
-- 60Hz 固定步（`game_app.rs` 的 `FixedTimeStep::new(60, 8)`），换算因此写死在这里：
-- 2 秒 = 120 步一跳、10 秒 = 600 步总计。秒 ↔ 步的对应由 `jhyt_m34_status` 解析
-- 那张表逐格比对，改了这边不改那边就是门红（与 §6.3 的受击反馈表同一口径）。
--
-- 字段（都必须是整数，见 validate_all）：
--   steps      状态持续多少步
--   tick_steps 每隔多少步结算一次（必须 <= steps，否则整段持续时间一跳都走不到）
--   damage     每次结算扣多少点气血（必须 >= 1：0 伤害的状态是条假状态）
--   feedback   挂上状态时写进 `state.last_action` 的键（中文文案在 `ui/hud.lua` 的
--              ACTION_TEXT 里；由 jhyt_m34_status 断言"这个键真有中文映射"，
--              否则 HUD 会把英文键名直接甩给玩家）
M.status = {
  poison = { steps = 600, tick_steps = 120, damage = 3, feedback = "poisoned" }
}

-- 状态条目的合法字段（写了别的键 = 这个键没有消费方，直接判错）。
M.status_keys = { steps = true, tick_steps = true, damage = true, feedback = true }

function M.status_entry(kind)
  if type(kind) ~= "string" then
    return nil
  end
  return M.status[kind]
end

-- 已定义的状态种类（排序后返回，供校验报错文案与 headless 断言用）。
function M.status_kinds()
  local out = {}
  for kind in pairs(M.status) do
    out[#out + 1] = kind
  end
  table.sort(out)
  return out
end

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
  -- 瞬时状态表（M34-03）：整张表缺席 = 毒蝎那句"带中毒"的注释重新变成假话，
  -- 所以至少要有中毒这一条；每一步长都必须是整数（步数是唯一计时单位，小数没有意义）。
  if type(M.status) ~= "table" or next(M.status) == nil then
    issues[#issues + 1] = "data/combat.lua:status: expected a non-empty table of status kinds"
  else
    for kind, entry in pairs(M.status) do
      local path = "data/combat.lua:status." .. tostring(kind)
      if type(kind) ~= "string" or kind == "" then
        issues[#issues + 1] = "data/combat.lua:status: kind key must be a non-empty string"
      elseif type(entry) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        for field, value in pairs(entry) do
          if not M.status_keys[field] then
            issues[#issues + 1] = path .. "." .. tostring(field) .. ": unknown effect key"
          elseif field == "feedback" then
            if type(value) ~= "string" or value == "" then
              issues[#issues + 1] = path .. ".feedback: expected non-empty string (HUD 反馈键)"
            end
          elseif not is_non_negative_number(value) or math.floor(value) ~= value then
            issues[#issues + 1] = path .. "." .. tostring(field) .. ": expected non-negative integer (fixed steps)"
          end
        end
        for _, field in ipairs({ "steps", "tick_steps", "damage" }) do
          if not is_non_negative_number(entry[field]) or math.floor(entry[field]) ~= entry[field] then
            issues[#issues + 1] = path .. "." .. field .. ": expected non-negative integer (fixed steps)"
          end
        end
        -- 一跳都走不到的状态是"看不见的假状态"：图标会亮、伤害永远不结算。
        if entry.tick_steps and entry.steps and entry.tick_steps > entry.steps then
          issues[#issues + 1] = path .. ".tick_steps: " .. tostring(entry.tick_steps)
            .. " > steps " .. tostring(entry.steps) .. "（整段持续时间一次都跳不到）"
        end
        if entry.damage and entry.damage < 1 then
          issues[#issues + 1] = path .. ".damage: expected >= 1（0 伤害的状态没有消费方）"
        end
        -- 缺 feedback 键 = 挂上状态时玩家看不到任何反馈（图标亮着，HUD 一行字没有）。
        if type(entry.feedback) ~= "string" or entry.feedback == "" then
          issues[#issues + 1] = path .. ".feedback: required non-empty string"
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
