-- tutorial.lua：《江湖有田》新手引导运行时（M19-02）。
--
-- 目标（验收项④）：新玩家不读源码即可走完 MVP 剧本。步骤序列是纯数据
-- （data/tutorial.lua），本模块只负责「匹配事件 → 推进步骤 → 写剧情变量」。
--
-- 持久化：步骤进度写 state.vars["tutorial_step"]（= 已完成步数，0 起算），
-- 随 state provider 存档恢复，无需新增 provider。一次性奖励锚点
-- state.vars["tutorial_rewarded"] 保证重复完成不重复发奖。
--
-- 幂等：每次事件最多推进一步；重复触发同一步不会重复推进（当前步骤已变，
-- 且旧步骤不再匹配）。
--
-- 非新档不打扰：tutorial_step 缺失（功能上线前的老档）或已达 #steps
-- 时 prompt() 返回 nil、事件不产生任何写入。
--
-- 事件由 input.lua / main.lua 驱动：
--   * input.update（Playing 态每帧）→ M.on_action(state.last_action)
--   * main.on_day_start → M.on_day_start()
--   * main.enter_playable_map → M.on_enter(map_id)
local state = require("core.state")
local quest = require("world.quest")

local M = {}

local STEP_VAR = "tutorial_step"
local REWARD_VAR = "tutorial_rewarded"

-- 数据表（require 缓存；加载失败时回退为空步骤表，引导静默禁用）。
local function defs()
  local ok, mod = pcall(require, "data.tutorial")
  if ok and type(mod) == "table" then
    return mod
  end
  return { steps = {} }
end

function M.steps()
  local data = defs()
  return type(data.steps) == "table" and data.steps or {}
end

-- 当前步骤下标：nil = 引导未启用（老档）；0 = 第一步；#steps = 已完成。
function M.index()
  local value = tonumber(state.get_var(STEP_VAR))
  if value == nil then
    return nil
  end
  if value < 0 then
    return 0
  end
  local total = #M.steps()
  if value > total then
    return total
  end
  return math.floor(value)
end

function M.total()
  return #M.steps()
end

-- 当前步骤（nil = 未启用或已全部完成）。
function M.step()
  local index = M.index()
  if index == nil then
    return nil
  end
  return M.steps()[index + 1]
end

function M.active()
  return M.step() ~= nil
end

-- HUD 提示区文案；未启用/已完成返回 nil。
function M.prompt()
  local index = M.index()
  local step = M.step()
  if index == nil or not step then
    return nil
  end
  return string.format("引导 %d/%d：%s", index + 1, #M.steps(), tostring(step.prompt))
end

-- 只读状态（headless 验收 / 调试面板用）。
function M.status()
  local index = M.index()
  local step = M.step()
  return {
    active = step ~= nil,
    index = index,
    total = #M.steps(),
    step = step and step.id or nil,
    prompt = M.prompt(),
    rewarded = state.get_var(REWARD_VAR) == true
  }
end

-- 一次性奖励（幂等：只有 tutorial_rewarded 未置位才发）。
function M.grant_reward()
  local data = defs()
  local reward = data.reward
  if type(reward) ~= "table" or type(reward.item) ~= "string" then
    return false
  end
  if state.get_var(REWARD_VAR) == true then
    return false
  end
  local count = math.floor(tonumber(reward.count) or 0)
  if count <= 0 then
    return false
  end
  state.set_var(REWARD_VAR, true, "tutorial", "reward")
  local ok = pcall(inv.add, state.inventory, reward.item, count)
  if not ok then
    log.warn("[tutorial] 奖励发放失败：" .. tostring(reward.item))
    return false
  end
  state.last_action = "tutorial_reward_" .. reward.item
  log.info("[tutorial] 引导完成，奖励 " .. tostring(reward.item) .. " x" .. tostring(count))
  return true
end

-- 推进到下一步；返回是否确实推进（旧步骤 id 记入变量事件流水）。
function M.advance(reason)
  local index = M.index()
  if index == nil then
    return false
  end
  local step = M.steps()[index + 1]
  if not step then
    return false
  end
  local next_index = index + 1
  state.set_var(STEP_VAR, next_index, "tutorial", tostring(step.id) .. ":" .. tostring(reason or "event"))
  log.info("[tutorial] 步骤完成：" .. tostring(step.id) .. " → " .. tostring(next_index) .. "/" .. tostring(#M.steps()))
  -- 进入终止标记（最后一步 = finish）即视为完成并发奖。
  M.settle()
  if M.index() == #M.steps() then
    -- 兜底：无论走到哪条路径，只要已到末步则确保发奖（REWARD_VAR 保证幂等）。
    M.grant_reward()
  end
  return true
end

-- 终止标记自动收尾（幂等：已完成时重复调用无写入）。
function M.settle()
  local index = M.index()
  if index == nil then
    return false
  end
  local step = M.steps()[index + 1]
  if not step then
    return false
  end
  local kind = step.match and step.match.kind
  if kind ~= "finish" then
    return false
  end
  state.set_var(STEP_VAR, #M.steps(), "tutorial", tostring(step.id) .. ":finish")
  M.grant_reward()
  return true
end

local function values_of(value)
  if type(value) == "table" then
    return value
  end
  return { value }
end

local function action_matches(match, action)
  local kind = match.kind
  for _, candidate in ipairs(values_of(match.value)) do
    if type(candidate) == "string" and candidate ~= "" then
      if kind == "action" and action == candidate then
        return true
      end
      if kind == "action_prefix" and action:sub(1, #candidate) == candidate then
        return true
      end
    end
  end
  return false
end

local function quest_matches(match)
  if type(match.value) ~= "string" then
    return false
  end
  local current = quest.state(match.value)
  if current == nil then
    return false
  end
  if match.kind == "quest_accepted" then
    return current ~= "not_started"
  end
  if match.kind == "quest_delivered" then
    return current == "completed"
  end
  return false
end

-- 事件：Playing 态每帧的 state.last_action（以及任务状态轮询）。
function M.on_action(last_action)
  local step = M.step()
  if not step then
    return false
  end
  local match = step.match or {}
  local kind = match.kind
  local matched = false
  if kind == "action" or kind == "action_prefix" then
    matched = action_matches(match, tostring(last_action or ""))
  elseif kind == "quest_accepted" or kind == "quest_delivered" then
    -- 任务类步骤在任意动作时轮询（交付/接取可能不产生新的 last_action）。
    matched = quest_matches(match)
  elseif kind == "finish" then
    matched = true
  end
  if not matched then
    return false
  end
  return M.advance(kind)
end

-- 事件：新的一天（T 睡觉 → on_day_start）。
function M.on_day_start()
  local step = M.step()
  if not step or not step.match or step.match.kind ~= "day_start" then
    return false
  end
  return M.advance("day_start")
end

-- 事件：进入地图（enter_playable_map / on_create）。
function M.on_enter(map_id)
  M.settle()
  local step = M.step()
  if not step or not step.match or step.match.kind ~= "enter_map" then
    return false
  end
  if type(map_id) ~= "string" or step.match.value ~= map_id then
    return false
  end
  return M.advance("enter_map:" .. map_id)
end

-- 新档复位：从第 0 步重新开始，并允许重新发奖。
function M.reset()
  state.set_var(STEP_VAR, 0, "tutorial", "reset")
  state.set_var(REWARD_VAR, false, "tutorial", "reset")
  return true
end

-- 兼容老档：未启用引导时不写入任何状态。
function M.disable()
  state.set_var(STEP_VAR, #M.steps(), "tutorial", "disable")
  return true
end

return M
