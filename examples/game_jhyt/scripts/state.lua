local M = {
  player = nil,
  inventory = "player",
  money = 120,
  energy = 100,
  inner_power = 50,
  facing = "down",
  last_action = "none"
}

-- 存档快照（save_manager 调用）：player 是运行时实体 id，跨会话会代际失效，
-- 不入档；实体由 player.ensure() 按 PersistId 恢复。
function M.capture()
  return {
    money = M.money,
    energy = M.energy,
    inner_power = M.inner_power,
    facing = M.facing,
    last_action = M.last_action
  }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  M.money = tonumber(data.money) or M.money
  M.energy = tonumber(data.energy) or M.energy
  M.inner_power = tonumber(data.inner_power) or M.inner_power
  M.facing = tostring(data.facing)
  M.last_action = tostring(data.last_action)
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
  M.last_action = "none"
end

return M
