local state = require("state")
local M = {}

local SEASONS = { "spring", "summer", "autumn", "winter" }

function M.sync()
  local now = clock.now()
  local day = tonumber(now.day) or 1
  local season_index = math.floor((day - 1) / 28) % #SEASONS + 1
  state.season = SEASONS[season_index]
  return state.season
end

function M.current()
  return M.sync()
end

return M
