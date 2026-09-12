local state = require("state")
local player = require("player")
local interaction = require("interaction")
local dialogue = require("dialogue")
local shop = require("shop")
local combat = require("combat")

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

local function just_pressed(key)
  return input.just_pressed(key)
end

local function update_context()
  if just_pressed("p") then
    state.toggle_pause()
  elseif just_pressed("i") then
    state.toggle_inventory()
  end
end

-- 状态栏 HUD 文案（底部固定提示左侧追加当日信息；当前 MVP 只保留控制提示）
local function hud_text()
  local d = clock.now()
  local hp_text = combat.is_player_dead() and "气血 0（战败）"
    or "气血 " .. tostring(combat.player_hp())
  return "第 " .. tostring(d.day) .. " 天 · 体力 " .. tostring(state.energy)
    .. " · " .. hp_text
end

-- 对话态：冻结移动与移动端交互，Esc 结束、数字键选择选项。
-- 必须在 `not Playing` 早退之前分发，否则对话输入会被吞掉（M17-03 风险项）。
local function update_dialogue()
  player.stop()
  if just_pressed("escape") then
    dialogue.close()
  else
    for i = 1, dialogue.option_limit() do
      if just_pressed(tostring(i)) then
        dialogue.choose(i)
        break
      end
    end
  end
  -- 换节点后重绘；若已结束（close 复位 Playing）则不再绘制。
  if state.is_mode("Dialogue") then
    dialogue.render()
  end
  return hud_text()
end

-- 商店态：冻结移动，Esc 关闭、F 切换买卖、+/- 调整数量、数字键选择、空格确认。
-- 同样必须在 `not Playing` 早退之前分发，否则商店输入会被吞掉。
local function update_shop()
  player.stop()
  if just_pressed("escape") then
    shop.close_ui()
  else
    if just_pressed("f") then
      shop.toggle_tab()
    end
    if just_pressed("+") then
      shop.adjust(1)
    elseif just_pressed("-") then
      shop.adjust(-1)
    end
    for i = 1, shop.row_limit() do
      if just_pressed(tostring(i)) then
        shop.select(i)
        break
      end
    end
    if just_pressed("space") then
      shop.confirm()
    end
  end
  -- 换页/交易后重绘；若已关闭（close_ui 复位 Playing）则不再绘制。
  if state.is_mode("Shop") then
    shop.render()
  end
  return hud_text()
end

function M.update()
  player.ensure()

  if state.is_mode("Dialogue") then
    return update_dialogue()
  end

  if state.is_mode("Shop") then
    return update_shop()
  end

  update_context()

  if not state.is_mode("Playing") then
    player.stop()
    return hud_text()
  end

  -- 战败态：冻结移动与攻击，仅 R 重开（复用 Playing 态，不新增状态机模式）。
  if combat.is_player_dead() then
    player.stop()
    if just_pressed("r") then
      combat.respawn()
    end
    return "战败 · 按 R 重新开始"
  end

  update_movement()

  if input.just_pressed("e") then
    local result = interaction.interact(state.facing)
    if result ~= "move" and result ~= "none" then
      state.last_action = result
    end
  end

  -- 战斗输入（M18-01）：J 普攻（随武器加成）、K 武学（消耗内力）。
  -- 攻击不锁移动（ARPG 习惯）；冷却/前摇/去重由 combat 底座处理。
  if input.just_pressed("j") then
    combat.player_attack("strike")
  elseif input.just_pressed("k") then
    combat.player_attack("palm_gust")
  end

  if input.just_pressed("t") then
    state.last_action = "sleep"
    -- day.sleep() 依次触发 on_day_end（存档）与 on_day_start（日结算/再存档）
    day.sleep()
  end

  if input.just_pressed("q") then
    state.inner_power = math.min(100, (state.inner_power or 0) + 5)
    state.last_action = "practice_breath"
  end

  return hud_text()
end

return M
