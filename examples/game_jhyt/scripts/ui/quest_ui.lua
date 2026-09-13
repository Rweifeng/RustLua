-- quest_ui.lua：《江湖有田》任务日志面板（M19-02；导航/滚动 M20-02）。
--
-- 只读面板：列出全部已注册任务的五态（not_started / in_progress /
-- deliverable / completed / failed）与当前选中任务的目标进度。
-- **不在此交付任务**：交付仍走对话/交互（M17-03 的单一入口），避免出现
-- 第二条交付路径导致奖励重复或状态漂移。
--
-- 键位（与 shop.lua 同款纯键盘范式）：
--   W/S 或上下方向键 上/下选择（走引擎焦点环，到边界环绕；方向键为 M20-02）
--   A/D 翻页（每页 7 条），1-9 直接定位
--   L / Q / Esc 关闭
-- 任务超过一页时列表按选中行滚动（引擎 `ui.scroll_*`，M20-02）。
--
-- 数据来源：quest.ids() / quest.state(id) / quest.progress(id)（只读接口），
-- 本模块不写任何任务状态。
local state = require("core.state")
local quest = require("world.quest")
local npcs = require("data.npcs")
-- M19-02：面板开闭音效（只读面板无确认/取消操作）；M20-02 走 UI 总线。
local audio = require("core.audio")
-- M20-02：列表焦点/滚动导航（引擎 ui_navigation）。
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环）。
local hints = require("ui.hints")

-- 数字直选 1..9 的按键名（默认只有键盘数字键；未绑定该设备的动作自动跳过）。
local function digits_label()
  return hints.binding_labels(
    { "slot_1", "slot_2", "slot_3", "slot_4", "slot_5", "slot_6", "slot_7", "slot_8", "slot_9" },
    nil,
    "/"
  )
end

-- 底部提示：按当前设备二选一，按键名从**当前绑定**推导（改键后自动同步）。
local function hint_line()
  local close_label = hints.binding_labels({ "quest", "cancel", "practice" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = "A/D", alt_keyboard = "左右方向键", gamepad = "左右推杆", label = "翻页" },
    { keyboard = "1-9", gamepad = digits_label(), label = "定位" },
    { keyboard = close_label, gamepad = close_label, label = "关闭" },
  })
end

local M = {}

local PANEL = { x = 340, y = 120, w = 600, h = 470 }
local ROW_LIMIT = 7
local ROW_H = 34

local FOCUS = "qst:focus"
local SCROLL = "qst:scroll"

local STATE_TEXT = {
  not_started = "未接取",
  in_progress = "进行中",
  deliverable = "可交付",
  completed = "已完成",
  failed = "已失败"
}

local ui_state = nil

-- 前向声明（定义在 select/move 之后；Lua 词法作用域只认已声明的 local）。
local sync_focus

function M.state_text(value)
  return STATE_TEXT[value] or tostring(value)
end

function M.row_limit()
  return ROW_LIMIT
end

function M.ui_active()
  return ui_state ~= nil
end

function M.index()
  return ui_state and ui_state.index or 0
end

-- 任务行：{ id, name, state, giver_name }。id 排序沿用 quest.ids()（注册表顺序）。
function M.rows()
  local out = {}
  for _, id in ipairs(quest.ids()) do
    local def = quest.get(id) or {}
    local giver = npcs.get(def.giver)
    out[#out + 1] = {
      id = id,
      name = tostring(def.name or id),
      state = quest.state(id),
      giver_name = giver and giver.name or nil
    }
  end
  return out
end

-- 选中任务的目标进度（只读）。
function M.objectives()
  if not ui_state then
    return {}
  end
  local row = M.rows()[ui_state.index + 1]
  if not row then
    return {}
  end
  return quest.progress(row.id)
end

local function clear_ui()
  ui.clear("qst:box")
  for _, id in ipairs({ "qst:title", "qst:state", "qst:giver", "qst:objtitle", "qst:hint", "qst:empty", "qst:msg" }) do
    ui.hide(id)
  end
  for i = 1, ROW_LIMIT do
    ui.hide("qst:row" .. i)
    ui.hide("qst:obj" .. i)
  end
end

function M.select(index)
  if not ui_state then
    return false
  end
  local rows = M.rows()
  local i = math.floor(tonumber(index) or 0)
  if i < 0 or i >= #rows then
    return false
  end
  ui_state.index = i
  sync_focus()
  return true
end

-- 焦点环与当前序号同步（M20-02）。
function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #M.rows(), 1)
  nav.select(FOCUS, ui_state.index + 1)
end

-- 焦点环方向移动（`step` 行，正数向下）；引擎 API 不可用返回 nil。
local function ring_step(step)
  if not ui_state then
    return nil
  end
  local direction = step > 0 and "down" or "up"
  local index = ui_state.index + 1
  local moved = false
  for _ = 1, math.abs(step) do
    local next_index = nav.move(FOCUS, direction)
    if not next_index then
      break
    end
    index = next_index
    moved = true
  end
  if not moved then
    return nil
  end
  return index - 1
end

-- 相对移动：焦点环可用时按方向逐行移动（到边界环绕），否则退回钳制语义。
function M.move(delta)
  if not ui_state then
    return false
  end
  local rows = M.rows()
  if #rows == 0 then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 then
    return false
  end
  sync_focus()
  local target = ring_step(step)
  if target == nil then
    target = math.max(0, math.min(#rows - 1, ui_state.index + step))
  end
  if target == ui_state.index then
    return false
  end
  ui_state.index = target
  return true
end

-- 方向键移动（M20-02）：直接交给引擎焦点环（到边界环绕）。
function M.move_dir(direction)
  if not ui_state or #M.rows() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  ui_state.index = index - 1
  return true
end

function M.render()
  if not ui_state then
    return false
  end
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local rows = M.rows()
  sync_focus()
  ui.panel("qst:box", x, y, w, h, 0.10, 0.11, 0.15, 0.94)
  ui.text_at("qst:title", x + 24, y + 16, "任务日志", 1.0, 22.0)

  if #rows == 0 then
    ui.text_at("qst:empty", x + 24, y + 70, "暂无任务", 1.0, 18.0)
  else
    ui.hide("qst:empty")
  end

  -- 任务超过一页时按选中行滚动窗口（引擎 ScrollState，M20-02）。
  local first = nav.window(SCROLL, #rows, ROW_LIMIT, ROW_H, ui_state.index + 1)
  for slot = 0, ROW_LIMIT - 1 do
    local id = "qst:row" .. (slot + 1)
    local index = first + slot
    local row = rows[index]
    if row then
      local label = string.format("%d. %s【%s】", index, row.name, M.state_text(row.state))
      if index == ui_state.index + 1 then
        label = "> " .. label
      else
        label = "  " .. label
      end
      ui.text_at(id, x + 24, y + 58 + slot * ROW_H, label, 1.0, 17.0)
    else
      ui.hide(id)
    end
  end

  -- 选中任务详情：发布者 + 目标进度（只读）。
  local row = rows[ui_state.index + 1]
  if row then
    ui.text_at("qst:state", x + w - 220, y + 20, "状态 " .. M.state_text(row.state), 1.0, 16.0)
    ui.text_at("qst:giver", x + 24, y + 58 + ROW_LIMIT * ROW_H, "发布者 " .. tostring(row.giver_name or "未知"), 1.0, 15.0)
    ui.text_at("qst:objtitle", x + 24, y + 58 + ROW_LIMIT * ROW_H + 26, "目标：", 1.0, 15.0)
    local objectives = M.objectives()
    for i = 1, ROW_LIMIT do
      local objective = objectives[i]
      local id = "qst:obj" .. i
      if objective then
        local mark = objective.done and "✓" or "…"
        local text = string.format("%s %s %d/%d", mark, tostring(objective.label), objective.current, objective.target)
        ui.text_at(id, x + 56, y + 58 + ROW_LIMIT * ROW_H + 26 + i * 24, text, 1.0, 15.0)
      else
        ui.hide(id)
      end
    end
  else
    for _, id in ipairs({ "qst:state", "qst:giver", "qst:objtitle" }) do
      ui.hide(id)
    end
    for i = 1, ROW_LIMIT do
      ui.hide("qst:obj" .. i)
    end
  end

  ui.text_at("qst:msg", x + 24, y + h - 56, "只读面板：任务在对话/交互中接取与交付", 1.0, 14.0)
  ui.text_at("qst:hint", x + 24, y + h - 30, hint_line(), 1.0, 14.0)
  return true
end

function M.open()
  ui_state = { index = 0 }
  state.set_mode("Quest")
  audio.ui("ui_click")
  M.render()
  return true
end

function M.close()
  if not ui_state then
    return false
  end
  ui_state = nil
  clear_ui()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  if state.is_mode("Quest") then
    state.set_mode("Playing")
  end
  audio.ui("ui_cancel")
  return true
end

function M.reset()
  ui_state = nil
  clear_ui()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  return true
end

return M
