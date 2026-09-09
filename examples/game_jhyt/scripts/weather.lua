local state = require("state")
local M = {}

function M.current()
  local weather = state.weather
  if weather ~= "sunny" and weather ~= "rainy" then
    weather = "sunny"
    state.weather = weather
  end
  return weather
end

function M.is_rainy()
  return M.current() == "rainy"
end

function M.set(value)
  if value == "sunny" or value == "rainy" then
    state.weather = value
    return true
  end
  return false
end

return M
