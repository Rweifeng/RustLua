-- quest.lua：任务状态机（M17-03）。
--
-- 五态（验收项③）：
--   not_started  未接取（无记录）
--   in_progress  已接取，目标未全达成
--   deliverable  已接取，目标全达成（派生态，不落档）
--   completed    已交付且奖励已发（幂等锚点）
--   failed       显式失败
--
-- 数据定义见 data/quests/*.lua；新增任务只改数据，不改本文件与 Rust 核心
-- （验收项①）。引用错误一律在 validate_all 的加载期暴露，不静默失败（硬风险①）。
local state = require("state")
local npcs = require("data.npcs")
local items = require("data.items")
local tools = require("data.tools")

local M = {}

local VALID_STATUS = { in_progress = true, completed = true, failed = true }
local VALID_OBJECTIVE = { item = true, var = true, flag = true }
local VALID_REWARD = { item = true, money = true, inner_power = true }

-- 运行时记录：quest_id → { status = "in_progress" | "completed" | "failed" }
M.records = {}

-- 定义缓存：id → 定义表；加载失败缓存 false，避免反复 require 失败。
local defs = {}

local function definition(id)
  if type(id) ~= "string" then
    return nil
  end
  if defs[id] == nil then
    local ok, mod = pcall(require, "data.quests." .. id)
    defs[id] = (ok and type(mod) == "table") and mod or false
  end
  return defs[id] or nil
end

local function item_exists(id)
  return type(id) == "string" and (items[id] ~= nil or tools[id] ~= nil)
end

local function positive(value)
  local n = tonumber(value)
  return n ~= nil and n > 0
end

function M.get(id)
  return definition(id)
end

-- 注册表（data/quests/index.lua）顺序遍历；非字符串项跳过，由 validate_all 报告。
function M.ids()
  local ok, list = pcall(require, "data.quests.index")
  if not ok or type(list) ~= "table" then
    return {}
  end
  local out = {}
  for i = 1, #list do
    if type(list[i]) == "string" then
      out[#out + 1] = list[i]
    end
  end
  return out
end

-- 单个目标的 当前值, 目标值。
local function objective_progress(objective)
  local kind = objective.kind
  if kind == "item" then
    local target = tonumber(objective.count) or 1
    if not objective.item then
      return 0, target
    end
    return inv.count(state.inventory, objective.item), target
  elseif kind == "var" then
    return tonumber(state.get_var(objective.var)) or 0, tonumber(objective.count) or 1
  elseif kind == "flag" then
    local value = state.get_var(objective.var)
    return (value ~= nil and value ~= false) and 1 or 0, 1
  end
  return 0, 1
end

function M.objectives_met(id)
  local def = definition(id)
  if not def or type(def.objectives) ~= "table" then
    return false
  end
  for _, objective in ipairs(def.objectives) do
    local current, target = objective_progress(objective)
    if current < target then
      return false
    end
  end
  return true
end

-- 五态查询；未知任务返回 nil。
function M.state(id)
  if not definition(id) then
    return nil
  end
  local record = M.records[id]
  if not record then
    return "not_started"
  end
  if record.status == "completed" then
    return "completed"
  end
  if record.status == "failed" then
    return "failed"
  end
  if M.objectives_met(id) then
    return "deliverable"
  end
  return "in_progress"
end

function M.accept(id)
  if not definition(id) then
    return false, "unknown_quest"
  end
  local record = M.records[id]
  if record then
    return false, "already_" .. tostring(record.status)
  end
  M.records[id] = { status = "in_progress" }
  return true
end

function M.fail(id)
  if not definition(id) then
    return false, "unknown_quest"
  end
  local record = M.records[id]
  if not record then
    return false, "not_accepted"
  end
  if record.status == "completed" then
    return false, "already_completed"
  end
  if record.status == "failed" then
    return false, "already_failed"
  end
  record.status = "failed"
  return true
end

local function consume_objectives(def)
  for _, objective in ipairs(def.objectives or {}) do
    local need = tonumber(objective.count) or 1
    if objective.kind == "item" and objective.consume and objective.item and need > 0 then
      inv.remove(state.inventory, objective.item, need)
    end
  end
end

-- 奖励发放：item 用 count，money/inner_power 用 amount（与数据 schema 一致）。
local function grant_rewards(def)
  for _, reward in ipairs(def.rewards or {}) do
    if reward.kind == "item" then
      local count = tonumber(reward.count)
      if reward.item and count and count > 0 then
        inv.add(state.inventory, reward.item, count)
      end
    else
      local amount = tonumber(reward.amount)
      if amount then
        if reward.kind == "money" then
          state.money = state.money + amount
        elseif reward.kind == "inner_power" then
          state.inner_power = state.inner_power + amount
        end
      end
    end
  end
end

-- 副作用解释器：对话选项与任务 on_complete 共用（硬风险③：来源 + 事件入流水）。
function M.apply_effects(effects, source, event)
  if type(effects) ~= "table" then
    return true
  end
  if type(effects.set_var) == "table" then
    for name, value in pairs(effects.set_var) do
      state.set_var(name, value, source, event)
    end
  end
  if type(effects.add_var) == "table" then
    for name, delta in pairs(effects.add_var) do
      state.add_var(name, delta, source, event)
    end
  end
  if type(effects.add_item) == "table" then
    for _, entry in ipairs(effects.add_item) do
      local count = tonumber(entry.count) or 0
      if entry.item and count > 0 then
        inv.add(state.inventory, entry.item, count)
      end
    end
  end
  local money = tonumber(effects.money)
  if money then
    state.money = state.money + money
  end
  local inner_power = tonumber(effects.inner_power)
  if inner_power then
    state.inner_power = state.inner_power + inner_power
  end
  if type(effects.accept_quest) == "string" then
    M.accept(effects.accept_quest)
  end
  if type(effects.deliver_quest) == "string" then
    M.deliver(effects.deliver_quest)
  end
  if type(effects.fail_quest) == "string" then
    M.fail(effects.fail_quest)
  end
  return true
end

-- 交付：目标校验 → 落定完成态 → 消耗 → 发奖 → on_complete。
-- 完成态先落定，重复交付在入口即被拦截，奖励绝不重复发放（硬风险②）。
function M.deliver(id)
  local def = definition(id)
  if not def then
    return false, "unknown_quest"
  end
  local record = M.records[id]
  if not record then
    return false, "not_accepted"
  end
  if record.status == "completed" then
    return false, "already_completed"
  end
  if record.status == "failed" then
    return false, "quest_failed"
  end
  if not M.objectives_met(id) then
    return false, "objectives_unmet"
  end
  record.status = "completed"
  consume_objectives(def)
  grant_rewards(def)
  M.apply_effects(def.on_complete, "quest:" .. id, "on_complete")
  return true
end

-- 进度快照：{ {id, label, current, target, done}, ... }（UI 与调试用）。
function M.progress(id)
  local out = {}
  local def = definition(id)
  if not def then
    return out
  end
  for _, objective in ipairs(def.objectives or {}) do
    local current, target = objective_progress(objective)
    out[#out + 1] = {
      id = objective.id,
      label = objective.label or objective.id,
      current = current,
      target = target,
      done = current >= target
    }
  end
  return out
end

-- 进行中 / 可交付的任务 id（任务栏用）。
function M.active()
  local out = {}
  for _, id in ipairs(M.ids()) do
    local status = M.state(id)
    if status == "in_progress" or status == "deliverable" then
      out[#out + 1] = id
    end
  end
  return out
end

function M.capture()
  local records = {}
  for id, record in pairs(M.records) do
    records[id] = { status = record.status }
  end
  return { records = records }
end

function M.restore(data)
  M.records = {}
  if type(data) ~= "table" or type(data.records) ~= "table" then
    return
  end
  for id, record in pairs(data.records) do
    if type(id) == "string" and type(record) == "table" and VALID_STATUS[record.status] then
      M.records[id] = { status = record.status }
    end
  end
end

function M.reset()
  M.records = {}
end

-- 加载期校验：返回 ok, issues（issues 为字符串列表，空表即通过）。
function M.validate_all()
  local issues = {}
  local seen = {}
  for _, id in ipairs(M.ids()) do
    if seen[id] then
      issues[#issues + 1] = "任务注册表重复 id：" .. id
    end
    seen[id] = true

    local def = definition(id)
    if not def then
      issues[#issues + 1] = "任务定义缺失或加载失败：data.quests." .. id
    else
      if def.id ~= id then
        issues[#issues + 1] = "任务 " .. id .. " 的 id 字段不匹配：" .. tostring(def.id)
      end
      if type(def.name) ~= "string" or def.name == "" then
        issues[#issues + 1] = "任务 " .. id .. " 缺少 name"
      end
      if type(def.giver) ~= "string" or not npcs.get(def.giver) then
        issues[#issues + 1] = "任务 " .. id .. " 的 giver 引用未知 NPC：" .. tostring(def.giver)
      end

      if type(def.objectives) ~= "table" or #def.objectives == 0 then
        issues[#issues + 1] = "任务 " .. id .. " 缺少 objectives"
      else
        local objective_ids = {}
        for _, objective in ipairs(def.objectives) do
          local where = "任务 " .. id .. " 目标 " .. tostring(objective.id)
          if objective.id == nil then
            issues[#issues + 1] = "任务 " .. id .. " 存在缺少 id 的目标"
          elseif objective_ids[objective.id] then
            issues[#issues + 1] = "任务 " .. id .. " 目标 id 重复：" .. tostring(objective.id)
          end
          objective_ids[objective.id] = true

          if not VALID_OBJECTIVE[objective.kind] then
            issues[#issues + 1] = where .. " 的 kind 非法：" .. tostring(objective.kind)
          elseif objective.kind == "item" then
            if not item_exists(objective.item) then
              issues[#issues + 1] = where .. " 引用未知物品：" .. tostring(objective.item)
            end
            if not positive(objective.count) then
              issues[#issues + 1] = where .. " 的 count 必须为正数"
            end
          else
            if type(objective.var) ~= "string" or objective.var == "" then
              issues[#issues + 1] = where .. " 缺少 var"
            end
            if objective.kind == "var" and not positive(objective.count) then
              issues[#issues + 1] = where .. " 的 count 必须为正数"
            end
          end
        end
      end

      for _, reward in ipairs(def.rewards or {}) do
        if not VALID_REWARD[reward.kind] then
          issues[#issues + 1] = "任务 " .. id .. " 奖励 kind 非法：" .. tostring(reward.kind)
        elseif reward.kind == "item" then
          if not item_exists(reward.item) then
            issues[#issues + 1] = "任务 " .. id .. " 奖励引用未知物品：" .. tostring(reward.item)
          end
          if not positive(reward.count) then
            issues[#issues + 1] = "任务 " .. id .. " 奖励 count 必须为正数"
          end
        elseif not tonumber(reward.amount) then
          issues[#issues + 1] = "任务 " .. id .. " 奖励缺少 amount"
        end
      end

      if def.on_complete ~= nil and type(def.on_complete) ~= "table" then
        issues[#issues + 1] = "任务 " .. id .. " 的 on_complete 必须是表"
      elseif type(def.on_complete) == "table" then
        for key, value in pairs(def.on_complete) do
          if key ~= "set_var" and key ~= "add_var" then
            issues[#issues + 1] = "任务 " .. id .. " 的 on_complete 含非法字段：" .. tostring(key)
          elseif type(value) ~= "table" then
            issues[#issues + 1] = "任务 " .. id .. " 的 on_complete." .. tostring(key) .. " 必须是表"
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
