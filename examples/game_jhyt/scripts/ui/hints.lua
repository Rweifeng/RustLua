-- hints.lua：键鼠 / 手柄两套操作说明与菜单导航手感（M20-09）。
--
-- 为什么单独一个模块：顶部 HUD 提示在各模式下由 `ui.input` 画，但**面板底部提示**
-- 由各面板自己画（`world.dialogue`、`systems.shop`、`ui.inventory_ui`、`ui.quest_ui`、
-- `ui.pause_menu`）。这些面板不能 require `ui.input`（`input → dialogue` 会成环），
-- 因此把「设备判定 + 键位命名 + 提示组装 + 导航手感」放在这里：它只依赖引擎的
-- `input` API，不依赖任何面板，谁都能安全引用。
--
-- 两件事：
--   * 设备判定：按**最近使用的设备**在键鼠/手柄两套说明间切换（推摇杆/按手柄键 →
--     手柄说明；动键盘/鼠标 → 键鼠说明）。只看「哪个输入在动」，不看手柄是否已连接，
--     所以插着手柄用键盘玩仍显示键鼠说明。
--   * 导航手感：摇杆阈值与「按住后首次延迟 + 重复间隔」可从工程 `settings.toml` 的
--     `[input]` 段配置（`nav_threshold` / `nav_delay_frames` / `nav_repeat_frames`），
--     由 `ui.input.apply_nav_settings()` 在启动时应用，`ui.input.ui_step()` 每帧读取。
local M = {}

local DEVICE_KEYBOARD = "keyboard"
local DEVICE_GAMEPAD = "gamepad"
-- 初始 `nil` = 尚未检测；首个动过的设备决定提示，都没动过时按键鼠显示。
local active_device = nil
-- 摇杆噪声容限：低于它不算「在用摇杆」（与导航阈值默认 0.5 区分开）。
local DEVICE_AXIS_THRESHOLD = 0.35

local KEYBOARD_KEY_NAMES = {
  space = "空格",
  escape = "Esc",
  up = "↑",
  down = "↓",
  left = "←",
  right = "→",
}

-- 手柄按键中文名（与编辑器「项目设置 → 输入」面板的 `gamepad_button_name` 同源，
-- 玩家在两边看到的是同一套名字）。
local PAD_BUTTONS = {
  "A",
  "B",
  "X",
  "Y",
  "LB",
  "RB",
  "LT",
  "RT",
  "Back",
  "Start",
  "L3",
  "R3",
  "DPad↑",
  "DPad↓",
  "DPad←",
  "DPad→",
}

-- 手柄轴中文名（契约 0..5：左右摇杆 X/Y、LT/RT）。摇杆方向按正负拆成动作，
-- 因此轴绑定一律显示摇杆名（"左摇杆"/"右摇杆"/"LT 扳机"…）。
local PAD_AXES = {
  "左摇杆",
  "左摇杆",
  "右摇杆",
  "右摇杆",
  "LT 扳机",
  "RT 扳机",
}

-- ---------- 导航手感（可在 settings.toml 的 [input] 段配置） ----------

local NAV_DEFAULTS = { threshold = 0.5, delay_frames = 16, repeat_frames = 8 }
local NAV_MAX_FRAMES = 120
local nav = {
  threshold = NAV_DEFAULTS.threshold,
  delay_frames = NAV_DEFAULTS.delay_frames,
  repeat_frames = NAV_DEFAULTS.repeat_frames,
}

-- 把 `input.nav_settings()`（= 工程 settings.toml 的 [input] 段）应用到导航手感。
-- 引擎未提供该 API（旧引擎）或文件缺失/非法时保持默认；返回生效值。
function M.apply_nav_settings()
  if type(input.nav_settings) == "function" then
    local ok, values = pcall(input.nav_settings)
    if ok and type(values) == "table" then
      local threshold = tonumber(values.threshold)
      if threshold and threshold >= 0 and threshold <= 1 then
        nav.threshold = threshold
      end
      local delay = tonumber(values.delay_frames)
      if delay then
        nav.delay_frames = math.max(0, math.min(NAV_MAX_FRAMES, math.floor(delay)))
      end
      local repeat_frames = tonumber(values.repeat_frames)
      if repeat_frames then
        nav.repeat_frames = math.max(1, math.min(NAV_MAX_FRAMES, math.floor(repeat_frames)))
      end
    end
  end
  return M.nav_settings()
end

-- 当前导航手感（只读副本）。
function M.nav_settings()
  return {
    threshold = nav.threshold,
    delay_frames = nav.delay_frames,
    repeat_frames = nav.repeat_frames,
  }
end

function M.nav_threshold()
  return nav.threshold
end

function M.nav_delay_frames()
  return nav.delay_frames
end

function M.nav_repeat_frames()
  return nav.repeat_frames
end

-- ---------- 设备判定 ----------

-- 一次全量扫描：任一键盘键按下 → 键盘；任一非零轴/按住按钮 → 手柄。
-- 返回值：`"keyboard"` / `"gamepad"` / `nil`（本帧两者都没动，保持上次选择）。
-- 动作表只在启动 `input.load_settings()` 后变化，缓存一次避免每帧重建 Lua 表。
local cached_actions = nil

local function all_actions()
  if not cached_actions then
    cached_actions = input.actions()
  end
  return cached_actions
end

local function detect_device()
  local pad_axis = input.pad_axis
  if type(pad_axis) == "function" then
    for index = 0, 5 do
      local value = tonumber(pad_axis(index)) or 0
      if math.abs(value) >= DEVICE_AXIS_THRESHOLD then
        return DEVICE_GAMEPAD
      end
    end
    local pad_button = input.pad_button
    if type(pad_button) == "function" then
      for index = 0, 15 do
        if pad_button(index) then
          return DEVICE_GAMEPAD
        end
      end
    end
  end
  for _, action in ipairs(all_actions()) do
    for _, text in ipairs(input.bindings(action) or {}) do
      local key = tostring(text):match("^key:(.+)$")
      if key and input.is_down(key) then
        return DEVICE_KEYBOARD
      end
    end
  end
  return nil
end

-- 当前生效的操作说明设备（每帧刷新；未检测到输入时保持上一次）。
function M.device()
  local detected = detect_device()
  if detected then
    active_device = detected
  end
  return active_device or DEVICE_KEYBOARD
end

-- 测试/调试用：强制某套说明（`"keyboard"`/`"gamepad"`），`nil` 恢复自动检测。
function M.set_device(device)
  if device == DEVICE_KEYBOARD or device == DEVICE_GAMEPAD then
    active_device = device
  else
    active_device = nil
  end
  return M.device()
end

function M.is_gamepad()
  return M.device() == DEVICE_GAMEPAD
end

-- ---------- 绑定 → 可读名字 ----------

function M.binding_hint(text)
  local value = tostring(text or "")
  local key = value:match("^key:(.+)$")
  if key then
    if KEYBOARD_KEY_NAMES[key] then
      return KEYBOARD_KEY_NAMES[key]
    end
    local code = key:match("^other:(%d+)$")
    if code then
      return "键" .. code
    end
    return key:upper()
  end
  local pad = value:match("^pad:(%d+)$")
  if pad then
    return PAD_BUTTONS[tonumber(pad) + 1] or ("手柄" .. pad)
  end
  local axis, sign = value:match("^padaxis:(%d+)([%+%-])$")
  if axis then
    local name = PAD_AXES[tonumber(axis) + 1] or ("摇杆" .. axis)
    return name .. (sign == "+" and "→/↑" or "←/↓")
  end
  return value
end

local function is_gamepad_binding(text)
  local value = tostring(text or "")
  return value:match("^pad:") ~= nil or value:match("^padaxis:") ~= nil
end

local function is_keyboard_binding(text)
  local value = tostring(text or "")
  return value:match("^key:") ~= nil or value:match("^mouse:") ~= nil
end

-- 动作在指定设备下的按键名：该设备没有绑定 → nil（**不从另一套回退**，
-- 否则手柄说明里会混进 "E" 这类键盘标签）。
function M.binding_label(action, device)
  if action == "volume_up" then
    -- 音量成对出现，单独显示 "+" 会误导（`-` 是反向动作），面板文案里直接写 +/-。
    return "+/-"
  end
  local target = device or M.device()
  for _, text in ipairs(input.bindings(action) or {}) do
    if target == DEVICE_GAMEPAD then
      if is_gamepad_binding(text) then
        return M.binding_hint(text)
      end
    elseif is_keyboard_binding(text) then
      return M.binding_hint(text)
    end
  end
  return nil
end

-- 多个动作的按键名连接成一段；`separator` 缺省 " / "。全无绑定返回 nil。
function M.binding_labels(actions, device, separator)
  local labels = {}
  for _, action in ipairs(actions or {}) do
    local label = M.binding_label(action, device)
    if label then
      labels[#labels + 1] = label
    end
  end
  if #labels == 0 then
    return nil
  end
  return table.concat(labels, separator or " / ")
end

-- ---------- 顶部提示 ----------

-- 两套说明的控制项。`actions` 里的动作按可改绑的**动作**列出（改键后提示自动同步）；
-- 手柄的"移动"没有对应动作（默认只绑了菜单导航 ui_*，玩法移动 padaxis:* 由玩家改绑），
-- 因此单独用 `literal_gamepad` 给出契约名称（左摇杆/DPad），改绑后仍按实际绑定显示。
local HINT_GROUPS = {
  keyboard = {
    { label = "移动", actions = { "move_up", "move_left", "move_down", "move_right" }, separator = "/" },
    { label = "交互", actions = { "interact" } },
    { label = "攻击", actions = { "attack" } },
    { label = "武学", actions = { "skill" } },
    { label = "送礼", actions = { "gift" } },
    { label = "睡觉", actions = { "sleep" } },
    { label = "修炼", actions = { "practice" } },
    { label = "建造", actions = { "build" } },
    { label = "背包", actions = { "inventory" } },
    { label = "任务", actions = { "quest" } },
    { label = "暂停", actions = { "pause" } },
  },
  gamepad = {
    { label = "移动", literal_gamepad = "左摇杆/DPad" },
    { label = "交互", actions = { "interact" } },
    { label = "攻击", actions = { "attack" } },
    { label = "武学", actions = { "skill" } },
    { label = "背包", actions = { "inventory" } },
    { label = "任务", actions = { "quest" } },
    { label = "暂停", actions = { "pause" } },
    { label = "确认", actions = { "confirm" } },
    { label = "取消", actions = { "cancel" } },
  },
}

-- 指定设备的单行操作说明；该设备没有任何绑定 → 空串（调用方回退）。
function M.hint_for_device(device)
  local groups = HINT_GROUPS[device]
  if not groups then
    return ""
  end
  local parts = {}
  for _, entry in ipairs(groups) do
    local labels = {}
    if entry.literal_gamepad and device == DEVICE_GAMEPAD then
      labels[1] = entry.literal_gamepad
    else
      for _, action in ipairs(entry.actions or {}) do
        local label = M.binding_label(action, device)
        if label and not labels[label] then
          labels[#labels + 1] = label
        end
      end
    end
    if #labels > 0 then
      parts[#parts + 1] = table.concat(labels, entry.separator or "") .. " " .. entry.label
    end
  end
  return table.concat(parts, " · ")
end

-- 单行键位提示（当前设备优先；该设备无绑定则回退另一套；都为空给兜底文案）。
function M.key_hints(device)
  local target = device or M.device()
  local other = DEVICE_KEYBOARD
  if target == DEVICE_KEYBOARD then
    other = DEVICE_GAMEPAD
  end
  local text = M.hint_for_device(target)
  if text == "" then
    text = M.hint_for_device(other)
  end
  if text == "" then
    return "无可用按键绑定（可在项目设置 → 输入里改键）"
  end
  return text
end

-- 面板底部提示：**同一句文案按设备二选一**。
-- `pairs` = `{ keyboard = "…", gamepad = "…" }`；当前设备没有对应文案时回落
-- `fallback`（缺省为空串）。
function M.panel_hint(pairs, fallback)
  if type(pairs) ~= "table" then
    return fallback or ""
  end
  local text = pairs[M.device()]
  if type(text) ~= "string" or text == "" then
    text = fallback
  end
  return text or ""
end

-- 面板提示的常用片段拼装。
-- `spec` = `{ { keyboard = "W/S", gamepad = "左摇杆", label = "选择",
--               alt = " 或 ", alt_keyboard = "方向键" }, ... }`；
-- 当前设备没有对应片段时跳过该项；`alt_keyboard`/`alt_gamepad` 是该设备下的等价输入，
-- 用 `alt` 连接（例如键盘「W/S 或 方向键」）。
function M.panel_line(spec, separator)
  local device = M.device()
  local parts = {}
  for _, entry in ipairs(spec or {}) do
    local verb = entry[device]
    if type(verb) == "string" and verb ~= "" then
      local alternative = entry["alt_" .. device]
      if type(alternative) == "string" and alternative ~= "" then
        verb = verb .. tostring(entry.alt or " 或 ") .. alternative
      end
      parts[#parts + 1] = verb .. " " .. tostring(entry.label or "")
    end
  end
  return table.concat(parts, separator or " · ")
end

return M
