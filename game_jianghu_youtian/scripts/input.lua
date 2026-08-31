local state = require("state")
local player = require("player")
local inventory = require("inventory")

local M = {}

function M.update()
  player.ensure()
  local speed = 2.5
  if input.is_down("W") then player.move(0, -speed, "up") end
  if input.is_down("S") then player.move(0, speed, "down") end
  if input.is_down("A") then player.move(-speed, 0, "left") end
  if input.is_down("D") then player.move(speed, 0, "right") end

  if input.just_pressed("E") and inventory.use("seed_lingzhi") then
    state.last_action = "plant_lingzhi"
  end
  if input.just_pressed("Q") then
    state.last_action = "practice_basic_blade"
    state.inner_power = math.min(100, state.inner_power + 5)
  end
  if input.just_pressed("F") then
    state.last_action = "interact"
  end

  return "jianghu_youtian_day_" .. tostring(clock.now().day)
end

return M
