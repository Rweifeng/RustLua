local state = require("state")
local player = require("player")
local farm = require("farm")

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

function M.prompt(candidate, facing)
  local ok, reason = M.can_interact(candidate, facing)
  if candidate and candidate.target and candidate.target.prompt then
    return candidate.target.prompt(candidate, ok and nil or reason)
  end
  return ok and "E: 交互" or reason
end

function M.interact(facing)
  local candidates = M.query(facing)
  local candidate = candidates[1]
  if not candidate then
    local pid = player.ensure()
    local reason = pid and "no_target" or "no_player"
    state.last_action = reason
    return reason, nil
  end
  local ok, reason = M.can_interact(candidate, facing)
  if not ok then
    state.last_action = reason
    return reason, candidate
  end
  local result = candidate.target.interact(candidate, facing or state.facing or "down")
  return result, candidate
end

function M.clear()
  targets = {}
  next_order = 0
end

return M
