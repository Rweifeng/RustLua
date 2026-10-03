-- world/festival.lua：节气效果的**只读查询**（M34-02）。
--
-- 形状说明（与 player/relationship.lua 的档位查询同一口径）：
--   * 节气是 `day` 的派生值（world/calendar.lua 的 M.jieqi），所以本模块**没有状态、
--     没有刷新钩子、没有存档 provider**——存档只写分数与锚点，跨天/读档后节气自己回来。
--   * 消费者（gather / shop / map_manager / dialogue）每次交易、每次按 E 自取一次查询结果，
--     运行时代码里不出现 `if jieqi == "qingming"` 这种分支串：七个节气各写一处 if，
--     第八个必然漏一处。
--   * 唯一的例外是 `M.available()`（M34-05）：它除了看日历，还读一个"茶棚建到过几级"的
--     state var。仍然是**只读**——本模块不写它、不刷新它、不为它增设 provider，
--     写方是 systems/building.lua，随 state provider 进出存档。
--   * 每个查询都接受可选的 `day` 参数（缺省 = clock.now().day），
--     headless 验收可以直接按日号问"那天是什么效果"，不用拨时钟。
local calendar = require("world/calendar")
local state = require("core.state")
local buildings = require("data.buildings")

local M = {}

local function day_number(day)
  if day ~= nil then
    return math.floor(tonumber(day) or 1)
  end
  return math.floor(tonumber(clock.now().day) or 1)
end

--- 给定日数（缺省今天）的节气条目；不在任何窗口里返回 nil。
function M.of_day(day)
  return calendar.jieqi(day_number(day))
end

function M.current()
  return M.of_day(nil)
end

--- 节气 id（无事件时 ""）：诊断与验收用，不参与玩法判定。
function M.id(day)
  local entry = M.of_day(day)
  return entry and entry.id or ""
end

--- 玩家看到的中文称号（无事件时 ""）。
function M.name(day)
  local entry = M.of_day(day)
  return entry and entry.name or ""
end

--- 这场事件**今天**做不做得成：日数落在窗口里，且数据里的设施门槛（`unlock`）已满足。
--- 与 `M.id()` 的分工要分清：id() 只回答"日历今天算哪天"（纯派生，谁都改不了它），
--- available() 才回答"这天的事能不能办"。对话条件只走这一个入口——
--- 分成两处判就会变成"是不是春分"与"能不能开茶会"各判一半，第三处必然漏。
--- 等级读的是 systems/building.lua 记进 state.vars 的历史最高值（跨地图可用：
--- 茶棚在山庄，而沈青萝在云泽村）。
function M.available(id, day)
  if type(id) ~= "string" then
    return false
  end
  local entry = M.of_day(day)
  if not entry or entry.id ~= id then
    return false
  end
  local unlock = entry.unlock
  if unlock == nil then
    return true
  end
  local def = buildings.get(unlock.building)
  if not def then
    -- 加载期校验已经挡住这种数据；这里宁可不办，也不让一条坏数据变成"永远可办"。
    return false
  end
  local reached = tonumber(state.get_var(buildings.level_var(unlock.building))) or 0
  return reached >= (tonumber(unlock.level) or 1)
end

--- 该图今日的采集规则：`{ mult = 整数倍 }` 或 `{ one_time_daily = true }`，无事件返回 nil。
--- 返回的是数据表本身，调用方**只读**（改了会污染下一帧的判定，故不复制但要写清约束）。
function M.gather_rule(map_id, day)
  if type(map_id) ~= "string" then
    return nil
  end
  local entry = M.of_day(day)
  local gather = entry and entry.effects and entry.effects.gather
  if type(gather) ~= "table" then
    return nil
  end
  for _, listed in ipairs(gather.maps or {}) do
    if listed == map_id then
      return gather
    end
  end
  return nil
end

--- 该物品今日的出售价加成百分比（无事件/不在行情表里 = 0）。
function M.sell_bonus(item_id, day)
  local entry = M.of_day(day)
  local bonus = entry and entry.effects and entry.effects.sell_bonus
  if type(bonus) ~= "table" then
    return 0
  end
  for _, listed in ipairs(bonus.items or {}) do
    if listed == item_id then
      return tonumber(bonus.bonus) or 0
    end
  end
  return 0
end

--- 该出口今日是否封山（只回答"是/不是"，方向与放行都由调用方的数据决定）。
function M.exit_closed(exit_id, day)
  if type(exit_id) ~= "string" then
    return false
  end
  local entry = M.of_day(day)
  local closed = entry and entry.effects and entry.effects.exit_closed
  if type(closed) ~= "table" then
    return false
  end
  for _, listed in ipairs(closed) do
    if listed == exit_id then
      return true
    end
  end
  return false
end

return M
