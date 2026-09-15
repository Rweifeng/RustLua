-- goals.lua：《江湖有田》七日目标运行时（M27 / 计划 G1）。
--
-- 职责：把「目标表」（data/goals.lua，纯数据）与「玩家实际做了什么」对上：
--   事件 → 推进计数 → 跨天结算 → 发奖 → HUD 一行提示。
--
-- 数据与逻辑的分工：
--   * data/goals.lua 决定"有哪些天、每天几条目标、条件是什么、奖励多少"；
--   * 本模块只负责匹配、计数、结算、发奖，**不复制任何数值**。
--
-- 事件来源（与 tutorial.lua 同一套接线，不新增引擎能力）：
--   * input.lua（Playing 态每帧）→ M.on_action(state.last_action)
--   * main.on_day_start      → M.on_day_start()      ← 日结算挂在这里
--   * main.enter_playable_map→ M.on_enter(map_id)
--   * main.new_game          → M.reset()
--
-- 持久化：全部用 state.vars 的**标量**（state 只接受 number/boolean/string），
-- 随既有 state provider 存档，无需新增 provider：
--   goals_day            当前目标日（1..7；7 天走完变 8 = 目标线结束）
--   goals_p_<goal_id>    事件类目标的进度计数
--   goals_day_<n>        第 n 天结算结果："completed" / "missed"
--   goals_day_<n>_reward 第 n 天奖励是否已发（幂等锚点，跨存档去重）
--   goals_milestone      七日里程碑状态："completed"
--   goals_milestone_reward 里程碑奖励是否已发
--
-- 幂等与兼容：
--   * 计数按 goal id 分桶（id 全局唯一，data/goals.lua 校验），跨天不会串。
--   * 奖励只在"当天全清"时发一次，锚点变量保证重复结算/重复读档不重复发。
--   * 老档（没有 goals_* 变量）：day 从 1 起算，不追溯历史，也不报错。
local state = require("core.state")
-- `log` 是引擎提供的全局（与 tutorial.lua 一致），没有 core.log 模块。

local M = {}

local DAY_VAR = "goals_day"
local MILESTONE_VAR = "goals_milestone"
local MILESTONE_REWARD_VAR = "goals_milestone_reward"

-- 上一帧看到的 last_action：用于把"按键产生的事件"与"每帧轮询"区分开，
-- 否则同一个 last_action 会在后续每一帧重复计数。
local last_seen_action

local function defs()
  local ok, mod = pcall(require, "data.goals")
  if ok and type(mod) == "table" then
    return mod
  end
  return nil
end

local function progress_var(goal_id)
  return "goals_p_" .. tostring(goal_id)
end

local function day_var(day)
  return "goals_day_" .. tostring(day)
end

local function day_reward_var(day)
  return day_var(day) .. "_reward"
end

local function current_map()
  local ok, value = pcall(function()
    return map.current() or scene.reload()
  end)
  if ok then
    return value
  end
  return nil
end

-- ---------- 读取 ----------

function M.total_days()
  local data = defs()
  return data and data.total_days() or 0
end

-- 当前目标日；表缺失时为 1（目标系统静默禁用）。
function M.day()
  local value = tonumber(state.get_var(DAY_VAR))
  if value == nil then
    return 1
  end
  local total = M.total_days()
  if total > 0 and value > total + 1 then
    return total + 1
  end
  if value < 1 then
    return 1
  end
  return math.floor(value)
end

function M.def(day)
  local data = defs()
  if not data then
    return nil
  end
  return data.day(day or M.day())
end

function M.goal_count(day)
  local def = M.def(day)
  if not def or type(def.goals) ~= "table" then
    return 0
  end
  return #def.goals
end

local function quest_satisfied(kind, quest_id)
  local ok, quest = pcall(require, "world.quest")
  if not ok or type(quest) ~= "table" or type(quest.state) ~= "function" then
    return false
  end
  local ok2, current = pcall(quest.state, quest_id)
  if not ok2 or current == nil then
    return false
  end
  if kind == "quest_accepted" then
    return current ~= "not_started"
  end
  return current == "completed"
end

local function kills_value()
  local ok, combat = pcall(require, "systems.combat")
  if not ok or type(combat) ~= "table" or type(combat.stats) ~= "function" then
    return 0
  end
  local ok2, stats = pcall(combat.stats)
  if not ok2 or type(stats) ~= "table" then
    return 0
  end
  return tonumber(stats.kills) or 0
end

local function item_value(item_id)
  -- `inv` 是引擎提供的全局背包 API（`player/inventory.lua` 只管 setup/use/存档），
  -- 与教程奖励同一条通路。
  local ok, count = pcall(inv.count, state.inventory, item_id)
  if not ok then
    return 0
  end
  return tonumber(count) or 0
end

-- 状态类目标（随时可判定）：返回 (是否达成, 当前值, 需要值)。
local function state_progress(goal)
  local match = goal.match or {}
  local need = math.max(1, math.floor(tonumber(goal.count) or 1))
  if match.kind == "enter_map" then
    local here = current_map()
    return here == match.value, here == match.value and 1 or 0, 1
  end
  if match.kind == "quest_accepted" or match.kind == "quest_delivered" then
    local done = quest_satisfied(match.kind, match.value)
    return done, done and 1 or 0, 1
  end
  if match.kind == "kills" then
    local value = kills_value()
    return value >= need, math.min(value, need), need
  end
  if match.kind == "item" then
    local value = item_value(match.value)
    return value >= need, math.min(value, need), need
  end
  if match.kind == "money" then
    local value = tonumber(state.money) or 0
    return value >= need, math.min(value, need), need
  end
  return false, 0, need
end

local function is_state_kind(goal)
  local kind = goal.match and goal.match.kind
  return kind == "quest_accepted"
    or kind == "quest_delivered"
    or kind == "kills"
    or kind == "item"
    or kind == "money"
end

-- `enter_map` 是**当日粘性事件**：去过就算达成（不是"此刻站在那儿"）。
-- 否则玩家"进村 → 回山庄采集 → 睡觉"会被判成没去过，日结算把当天记成 missed。
local function enter_map_hit(goal)
  local match = goal.match or {}
  return match.kind == "enter_map" and current_map() == match.value
end

-- 单条目标的进度：(已达成, 进度, 需要值)。
local function goal_progress(goal)
  local need = math.max(1, math.floor(tonumber(goal.count) or 1))
  if is_state_kind(goal) then
    return state_progress(goal)
  end
  local value = tonumber(state.get_var(progress_var(goal.id))) or 0
  return value >= need, math.min(value, need), need
end

function M.is_done(goal_id)
  local def = M.def()
  if not def then
    return false
  end
  for _, goal in ipairs(def.goals) do
    if goal.id == goal_id then
      return (goal_progress(goal))
    end
  end
  return false
end

-- 当天目标清单（含进度）；跨天后返回空表。
function M.goals_of(day)
  local def = M.def(day)
  if not def then
    return {}
  end
  local out = {}
  for _, goal in ipairs(def.goals) do
    local done, progress, need = goal_progress(goal)
    out[#out + 1] = {
      id = goal.id,
      text = goal.text,
      kind = goal.match and goal.match.kind or nil,
      done = done,
      progress = progress,
      count = need
    }
  end
  return out
end

function M.all_done(day)
  local def = M.def(day)
  if not def then
    return false
  end
  for _, goal in ipairs(def.goals) do
    if not (goal_progress(goal)) then
      return false
    end
  end
  return true
end

function M.completed_days()
  local data = defs()
  if not data then
    return 0
  end
  local count = 0
  for day = 1, data.total_days() do
    if state.get_var(day_var(day)) == "completed" then
      count = count + 1
    end
  end
  return count
end

function M.missed_days()
  local data = defs()
  if not data then
    return 0
  end
  local count = 0
  for day = 1, data.total_days() do
    if state.get_var(day_var(day)) == "missed" then
      count = count + 1
    end
  end
  return count
end

-- 七日里程碑是否达成（7 天全部 completed）。
function M.milestone_complete()
  local data = defs()
  if not data then
    return false
  end
  return M.completed_days() >= data.total_days()
end

-- 目标线是否走完（第 7 天已结算）。
function M.arc_done()
  return M.day() > M.total_days()
end

function M.status()
  local total = M.total_days()
  local day = M.day()
  local def = M.def(day)
  local goals = M.goals_of(day)
  local done, next_text
  for _, goal in ipairs(goals) do
    if goal.done then
      done = (done or 0) + 1
    elseif not next_text then
      next_text = goal.text
    end
  end
  return {
    day = day,
    total_days = total,
    title = def and def.title or nil,
    hint = def and def.hint or nil,
    goals = goals,
    done_count = done or 0,
    goal_count = #goals,
    next_text = next_text,
    completed_days = M.completed_days(),
    missed_days = M.missed_days(),
    milestone = state.get_var(MILESTONE_VAR) == "completed",
    reward_granted = state.get_var(day_reward_var(day)) == true,
    arc_done = M.arc_done()
  }
end

-- HUD 一行：今日目标进度 + 下一条。
function M.prompt()
  local total = M.total_days()
  if total == 0 then
    return nil
  end
  if M.arc_done() then
    if M.milestone_complete() then
      return string.format("七日之约已达成（%d/%d 天全清）", M.completed_days(), total)
    end
    return string.format("七日已过（达标 %d/%d 天）", M.completed_days(), total)
  end
  local status = M.status()
  if status.goal_count == 0 then
    return nil
  end
  local head = string.format("%s 目标 %d/%d", tostring(status.title), status.done_count, status.goal_count)
  if status.next_text then
    return head .. "：" .. tostring(status.next_text)
  end
  return head .. "：全部达成"
end

-- ---------- 写入 ----------

local function grant_reward(reward, reason)
  if type(reward) ~= "table" then
    return false
  end
  if type(reward.item) == "string" then
    local count = math.max(1, math.floor(tonumber(reward.count) or 1))
    -- `inv` 是引擎全局背包 API（与 tutorial.lua 的奖励发放同一条通路）。
    local ok = pcall(inv.add, state.inventory, reward.item, count)
    if not ok then
      log.warn("[goals] 奖励发放失败：" .. tostring(reward.item) .. " x" .. tostring(count))
      return false
    end
    log.info(
      "[goals] "
        .. tostring(reason)
        .. " 奖励 "
        .. tostring(reward.item)
        .. " x"
        .. tostring(count)
    )
    return true
  end
  if tonumber(reward.money) then
    local amount = math.floor(tonumber(reward.money))
    state.money = (tonumber(state.money) or 0) + amount
    log.info("[goals] " .. tostring(reason) .. " 奖励金钱 " .. tostring(amount))
    return true
  end
  if tonumber(reward.inner_power) then
    local amount = math.floor(tonumber(reward.inner_power))
    state.inner_power = (tonumber(state.inner_power) or 0) + amount
    log.info("[goals] " .. tostring(reason) .. " 奖励内力 " .. tostring(amount))
    return true
  end
  return false
end

local function settle_day(day)
  local def = M.def(day)
  if not def then
    return nil
  end
  -- 跨天目标（day_start）：日结算事件本身就是它的达成条件。
  for _, goal in ipairs(def.goals) do
    local kind = goal.match and goal.match.kind
    if kind == "day_start" then
      local need = math.max(1, math.floor(tonumber(goal.count) or 1))
      state.set_var(progress_var(goal.id), need, "goals", "day_start:" .. tostring(day))
    end
  end
  local complete = M.all_done(day)
  state.set_var(day_var(day), complete and "completed" or "missed", "goals", "settle:" .. tostring(day))
  local rewarded = false
  if complete and state.get_var(day_reward_var(day)) ~= true then
    if grant_reward(def.reward, "第 " .. tostring(day) .. " 天全清") then
      state.set_var(day_reward_var(day), true, "goals", "reward:" .. tostring(day))
      rewarded = true
    end
  end
  log.info(
    "[goals] 第 "
      .. tostring(day)
      .. " 天结算："
      .. (complete and "全清" or "未达成")
      .. (rewarded and "（已发奖）" or "")
  )
  return complete
end

local function grant_milestone()
  local data = defs()
  if not data or not M.milestone_complete() then
    return false
  end
  if state.get_var(MILESTONE_REWARD_VAR) == true then
    return false
  end
  state.set_var(MILESTONE_VAR, "completed", "goals", "milestone")
  local milestone = data.milestone_def and data.milestone_def() or nil
  if type(milestone) == "table" then
    if grant_reward({ money = milestone.money }, "七日之约") then
      state.set_var(MILESTONE_REWARD_VAR, true, "goals", "milestone_reward")
    end
  end
  return true
end

-- 事件：Playing 态每帧的 state.last_action。
-- 只把**动作变化**当作一次事件（否则同一个 last_action 会在后续每帧重复计数）。
function M.on_action(last_action)
  local action = tostring(last_action or "")
  local def = M.def()
  if not def then
    last_seen_action = action
    return false
  end
  local edge = action ~= last_seen_action
  last_seen_action = action
  if not edge then
    return false
  end
  local progressed = false
  for _, goal in ipairs(def.goals) do
    local match = goal.match or {}
    local kind = match.kind
    local hit = false
    local need = math.max(1, math.floor(tonumber(goal.count) or 1))
    local current = tonumber(state.get_var(progress_var(goal.id))) or 0
    if kind == "action" then
      hit = action == match.value
    elseif kind == "action_prefix" then
      hit = match.value ~= "" and action:sub(1, #match.value) == match.value
    elseif kind == "enter_map" then
      -- 补判：跨天时玩家可能已经站在目标地图上（否则要"出图再进图"才算）。
      hit = enter_map_hit(goal)
    end
    if hit and current < need then
      state.set_var(progress_var(goal.id), current + 1, "goals", "action:" .. tostring(action))
      progressed = true
    end
  end
  return progressed
end

-- 事件：新的一天（T 睡觉 → on_day_start）。
-- 日结算顺序：先结算**刚结束**的那一天（含它的 day_start 目标），再推进到新的一天。
function M.on_day_start()
  local data = defs()
  if not data then
    return false
  end
  local day = M.day()
  local settled = false
  if day <= data.total_days() then
    settle_day(day)
    settled = true
    grant_milestone()
  end
  local next_day = math.min(day + 1, data.total_days() + 1)
  state.set_var(DAY_VAR, next_day, "goals", "day_start:" .. tostring(day))
  return settled
end

-- 事件：进入地图（`enter_map` 目标去过的当天即算达成）。
function M.on_enter(map_id)
  local def = M.def()
  if not def or type(map_id) ~= "string" then
    return false
  end
  local progressed = false
  for _, goal in ipairs(def.goals) do
    local match = goal.match or {}
    if match.kind == "enter_map" and match.value == map_id then
      local need = math.max(1, math.floor(tonumber(goal.count) or 1))
      local current = tonumber(state.get_var(progress_var(goal.id))) or 0
      if current < need then
        state.set_var(
          progress_var(goal.id),
          need,
          "goals",
          "enter_map:" .. tostring(map_id)
        )
        progressed = true
      end
    end
  end
  return progressed
end

-- 新档复位：把目标线清回第 1 天（含所有计数与结算结果）。
function M.reset()
  local data = defs()
  state.set_var(DAY_VAR, 1, "goals", "reset")
  state.set_var(MILESTONE_VAR, "none", "goals", "reset")
  state.set_var(MILESTONE_REWARD_VAR, false, "goals", "reset")
  if data then
    for day = 1, data.total_days() do
      state.set_var(day_var(day), "none", "goals", "reset")
      state.set_var(day_reward_var(day), false, "goals", "reset")
      local def = data.day(day)
      if def then
        for _, goal in ipairs(def.goals) do
          state.set_var(progress_var(goal.id), 0, "goals", "reset")
        end
      end
    end
  end
  last_seen_action = nil
  return true
end

return M
