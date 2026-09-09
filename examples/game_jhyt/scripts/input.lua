local state = require("state")
local player = require("player")
local farm = require("farm")

local M = {}

local MOVE_SPEED = 140 -- 世界单位/秒（32px 格 → 约 4.4 格/秒）

-- 按移动轴合成归一化方向并直驱刚体速度。
-- 面向（state.facing）记录最后一次有效轴向，供 farm.act 计算面前格。
local function update_movement()
  local dx, dy = 0, 0
  if input.is_down("A") then dx = dx - 1 end
  if input.is_down("D") then dx = dx + 1 end
  if input.is_down("S") then dy = dy - 1 end
  if input.is_down("W") then dy = dy + 1 end

  if dx ~= 0 or dy ~= 0 then
    local len = math.sqrt(dx * dx + dy * dy)
    dx, dy = dx / len, dy / len
    if dx > 0 then
      state.facing = "right"
    elseif dx < 0 then
      state.facing = "left"
    elseif dy > 0 then
      state.facing = "up"
    elseif dy < 0 then
      state.facing = "down"
    end
    player.set_velocity(dx * MOVE_SPEED, dy * MOVE_SPEED)
    state.last_action = "move"
  else
    player.stop()
  end
end

-- 状态栏 HUD 文案（底部固定提示左侧追加当日信息；当前 MVP 只保留控制提示）
local function hud_text()
  local d = clock.now()
  return "第 " .. tostring(d.day) .. " 天 · 体力 " .. tostring(state.energy)
end

function M.update()
  player.ensure()
  update_movement()

  if input.just_pressed("e") then
    local result = farm.act(state.facing)
    if result ~= "move" and result ~= "none" then
      state.last_action = result
    end
  end

  if input.just_pressed("t") then
    state.last_action = "sleep"
    -- day.sleep() 依次触发 on_day_end（存档）与 on_day_start（日结算/再存档）
    day.sleep()
  end

  if input.just_pressed("q") then
    state.inner_power = math.min(100, state.inner_power + 5)
    state.last_action = "practice_breath"
  end

  return hud_text()
end

return M
