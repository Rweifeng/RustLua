local state = require("core.state")
local festivals = require("data.festivals")
local M = {}

local SEASONS = { "spring", "summer", "autumn", "winter" }

-- 每季天数与 data/festivals.lua 同源：节气窗口是按"季内第几天"写的，
-- 两处若各写一个 28，改季长时节气就会整体错位。
local SEASON_LENGTH = festivals.season_length

function M.sync()
  local now = clock.now()
  local day = tonumber(now.day) or 1
  local season_index = math.floor((day - 1) / SEASON_LENGTH) % #SEASONS + 1
  state.season = SEASONS[season_index]
  return state.season
end

function M.current()
  return M.sync()
end

--- 季内第几天（1..SEASON_LENGTH）：day 1 → 1、day 29 → 1、day 113 → 1。
function M.day_in_season(day)
  local d = math.floor(tonumber(day) or 1)
  if d < 1 then
    d = 1
  end
  return (d - 1) % SEASON_LENGTH + 1
end

--- 季节 id（spring/summer/autumn/winter），与 sync() 用同一套算法，不读 state。
function M.season_of(day)
  local d = math.floor(tonumber(day) or 1)
  if d < 1 then
    d = 1
  end
  return SEASONS[math.floor((d - 1) / SEASON_LENGTH) % #SEASONS + 1]
end

--- 给定日数的节气条目（data/festivals.lua 的一条），不在任何窗口里返回 nil。
--- **纯映射**：不查随机数、不写状态、不入存档——112 天（四季 × 28 天）一个循环，
--- 所以"今天是什么节气"只需要 day 一个输入，读档/重进图/反复调用结论必然一致。
--- 缺省参数 = 今天（clock.now().day）。
function M.jieqi(day)
  local d = math.floor(tonumber(day) or (tonumber(clock.now().day) or 1))
  return festivals.at(M.season_of(d), M.day_in_season(d))
end

return M
