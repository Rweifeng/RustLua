local state = require("core.state")
local player = require("player.player")
local farm = require("systems.farm")

local M = {}
local targets = {}
local next_order = 0

local function register_target(target)
  assert(type(target) == "table", "interaction target must be a table")
  assert(type(target.id) == "string", "interaction target id is required")
  assert(type(target.query) == "function", "interaction target query is required")
  assert(type(target.interact) == "function", "interaction target interact is required")
  next_order = next_order + 1
  target.priority = tonumber(target.priority) or 0
  target._order = next_order
  targets[target.id] = target
  return target.id
end

function M.register(target)
  return register_target(target)
end

function M.unregister(id)
  targets[id] = nil
end

local function ensure_builtins()
  if targets.farm then
    return
  end
  register_target({
    id = "farm",
    priority = 0,
    query = function(facing)
      return farm.query(facing)
    end,
    can_interact = function(candidate, facing)
      local ok, reason = farm.can_interact(facing)
      return ok, reason
    end,
    prompt = function(candidate, reason)
      if reason then
        return reason
      end
      return "E: 农田"
    end,
    interact = function(candidate, facing)
      return farm.act(facing)
    end
  })
  -- M19-01：野外采集目标（priority 5，介于农田 0 与设施 10 之间）。
  -- 缺文件/加载失败时静默跳过，不影响其它交互（跨工程健壮性）。
  local ok, gather = pcall(require, "systems.gather")
  if ok and type(gather) == "table" and type(gather.ensure_target) == "function" then
    gather.ensure_target()
  end
end

local function candidate_order(a, b)
  local ad = tonumber(a.distance) or math.huge
  local bd = tonumber(b.distance) or math.huge
  if ad ~= bd then
    return ad < bd
  end
  local af = tonumber(a.facing) or 0
  local bf = tonumber(b.facing) or 0
  if af ~= bf then
    return af > bf
  end
  if a.target.priority ~= b.target.priority then
    return a.target.priority > b.target.priority
  end
  return a.target._order < b.target._order
end

function M.query(facing)
  ensure_builtins()
  local candidates = {}
  for _, target in pairs(targets) do
    local candidate, reason = target.query(facing or state.facing or "down")
    if candidate then
      candidate.target = target
      candidate.reason = candidate.reason or reason
      table.insert(candidates, candidate)
    end
  end
  table.sort(candidates, candidate_order)
  return candidates
end

function M.can_interact(candidate, facing)
  if not candidate or not candidate.target then
    return false, "no_target"
  end
  if candidate.reason then
    return false, candidate.reason
  end
  if candidate.target.can_interact then
    return candidate.target.can_interact(candidate, facing or state.facing or "down")
  end
  return true
end

-- M19-01：候选重排（MVP 垂直切片实测缺陷修复）。
-- 旧实现只看候选 #1：玩家站在农田旁与 NPC 说话时，farm 目标（priority 0）以
-- distance=1 排在第一，interact 直接落在它身上（act 返回 need_hoe / not_farmland），
-- NPC 对话永远打不开；而 farm.can_interact 只判断"面前是不是农田"，
-- 看不出工具/体力这类只有 act 才知道的结果。
--
-- 现在的两段式排序（只在候选内部生效，query() 的原始顺序不变）：
--   1) 先按 priority 降序：剧情交互（NPC 对话 10）优先于生产劳动（农田 0），
--      "面向 NPC 按 E 就是交谈"符合玩家预期；
--   2) 同级再按原有顺序（距离 → 朝向 → 优先级 → 注册顺序）。
-- 不可交互的候选排到最后，只作为"全部失败"时的原因来源。
-- 失败原因（not_farmland / tired / no_target …）与调用方约定保持不变。
local function ordered_candidates(facing)
  local candidates = M.query(facing)
  local usable, blocked = {}, {}
  for _, candidate in ipairs(candidates) do
    local ok = M.can_interact(candidate, facing)
    if ok then
      usable[#usable + 1] = candidate
    else
      blocked[#blocked + 1] = candidate
    end
  end
  table.sort(usable, function(a, b)
    return (a.target.priority or 0) > (b.target.priority or 0)
  end)
  for _, candidate in ipairs(blocked) do
    usable[#usable + 1] = candidate
  end
  return usable
end

function M.interact(facing)
  local candidates = ordered_candidates(facing)
  if #candidates == 0 then
    local pid = player.ensure()
    local reason = pid and "no_target" or "no_player"
    state.last_action = reason
    return reason, nil
  end
  local candidate = candidates[1]
  local ok, reason = M.can_interact(candidate, facing)
  if not ok then
    state.last_action = reason
    return reason, candidate
  end
  local result = candidate.target.interact(candidate, facing or state.facing or "down")
  return result, candidate
end

-- 提示文案同理：取第一个可交互目标；全不可交互时回退到首个候选的原因。
function M.prompt(facing)
  local candidates = ordered_candidates(facing)
  local candidate = candidates[1]
  if not candidate then
    return "no_target"
  end
  local ok, reason = M.can_interact(candidate, facing)
  if candidate.target.prompt then
    return candidate.target.prompt(candidate, ok and nil or reason)
  end
  return ok and "E: 交互" or reason
end

function M.clear()
  targets = {}
  next_order = 0
end

return M
