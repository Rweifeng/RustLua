-- systems/solar_terms.lua：节气历法的**只读查询**（阶段3「节气历法」，docs/33 2.1）。
--
-- 与 world/festival.lua 同一取舍（节气是 day 的派生值）：
--   * 本模块**没有运行时状态、没有存档 provider**——"今天是什么节气"由 day 纯函数推出
--     （world/calendar.lua 的 season_of / day_in_season + data/solar_terms.lua 的查表），
--     跨天/读档/反复调用结论必然一致，故 capture/restore/reset 全部空实现。
--   * 消费者（gather/shop/farm/hud…）每次要加成时自取一次查询结果，运行时代码里不写
--     `if term == "qingming"` 这种分支串。阶段3 修饰为**纯查询面**：暂不接线真实采集/出货，
--     只把加成以统一形状摆出来，待 Organizer 接线。
--   * 每个查询都接受可选的 `day` 参数（缺省 = clock.now().day），headless 验收可按日号问。
--
-- 与 world/festival.lua 的分工：节日是**3 天窗口 + 设施门槛 + effects 消费方**；
-- 节气是**单日天文原点 + 修饰查询面**，两者只读地通过 festival_link 对照（不互相改）。
local calendar = require("world.calendar")
local data = require("data.solar_terms")

local M = {}

local function day_number(day)
  if day ~= nil then
    return math.floor(tonumber(day) or 1)
  end
  return math.floor(tonumber(clock.now().day) or 1)
end

--- 给定日数（缺省今天）的节气条目；当天无节气返回 nil。纯函数。
function M.today(day)
  local d = day_number(day)
  return data.at(calendar.season_of(d), calendar.day_in_season(d))
end

--- 节气 id（当天无节气 = ""）：诊断与验收用。
function M.id(day)
  local entry = M.today(day)
  return entry and entry.id or ""
end

--- 玩家看到的中文名（无节气 = ""）。
function M.name(day)
  local entry = M.today(day)
  return entry and entry.name or ""
end

--- 当日节气的原始修饰表（data 里那张，调用方只读）；无节气返回空表。
function M.modifiers(day)
  local entry = M.today(day)
  if entry and type(entry.modifiers) == "table" then
    return entry.modifiers
  end
  return {}
end

--- 聚合后的当日修饰查询面（供其他系统直接读，带默认值）：
--   mult 类缺省 1、bonus 类缺省 0——调用方 `season_modifiers().gather_mult` 永远有数。
function M.season_modifiers(day)
  local out = {
    gather_mult = 1,
    crop_bonus = 1,
    fish_bonus = 1,
    sell_bonus = 0,
    energy_bonus = 0,
  }
  for key, value in pairs(M.modifiers(day)) do
    if out[key] ~= nil and type(value) == "number" then
      out[key] = value
    end
  end
  return out
end

--- 节气 → 阶段2 节日的只读对照表（term_id → festival_id）；无关联返回 nil。
function M.festival_link(term_id)
  local entry = data.get(term_id)
  return entry and entry.festival_link or nil
end

--- 全部节气→节日对照（验收扫描用，升序）。
function M.festival_links()
  local out = {}
  for _, entry in ipairs(data.terms) do
    if entry.festival_link then
      out[#out + 1] = entry.id .. "->" .. entry.festival_link
    end
  end
  table.sort(out)
  return out
end

-- ---------- 纯派生：无状态钩子（on_day_start 显式调用一次以便挂点闭环） ----------

-- 跨天钩子：节气随 day 自动派生，这里只回当日 id（日志/挂点用），不落任何状态。
function M.on_day_start()
  return M.id()
end

-- ---------- 纯派生：capture/restore/reset 空实现（不入档） ----------

function M.capture()
  return {}
end

function M.restore(_data)
  return true
end

function M.reset()
end

-- ---------- 加载期校验：纯派生模块自身无可校验状态，恒空 ----------

function M.validate_all()
  return ""
end

return M
