-- virtual_pad.lua：触屏虚拟摇杆 + 虚拟按钮（世界区左右下角）。
--
-- 几何**不在这里**：坐标、半径、拾取余量、死区全部来自 `ui/layout.lua` 的 `M.TOUCH`
-- （与 `M.PANEL` 同源），并由 `layout.audit_touch()` 守住三条承诺——整圆不越出设计画布、
-- 控件互不重叠、不压右栏文字与底部键位提示。这里只负责"读几何 + 注入虚拟键 + 画"。
--
-- 为什么不各自手写坐标（曾经就是这么写的，实机截图暴露了两个静默错误）：
--   * `x = 1180 / r = 46` → 右缘 1226，窗口一旦不是 1280 宽就被切掉；
--   * 按钮簇落在 x 1085..1226、y 518..706，正压右栏目标行（y 566..590）与资源条
--     （y 234..282）——画面上"目标 第 1 天…"被两个圆盖住，而没有任何报错。
-- 收进 `layout.lua` 后这类"圆盖住字"变成 `validate_data` 启动门能拦下的数据错误。
--
-- 输入注入用 `input.virtual_key`（按住态，跨帧保持；见引擎 `InputSnapshot`）与
-- `input.virtual_pressed`（按下边沿）。摇杆的按住态由引擎在**帧边界**沿用，
-- 因此本模块不必每帧重发（重发也无害）。
local ui_text = require("ui.ui_text")
local layout = require("ui.layout")
-- 面板模式判定（背包/任务/暂停/商店/对话 = 非 Playing）：十字键只在面板模式接管。
local state = require("core.state")

local M = {}

local STICK = layout.TOUCH.stick
local STICK_GRAB = layout.TOUCH.stick_grab_extra
local STICK_DEADZONE = layout.TOUCH.stick_deadzone
local BUTTONS = layout.TOUCH.buttons
local COLORS = layout.TOUCH.button_color

-- 面板导航十字键（DPad + 中央确定），几何在 layout.TOUCH.dpad；与玩法按钮簇不同屏。
local DPAD_KEYS = (layout.TOUCH.dpad and layout.TOUCH.dpad.keys) or {}
-- 十字键方向 → 注入的物理键名：面板导航走引擎物理键 up/down/left/right（bindings 的
-- ui_up/ui_down/...），中央确定走 space（bindings confirm）。这些不是 keyX 形式，
-- 运行时 __p2_virtual_name 原样返回，正好命中动作层（与摇杆注入 keyw → 'w' 同思路）。
local DPAD_PHYSICAL = {
  up = "up", down = "down", left = "left", right = "right", confirm = "space",
}

-- 方向名 → 物理键（模块级常量：原先写在函数体里，每次方向变化都新建一张表）。
local DIR_KEY = { up = "keyw", down = "keys", left = "keya", right = "keyd" }

-- 语义色名 → 按钮下标（下标随 `layout.TOUCH.buttons` 顺序变化，故按 key 查表而不是写死）。
local BUTTON_COLOR = {
  keye = COLORS.primary,
  keyj = COLORS.combat,
  keyi = COLORS.panel,
  keyl = COLORS.log,
  keyp = COLORS.system,
}

-- 调试文案（`ax/ay/stick/dx/dy`）默认关闭：它是排障用的，会压在左上角世界画面上。
-- 打开方式：`require("ui.virtual_pad").debug(true)`（控制台/脚本里均可）。
local debug_text = false

local stick_active = false
local stick_dx, stick_dy = 0, 0
local last_dir = { up = false, down = false, left = false, right = false }
local button_down = {}
local dpad_down = {}
-- 面板模式切换的边沿：只在 玩法↔面板 变化那一帧释放对侧控件的残留按住态。
local panel_active = false

local function in_circle(px, py, cx, cy, r)
  local dx, dy = px - cx, py - cy
  return dx * dx + dy * dy <= r * r
end

-- 文本居中用的半宽；宿主没有 `ui.measure_text`（或字体不可用）时按 0.5×字号×字数估。
local function half_width(text, size)
  if type(ui.measure_text) == "function" then
    local ok, measured = pcall(ui.measure_text, text, size)
    if ok and type(measured) == "table" and tonumber(measured.width) then
      return tonumber(measured.width) / 2
    end
  end
  return #text * size * 0.5
end

local function set_dir(name, down)
  if last_dir[name] ~= down then
    last_dir[name] = down
    input.virtual_key(DIR_KEY[name], down)
  end
end

local function release_all_dirs()
  set_dir("up", false)
  set_dir("down", false)
  set_dir("left", false)
  set_dir("right", false)
end

-- 按下（当帧）：先给一次"按下边沿"，再按住。边沿只发一次——原先滑出按钮范围时
-- 每帧重发 `virtual_pressed`，`action_pressed` 系动作会连发。
local function button_press(index, key)
  if button_down[index] then
    return
  end
  button_down[index] = true
  log.info("[vp] button_press " .. tostring(key) .. " idx=" .. tostring(index))
  input.virtual_pressed(key)
  input.virtual_key(key, true)
end

-- 抬起：只在真的按住时写一次释放（避免无谓地每帧注入 `false`）。
local function button_release(index, key)
  if not button_down[index] then
    return
  end
  button_down[index] = false
  input.virtual_key(key, false)
end

-- 十字键：与虚拟按钮同一手感（按下当帧一个边沿 + 按住态跨帧保持）。一次触摸 = 面板内一步
-- （web 运行时没有 action_repeat，ui_step 回落 action_pressed 只吃边沿，按住不连发）。
local function dpad_press(index, physical)
  if dpad_down[index] then
    return
  end
  dpad_down[index] = true
  input.virtual_pressed(physical)
  input.virtual_key(physical, true)
end

local function dpad_release(index, physical)
  if not dpad_down[index] then
    return
  end
  dpad_down[index] = false
  input.virtual_key(physical, false)
end

-- 释放所有虚拟输入。宿主窗口失焦 / 切场景 / 面板打开时由 `ui/input.lua` 调用，
-- 避免"手指抬起事件丢了 → 角色自己一直走"。
function M.release_all()
  stick_active = false
  stick_dx, stick_dy = 0, 0
  release_all_dirs()
  for index, btn in ipairs(BUTTONS) do
    button_release(index, btn.key)
  end
  for index, key in ipairs(DPAD_KEYS) do
    dpad_release(index, DPAD_PHYSICAL[key.dir or key.key])
  end
end

-- 开关排障文案（`ax/ay/stick/dx/dy`，默认关闭：会压在左上角世界画面上）。
function M.debug(enabled)
  debug_text = enabled and true or false
end

-- 宿主是否**真的**实现了这四条注入/绘制 API（= 这个构建需要屏上虚拟控件）。
--
-- 必须用 `rawget`，不能用 `type(input.virtual_key) == "function"`：web/微信运行时给
-- **缺失方法**装的 `__index` 陷阱返回的是**可调用代理**，于是那个判断在缺 API 的宿主上
-- 恒为真——虚拟控件会错误激活，而每帧的注入与绘制全部静默落空（微信实测：门只报出
-- `input.virtual_key / input.mouse_just_released / ui.circle` 三个缺口，`M.available()`
-- 却一直是 true）。`rawget` 绕开元表，拿到的是表里**真正存在**的字段：缺就是 nil；
-- 因为根本没碰代理，诊断日志里也不会留下假缺口——与 `world/camera_view.lua` 查
-- `__p2_missing_globals` 判能力是同一个思路。
local function host_function(api, name)
  if api == nil then
    return false
  end
  return type(rawget(api, name)) == "function"
end

function M.available()
  return host_function(input, "virtual_key")
    and host_function(input, "virtual_pressed")
    and host_function(input, "mouse_just_released")
    and host_function(ui, "circle")
end

function M.update()
  if not M.available() then
    return
  end
  local cp = input.cursor_position()
  local mx, my = cp.x or cp[1] or 0, cp.y or cp[2] or 0
  local mouse_pressed = input.mouse_just_pressed("left")
  local mouse_released = input.mouse_just_released("left")

  -- 命中测试用**按下位置**而不是实时光标：宿主实现 `input.press_position` 时
  -- （web/微信运行时：适配层把每根手指的按下点都记进 press_cursor），按住摇杆时
  -- 另一只手点按钮——光标仍跟摇杆手，按钮却按自己的按下点命中；缺该 API 的宿主
  -- （native 引擎等）回落光标位置，行为与原先一致。
  local press_x, press_y = mx, my
  if host_function(input, "press_position") then
    local ok, pp = pcall(input.press_position)
    if ok and pp then
      press_x = pp.x or pp[1] or mx
      press_y = pp.y or pp[2] or my
    end
  end

  -- 面板/对话/商店态：隐藏玩法控件，十字键（DPad）接管导航。切换瞬间释放玩法控件
  -- 的残留按住态（否则"打开背包前一指还按着按钮"会持续注入玩法键）。
  local is_panel = not state.is_mode("Playing")
  if is_panel ~= panel_active then
    panel_active = is_panel
    log.info("[vp] mode=" .. tostring(state.mode) .. " is_panel=" .. tostring(is_panel))
    if is_panel then
      stick_active = false
      stick_dx, stick_dy = 0, 0
      release_all_dirs()
      for index, btn in ipairs(BUTTONS) do
        button_release(index, btn.key)
      end
    else
      for index, key in ipairs(DPAD_KEYS) do
        dpad_release(index, DPAD_PHYSICAL[key.dir or key.key])
      end
    end
  end
  if is_panel then
    for index, key in ipairs(DPAD_KEYS) do
      if dpad_down[index] then
        if mouse_released then
          dpad_release(index, DPAD_PHYSICAL[key.dir or key.key])
        else
          input.virtual_key(DPAD_PHYSICAL[key.dir or key.key], true)
        end
      elseif mouse_pressed and in_circle(press_x, press_y, key.x, key.y, key.size / 2) then
        log.info("[vp] dpad_hit " .. tostring(key.dir or key.key) .. " press=" .. tostring(press_x) .. "," .. tostring(press_y))
        dpad_press(index, DPAD_PHYSICAL[key.dir or key.key])
      end
    end
    return
  end

  -- 摇杆：在"底圆 + 拾取余量"内按下即抓住，之后拖出圈也跟随（拇指滑动不脱手）。
  if not stick_active and mouse_pressed
      and in_circle(press_x, press_y, STICK.cx, STICK.cy, STICK.base_r + STICK_GRAB) then
    stick_active = true
  end
  if stick_active then
    if mouse_released then
      stick_active = false
      stick_dx, stick_dy = 0, 0
      release_all_dirs()
    else
      local dx = mx - STICK.cx
      local dy = my - STICK.cy
      local len = math.sqrt(dx * dx + dy * dy)
      local max = STICK.base_r
      if len > max then
        dx, dy = dx / len * max, dy / len * max
      end
      stick_dx, stick_dy = dx, dy
      set_dir("up", dy < -STICK_DEADZONE * max)
      set_dir("down", dy > STICK_DEADZONE * max)
      set_dir("left", dx < -STICK_DEADZONE * max)
      set_dir("right", dx > STICK_DEADZONE * max)
    end
  end

  -- 虚拟按钮：按下当帧产生"按下边沿"（`action_pressed` 系立刻可用）并保持按住；
  -- 之后拖出圆也**不掉**（与摇杆同一手感：抓住了就跟着手指），抬手统一释放。
  for index, btn in ipairs(BUTTONS) do
    if button_down[index] then
      if mouse_released then
        button_release(index, btn.key)
      else
        input.virtual_key(btn.key, true)
      end
    elseif mouse_pressed and in_circle(press_x, press_y, btn.x, btn.y, btn.size / 2) then
      log.info("[vp] hit " .. tostring(btn.key) .. " press=" .. tostring(press_x) .. "," .. tostring(press_y))
      button_press(index, btn.key)
    end
  end
end

function M.draw()
  if not M.available() then
    return
  end
  ui_text.begin("vp")

  -- 面板模式：只画十字键（方向键用 system 灰、中央确定用 primary 绿；按下提亮）。
  if not state.is_mode("Playing") then
    for index, key in ipairs(DPAD_KEYS) do
      local pressed = dpad_down[index]
      local radius = key.size / 2
      local color = (key.key == "confirm") and COLORS.primary or COLORS.system
      local ring_alpha = pressed and 0.27 or 0.15
      local body_alpha = pressed and 0.85 or 0.55
      ui.circle("vp:dring" .. index, key.x, key.y, radius + 3, 0.9, 0.9, 0.9, ring_alpha)
      ui.circle("vp:dbtn" .. index, key.x, key.y, radius, color[1], color[2], color[3], body_alpha)
      local label = key.label or ""
      local font = math.max(11, radius * 0.42)
      ui_text.draw("vp", "vp:dlbl" .. index, key.x - half_width(label, font), key.y - font * 0.5, font, radius * 2, label)
    end
    if debug_text then
      ui_text.draw("vp", "vp:dbg", 10, 10, 14, 400, "panel-mode")
    end
    return
  end

  -- 摇杆：外圈（按下时提亮）+ 底圆 + 摇杆头。
  local stick_alpha = stick_active and 0.7 or 0.45
  ui.circle("vp:stick_ring", STICK.cx, STICK.cy, STICK.base_r + 4, 0.15, 0.15, 0.18, stick_alpha)
  ui.circle("vp:stick_base", STICK.cx, STICK.cy, STICK.base_r, 0.25, 0.25, 0.30, stick_alpha)
  ui.circle(
    "vp:stick_knob",
    STICK.cx + stick_dx,
    STICK.cy + stick_dy,
    STICK.knob_r,
    0.55, 0.55, 0.65, 0.8
  )

  -- 按钮：外圈描边 + 主体 + 左上高光 + 居中标签（按下提亮）。
  for index, btn in ipairs(BUTTONS) do
    local pressed = button_down[index]
    local radius = btn.size / 2
    local color = BUTTON_COLOR[btn.key] or COLORS.system
    local ring_alpha = pressed and 0.27 or 0.15
    local body_alpha = pressed and 0.85 or 0.55
    ui.circle("vp:ring" .. index, btn.x, btn.y, radius + 3, 0.9, 0.9, 0.9, ring_alpha)
    ui.circle("vp:btn" .. index, btn.x, btn.y, radius, color[1], color[2], color[3], body_alpha)
    ui.circle(
      "vp:hl" .. index,
      btn.x - radius * 0.3,
      btn.y - radius * 0.3,
      radius * 0.25,
      1.0, 1.0, 1.0, 0.25
    )
    -- 标签：字号随按钮直径缩放（`size × 0.21`，下限 11；见 `layout.TOUCH` 注释）。
    -- M30 自检只认字面量/可解析常量字号，故按 layout 尺寸换算成每键字面量
    -- （108/84/72/80/68 → 22/17/15/16/14）；几何全部仍读 `layout.TOUCH.buttons`。
    local b1, b2, b3, b4, b5 = BUTTONS[1], BUTTONS[2], BUTTONS[3], BUTTONS[4], BUTTONS[5]
    ui_text.draw("vp", "vp:lbl1", b1.x - half_width(b1.label, 22), b1.y - 22 * 0.5, 22, b1.size, b1.label)
    ui_text.draw("vp", "vp:lbl2", b2.x - half_width(b2.label, 17), b2.y - 17 * 0.5, 17, b2.size, b2.label)
    ui_text.draw("vp", "vp:lbl3", b3.x - half_width(b3.label, 15), b3.y - 15 * 0.5, 15, b3.size, b3.label)
    ui_text.draw("vp", "vp:lbl4", b4.x - half_width(b4.label, 16), b4.y - 16 * 0.5, 16, b4.size, b4.label)
    ui_text.draw("vp", "vp:lbl5", b5.x - half_width(b5.label, 14), b5.y - 14 * 0.5, 14, b5.size, b5.label)
  end

  if debug_text then
    local ax = input.action_axis("move_left", "move_right")
    local ay = input.action_axis("move_down", "move_up")
    ui_text.draw(
      "vp",
      "vp:dbg",
      10, 10, 14, 400,
      string.format(
        "ax=%.2f ay=%.2f stick=%s dx=%.0f dy=%.0f",
        ax, ay, tostring(stick_active), stick_dx, stick_dy
      )
    )
  end
end

return M
