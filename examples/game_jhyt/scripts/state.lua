local M = {
  player = nil,
  inventory = "player",
  money = 120,
  energy = 100,
  inner_power = 50,
  facing = "down",
  selected_seed = "seed_lingzhi",
  selected_tool = "hoe",
  season = "spring",
  weather = "sunny",
  labor_minutes = 0,
  last_action = "none",
  mode = "Playing",
  return_mode = "Playing"
}

local VALID_MODES = {
  Playing = true,
  Dialogue = true,
  Inventory = true,
  Shop = true,
  Pause = true,
  Transition = true
}

function M.get_mode()
  return M.mode
end

function M.is_mode(mode)
  return M.mode == mode
end

function M.set_mode(mode)
  if not VALID_MODES[mode] then
    return false
  end
  if M.mode ~= mode then
    if mode == "Pause" then
      M.return_mode = M.mode
    elseif M.mode == "Pause" then
      M.return_mode = "Playing"
    end
    M.mode = mode
  end
  return true
end

function M.toggle_pause()
  if M.mode == "Pause" then
    return M.set_mode(M.return_mode or "Playing")
  end
  if M.mode == "Playing" then
    return M.set_mode("Pause")
  end
  return false
end

function M.toggle_inventory()
  if M.mode == "Inventory" then
    return M.set_mode("Playing")
  end
  if M.mode == "Playing" then
    return M.set_mode("Inventory")
  end
  return false
end

function M.capture()
  return {
    money = M.money,
    energy = M.energy,
    inner_power = M.inner_power,
    facing = M.facing,
    selected_seed = M.selected_seed,
    selected_tool = M.selected_tool,
    season = M.season,
    weather = M.weather,
    labor_minutes = M.labor_minutes,
    last_action = M.last_action,
    mode = M.mode
  }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  M.money = tonumber(data.money) or M.money
  M.energy = tonumber(data.energy) or M.energy
  M.inner_power = tonumber(data.inner_power) or M.inner_power
  M.facing = tostring(data.facing or M.facing)
  M.selected_seed = data.selected_seed and tostring(data.selected_seed) or M.selected_seed
  M.selected_tool = data.selected_tool and tostring(data.selected_tool) or M.selected_tool
  M.season = data.season and tostring(data.season) or M.season
  M.weather = data.weather and tostring(data.weather) or M.weather
  M.labor_minutes = tonumber(data.labor_minutes) or M.labor_minutes
  M.last_action = tostring(data.last_action or M.last_action)
  if VALID_MODES[data.mode] then
    M.mode = data.mode
  else
    M.mode = "Playing"
  end
  M.return_mode = "Playing"
end

-- 新档默认值（开始画面「开始新游戏」进入山庄前调用）。
-- player 为运行时实体 id，不入档、不在新档残留；由 player.ensure() 重建。
function M.reset()
  M.player = nil
  M.inventory = "player"
  M.money = 120
  M.energy = 100
  M.inner_power = 50
  M.facing = "down"
  M.selected_seed = "seed_lingzhi"
  M.selected_tool = "hoe"
  M.season = "spring"
  M.weather = "sunny"
  M.labor_minutes = 0
  M.last_action = "none"
  M.mode = "Playing"
  M.return_mode = "Playing"
end

return M
