-- title_menu.lua：主入口门面（main.scene）的手柄/键盘菜单导航（M20-09）。
--
-- 背景：`main.scene` 的三个按钮（BtnStart / BtnContinue / BtnExit）只有
-- `UiEvents event=click;function=on_click` —— 也就是**只有鼠标点击**通路，
-- 手柄与键盘在标题界面完全没有反应。本模块补齐这一层：
--
--   * 焦点环（`ui.panel_nav`）持有选中项，方向键 / DPad / 左摇杆 / 数字键直选都能改
--     选中；导航手感（摇杆阈值、按住重复）复用 `ui.input.ui_step()`，与游戏内面板
--     完全一致（不另写一份，避免手感分叉）；
--   * 选中项以金色高亮框 + 左侧箭头画在按钮上（`ui.panel` + `ui.text_at`，与暂停菜单
--     同款动态 UI 做法，不改场景文件）；
--   * 确认（`confirm`：空格 / 手柄 A）与鼠标点击统一走场景脚本的 `on_click(name)`，
--     两条路径**同一个入口**，因此行为完全一致；
--   * 顶部显示当前设备对应的操作说明（`ui.hints`），与游戏内 HUD 同源；
--   * 手柄 B / Esc（cancel）在标题界面等于「选中退出江湖」。
local nav = require("ui.panel_nav")
local hints = require("ui.hints")
-- `ui.input` 与 main.lua 同层：模块加载期不能 require（会与 main → title_menu 成环），
-- 首次使用时惰性取一次（此时 Lua 的 require 缓存里已有它）。
local input_module = nil

local function game_input()
  if not input_module then
    input_module = require("ui.input")
  end
  return input_module
end

local M = {}

local FOCUS = "title:menu"

-- 与 `main.scene` 里按钮/文字实体的 UiLayout 对齐（设计分辨率 1280x720）：
-- BtnStart 480,432,800,496 / BtnContinue 480,504,800,568 / BtnExit 480,576,800,640；
-- 文字 offsets 见场景文件（entity 8/10/12），这里逐字重画以显示选中箭头。
local OPTIONS = {
  {
    id = "BtnStart",
    text = "开始新游戏",
    panel = { 480, 432, 800, 496 },
    label = { 577, 449 },
  },
  {
    id = "BtnContinue",
    text = "继续江湖",
    panel = { 480, 504, 800, 568 },
    label = { 590, 521 },
  },
  {
    id = "BtnExit",
    text = "退出江湖",
    panel = { 480, 576, 800, 640 },
    label = { 590, 593 },
  },
}

-- 选中项（1 基，与焦点环同步）。
local selected = 1
-- 选中高亮的颜色（暖金，与按钮贴图同色系）。
local HIGHLIGHT = { 0.85, 0.72, 0.42, 0.30 }

function M.count()
  return #OPTIONS
end

function M.index()
  local index = nav.index(FOCUS)
  if index then
    selected = index
  end
  return selected
end

-- 当前选中项 id（`BtnStart` / `BtnContinue` / `BtnExit`）。
function M.selected_id()
  return OPTIONS[M.index()].id
end

-- 移动选中项（方向字符串 "up"/"down"）；返回是否移动。
function M.move_dir(direction)
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  selected = index
  return true
end

function M.select(index)
  if nav.select(FOCUS, index) then
    selected = index
    return true
  end
  return false
end

local function clear()
  for _, option in ipairs(OPTIONS) do
    ui.hide("title:hl:" .. option.id)
    ui.hide("title:label:" .. option.id)
  end
  ui.hide("title:hint")
end

-- 每帧输入：返回被触发的按钮 id（调用方转交 `on_click`）或 nil。
-- 顺序：取消（B/Esc）→ 方向/摇杆/数字直选 → 确认（A/空格），与游戏内面板一致。
function M.update()
  clear()
  local game = game_input()
  if input.action_pressed("cancel") then
    -- 标题界面的「取消」= 退出：直接落在最后一项并触发。
    M.select(#OPTIONS)
    return OPTIONS[#OPTIONS].id
  end
  local step = game.ui_step()
  if step == "up" then
    M.move_dir("up")
  elseif step == "down" then
    M.move_dir("down")
  end
  game.select_by_digit(function(index)
    M.select(index + 1)
  end, #OPTIONS)
  if input.action_pressed("confirm") then
    return M.selected_id()
  end
  return nil
end

-- 绘制选中高亮 + 标签（含选中箭头）+ 顶部操作说明。必须在场景 UI 之后画。
function M.render()
  local index = M.index()
  for position, option in ipairs(OPTIONS) do
    local x1, y1, x2, y2 = option.panel[1], option.panel[2], option.panel[3], option.panel[4]
    if position == index then
      ui.panel(
        "title:hl:" .. option.id,
        x1 - 6,
        y1 - 4,
        (x2 - x1) + 12,
        (y2 - y1) + 8,
        HIGHLIGHT[1],
        HIGHLIGHT[2],
        HIGHLIGHT[3],
        HIGHLIGHT[4]
      )
    else
      ui.hide("title:hl:" .. option.id)
    end
    local text = position == index and ("> " .. option.text) or option.text
    ui.text_at(
      "title:label:" .. option.id,
      option.label[1] - 22,
      option.label[2],
      text,
      1.0,
      24.0
    )
  end
  ui.text_at("title:hint", 16, 8, M.hint_line(), 1.0, 13.0)
  return true
end

-- 顶部操作说明：按当前设备二选一（与 HUD 同一套文案来源）。
function M.hint_line()
  local confirm = hints.binding_labels({ "confirm" })
  local cancel = hints.binding_labels({ "cancel" })
  local parts = {
    hints.is_gamepad() and "左摇杆/DPad 选择" or "W/S 或 方向键 选择",
  }
  if confirm then
    parts[#parts + 1] = confirm .. " 确认"
  end
  if cancel then
    parts[#parts + 1] = cancel .. " 退出"
  end
  return table.concat(parts, " · ")
end

-- 进入/离开门面时复位（场景切换会清空动态 UI，选中项也回到第一项）。
function M.reset()
  selected = 1
  nav.open(FOCUS, #OPTIONS, 1)
  nav.select(FOCUS, 1)
  return true
end

return M
