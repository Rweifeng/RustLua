-- title_menu.lua：主入口门面（main.scene）的手柄/键盘菜单导航（M20-09）。
--
-- 背景：`main.scene` 的三个按钮（BtnStart / BtnContinue / BtnExit）只有
-- `UiEvents event=click;function=on_click` —— 也就是**只有鼠标点击**通路，
-- 手柄与键盘在标题界面完全没有反应。本模块补齐这一层：
--
--   * 焦点环（`ui.panel_nav`）持有选中项，W/S、方向键 / DPad / 左摇杆 / 数字键直选都能改
--     选中；导航手感（摇杆阈值、按住重复）复用 `ui.input.ui_step()`，与游戏内面板
--     完全一致（不另写一份，避免手感分叉）；
--   * 选中项以金色高亮框 + 左侧箭头画在按钮上（`ui.panel` + `ui.text_at`，与暂停菜单
--     同款动态 UI 做法，不改场景文件）；**标签文字本身不画**——它由 `main.scene` 的
--     文字实体提供，脚本再画一遍会和场景文字叠成重影（M22 追加修复，见 `M.render`）；
--   * 确认（`confirm`：空格 / 手柄 A）与鼠标点击统一走场景脚本的 `on_click(name)`，
--     两条路径**同一个入口**，因此行为完全一致；
--   * 顶部显示当前设备对应的操作说明（`ui.hints`），与游戏内 HUD 同源；
--   * 手柄 B / Esc（cancel）在标题界面等于「选中退出江湖」。
local nav = require("ui.panel_nav")
local hints = require("ui.hints")
-- M30：文字统一走 `ui.ui_text`（裁剪 + 登记，验收遍历登记表逐条量宽）。
local ui_text = require("ui.ui_text")
-- `core.audio` 是叶子模块（只依赖 core.save_manager），顶层 require 不会成环；
-- 选中/确认/取消反馈音都经它（UI 总线）。
local audio = require("core.audio")
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

-- 与 `main.scene` 里按钮实体的 `UiLayout` 对齐（设计分辨率 1280x720）：
-- BtnStart 480,432,800,496 / BtnContinue 480,504,800,568 / BtnExit 480,576,800,640。
-- `label` 是场景**文字实体**（entity 8/10/12）的左上角：标签文字由场景画，脚本只在
-- 它的左侧留白里补选中箭头，因此两个坐标必须一起改（`tools/gen_main_menu_scene.py`）。
local OPTIONS = {
  {
    id = "BtnStart",
    panel = { 480, 432, 800, 496 },
    label = { 577, 449 },
  },
  {
    id = "BtnContinue",
    panel = { 480, 504, 800, 568 },
    label = { 590, 521 },
  },
  {
    id = "BtnExit",
    panel = { 480, 576, 800, 640 },
    label = { 590, 593 },
  },
}

-- 选中箭头及其左边距（像素）：箭头画在场景标签左边缘往左 `ARROW_GAP` 处，
-- 原先这里是 `"> " .. text` 的前缀位置，宽度约 19px，留 22px 不会压到标签。
local ARROW = ">"
local ARROW_GAP = 22

-- 选中项（1 基，与焦点环同步）。
local selected = 1
-- 选中高亮的颜色（暖金，与按钮贴图同色系）。
local HIGHLIGHT = { 0.85, 0.72, 0.42, 0.30 }

-- ---------- 选中反馈（M22） ----------
--
-- 手柄/键盘导航原先只有「高亮框换位置」这一个静态变化，按下去没有任何反馈，
-- 手感上像「没选中」。这里补两件事：
--   * 换项时高亮框**弹一下**（外扩 + 提亮，`POP_SECONDS` 内 ease-out 收回来）；
--   * 选中项**呼吸**（很轻的透明度起伏），让「当前选中哪一个」在静止时也一眼可见。
-- 都用固定步 `time.delta()` 推进，因此与渲染帧率无关，也和引擎固定步一致。
local POP_SECONDS = 0.14
local PULSE_SECONDS = 2.4
local pop_timer = 0
local pulse_phase = 0

-- 当前动画取值（`render` 与 headless 断言同源，避免「测试断言的公式和画面不一致」）。
-- `pop` 1 → 0（刚换项时为 1）；`grow_x/grow_y` 高亮框外扩像素；`alpha` 最终透明度。
function M.anim()
  local pop = POP_SECONDS > 0 and math.max(0, math.min(1, pop_timer / POP_SECONDS)) or 0
  local ease = 1 - (1 - pop) * (1 - pop)
  local breathe = 0.5 + 0.5 * math.cos(pulse_phase / PULSE_SECONDS * math.pi * 2)
  return {
    pop = pop,
    ease = ease,
    grow_x = 6 + 10 * ease,
    grow_y = 4 + 6 * ease,
    alpha = HIGHLIGHT[4] + 0.30 * ease + 0.06 * breathe,
  }
end

local function restart_pop()
  pop_timer = POP_SECONDS
end

local function advance_anim(delta)
  if delta and delta > 0 then
    pop_timer = math.max(0, pop_timer - delta)
    pulse_phase = (pulse_phase + delta) % PULSE_SECONDS
  end
end

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
  local before = M.index()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  selected = index
  if index ~= before then
    restart_pop()
  end
  return true
end

function M.select(index)
  local before = M.index()
  if nav.select(FOCUS, index) then
    selected = index
    if index ~= before then
      restart_pop()
    end
    return true
  end
  return false
end

local function clear()
  for _, option in ipairs(OPTIONS) do
    -- `ui.hide` 自 M22 起同时隐藏同 id 的动态面板与文本，因此高亮框、选中箭头一起收起
    --（此前它只认文本 → 非选中项的高亮框会残留）。`title:label:*` 是修复前脚本
    -- 自己画的标签（已改为只由场景绘制），这里一并收起，兼容热重载留下的旧状态。
    ui.hide("title:hl:" .. option.id)
    ui.hide("title:arrow:" .. option.id)
    ui.hide("title:label:" .. option.id)
  end
  ui.hide("title:hint")
end

-- 收起本模块画的所有动态 UI（离开门面时调用；场景切换也会清空动态 UI，这里是显式兜底）。
function M.hide()
  clear()
  return true
end

-- 每帧输入：返回被触发的按钮 id（调用方转交 `on_click`）或 nil。
-- 顺序：取消（B/Esc）→ W/S → 方向键/DPad/摇杆/数字直选 → 确认（A/空格），与游戏内面板一致。
function M.update()
  clear()
  local game = game_input()
  advance_anim(select(1, time.delta()))
  if input.action_pressed("cancel") then
    -- 标题界面的「取消」= 退出：直接落在最后一项并触发。
    audio.ui("ui_cancel")
    M.select(#OPTIONS)
    return OPTIONS[#OPTIONS].id
  end
  -- W/S（`move_up`/`move_down`）也要能导航：顶部提示写的是「W/S 或 方向键 选择」，
  -- 而面板（对话/商店/背包/任务/暂停）都是「先看 `move_*`，再退回 `ui_step()`」——
  -- 标题菜单此前只看 `ui_step()`（= `ui_up`/`ui_down`，默认绑方向键/DPad/左摇杆），
  -- 于是 W/S 在门面上**完全没反应**（提示与行为不一致）。
  -- 用 `else`：左摇杆同时驱动 `move_*` 与 `ui_*`，一次边沿只能走一步（避免双步）。
  if input.action_pressed("move_up") then
    M.move_dir("up")
  elseif input.action_pressed("move_down") then
    M.move_dir("down")
  else
    local step = game.ui_step()
    if step == "up" then
      M.move_dir("up")
    elseif step == "down" then
      M.move_dir("down")
    end
  end
  game.select_by_digit(function(index)
    M.select(index + 1)
  end, #OPTIONS)
  if input.action_pressed("confirm") then
    return M.selected_id()
  end
  return nil
end

-- 绘制选中高亮 + 选中箭头 + 顶部操作说明。必须在场景 UI 之后画。
--
-- **按钮文字不在这里画**（M22 追加修复的重影）：三行标签由**场景**绘制
-- （`main.scene` 的 entity 8/10/12，`UiLayout offsets` 577/590/590 即 `option.label`），
-- 脚本此前把同一串文字又画了一遍（选中项还多一个 `> ` 前缀），于是按钮上出现两层
-- 文字：不选中项**整体错开 22px**（`label_x` 的 -22 是给箭头预留的左边距）、选中项
-- 因为 `> ` 的前进宽度只有约 19px 而**只错开 3px**（看起来像重影/加粗）。两层的颜色
-- 也不同（脚本纯白 vs 场景米色），所以肉眼是「一明一暗两遍字」。
-- 现在标签文字单一来源 = 场景；脚本只补**选中箭头**（位置就是原先 `> ` 所在处）。
function M.render()
  local index = M.index()
  local anim = M.anim()
  ui_text.begin("title")
  for position, option in ipairs(OPTIONS) do
    local x1, y1, x2, y2 = option.panel[1], option.panel[2], option.panel[3], option.panel[4]
    if position == index then
      -- 换项时外扩 + 提亮（M22）：`grow_*` 是外扩像素，`alpha` 含呼吸项。
      ui.panel(
        "title:hl:" .. option.id,
        x1 - anim.grow_x,
        y1 - anim.grow_y,
        (x2 - x1) + anim.grow_x * 2,
        (y2 - y1) + anim.grow_y * 2,
        math.min(1.0, HIGHLIGHT[1] + 0.15 * anim.ease),
        math.min(1.0, HIGHLIGHT[2] + 0.15 * anim.ease),
        math.min(1.0, HIGHLIGHT[3] + 0.12 * anim.ease),
        math.min(1.0, anim.alpha)
      )
      -- 老版本由脚本自画的 `title:label:*`（已删除）若因热重载仍留着，一并收起，
      -- 否则它会作为第二层文字重新盖到场景标签上（正是这次修掉的重影）。
      ui.hide("title:label:" .. option.id)
      -- 选中箭头随弹跳轻微左移，与高亮框一起「推出来」；`ARROW_GAP` 让箭头落在
      -- 场景标签左边缘之外，与文字互不重叠。
      local arrow_x = option.label[1] - ARROW_GAP - 3.0 * anim.ease
      ui_text.draw_exact("title", "title:arrow:" .. option.id, arrow_x, option.label[2], 24.0, 48.0, ARROW)
    else
      ui.hide("title:hl:" .. option.id)
      ui.hide("title:arrow:" .. option.id)
      ui.hide("title:label:" .. option.id)
    end
  end
  ui_text.draw("title", "title:hint", 16, 8, 13.0, 1280 - 32, M.hint_line())
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
  pop_timer = 0
  pulse_phase = 0
  clear()
  nav.open(FOCUS, #OPTIONS, 1)
  nav.select(FOCUS, 1)
  return true
end

return M
