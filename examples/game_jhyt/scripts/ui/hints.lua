-- hints.lua：键鼠 / 手柄两套操作说明与菜单导航手感（M20-09 → M22）。
--
-- 为什么单独一个模块：顶部 HUD 提示在各模式下由 `ui.input` 画，但**面板底部提示**
-- 由各面板自己画（`world.dialogue`、`systems.shop`、`ui.inventory_ui`、`ui.quest_ui`、
-- `ui.pause_menu`）。这些面板不能 require `ui.input`（`input → dialogue` 会成环），
-- 因此把「设备判定 + 键位命名 + 提示组装 + 导航手感」放在这里：它只依赖引擎的
-- `input` API，不依赖任何面板，谁都能安全引用。
--
-- 两件事：
--   * 设备判定：引擎在**真实按下边沿**时记录最近使用的设备
--     （`input.last_device()` → `"keyboard"` / `"gamepad"`）。M22 前这里是每帧扫描
--     36 个动作的绑定 + 6 轴 + 16 按钮（约 60 次 FFI + 100 次正则），现在是一次查询。
--     只看「哪个输入在动」，不看手柄是否已连接，所以插着手柄用键盘玩仍显示键鼠说明。
--   * 导航手感：摇杆阈值与「按住后首次延迟 + 重复间隔」可从工程 `settings.toml` 的
--     `[input]` 段配置（`nav_threshold` / `nav_delay_ms` / `nav_repeat_ms`；旧的
--     `nav_delay_frames` / `nav_repeat_frames` 仍可读，按 60Hz 换算）。M22 起重复节奏
--     由**引擎**按毫秒执行（`input.action_repeat`），因此 144Hz 与 60Hz 手感一致。
local M = {}

local DEVICE_KEYBOARD = "keyboard"
local DEVICE_GAMEPAD = "gamepad"
-- 强制说明设备（测试/调试用）；`nil` = 自动跟随引擎记录的「最近使用的设备」。
local forced_device = nil

-- M22/W1：键位文本是**物理键名**（`key:keye`、`key:digit1`、`key:arrowup`、`key:shiftleft`），
-- 与 winit 的 `KeyCode` 变体名同名，所以这张表按**物理键名**索引。
-- 首次改键前，命中字母/数字分支的键只有 W/A/S/D/J/K/G/T/Q/B/F/X/U/L/R/C/I/P 与 1..9
-- （默认绑定表），其余条目是为「玩家改绑到任意键」准备的。
local KEYBOARD_KEY_NAMES = {
  space = "空格",
  escape = "Esc",
  enter = "回车",
  tab = "Tab",
  backspace = "退格",
  arrowup = "↑",
  arrowdown = "↓",
  arrowleft = "←",
  arrowright = "→",
  shiftleft = "Shift 左",
  shiftright = "Shift 右",
  controlleft = "Ctrl 左",
  controlright = "Ctrl 右",
  altleft = "Alt 左",
  altright = "Alt 右",
  superleft = "Win 左",
  superright = "Win 右",
  minus = "-",
  equal = "=",
  semicolon = ";",
  quote = "'",
  comma = ",",
  period = ".",
  slash = "/",
  backslash = "\\",
  intlbackslash = "\\ (ISO)",
  intlro = "ろ",
  intlyen = "¥",
  backquote = "`",
  bracketleft = "[",
  bracketright = "]",
  capslock = "CapsLock",
  numlock = "NumLock",
  scrolllock = "ScrollLock",
  printscreen = "PrtSc",
  pause = "Pause",
  contextmenu = "菜单键",
  insert = "Ins",
  delete = "Del",
  home = "Home",
  -- `end` 是 Lua 关键字，只能写成方括号形式（`delete`/`insert` 同理为求一致也保留裸名）。
  ["end"] = "End",
  pageup = "PgUp",
  pagedown = "PgDn",
  numpadadd = "小键盘 +",
  numpadsubtract = "小键盘 -",
  numpadmultiply = "小键盘 *",
  numpaddivide = "小键盘 /",
  numpaddecimal = "小键盘 .",
  numpadcomma = "小键盘 ,",
  numpadequal = "小键盘 =",
  numpadenter = "小键盘回车",
}

-- 变体名 → 可读名的**推导规则**（不枚举 194 项）：`keye`→`E`、`digit1`→`1`、
-- `f35`→`F35`、`numpad1`→`小键盘 1`、`lang1`→`Lang1`、`intlro`→`IntlRo`。
-- 未识别的形状原样 `upper()`，宁可显示得笨也不显示错。
local function derive_key_name(key)
  local letter = key:match("^key([a-z])$")
  if letter then
    return letter:upper()
  end
  local digit = key:match("^digit([0-9])$")
  if digit then
    return digit
  end
  local numpad = key:match("^numpad([0-9])$")
  if numpad then
    return "小键盘 " .. numpad
  end
  local fn = key:match("^f([0-9]+)$")
  if fn then
    return "F" .. fn
  end
  if key:match("^lang[0-9]+$") or key:match("^intl") then
    return key:sub(1, 1):upper() .. key:sub(2)
  end
  return key:upper()
end

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

-- M22：重复节奏用**毫秒**（旧帧键按 60Hz 换算）；默认 350ms / 90ms，与 M20-09 的
-- 「16 帧 / 8 帧 @60Hz」几乎一致，但不再随帧率漂移。
local NAV_DEFAULTS = { threshold = 0.5, delay_ms = 350, repeat_ms = 90 }
local NAV_MAX_MS = 5000
local NAV_MIN_REPEAT_MS = 16
local nav = {
  threshold = NAV_DEFAULTS.threshold,
  delay_ms = NAV_DEFAULTS.delay_ms,
  repeat_ms = NAV_DEFAULTS.repeat_ms,
}

-- 把 `input.nav_settings()`（= 工程 settings.toml 的 [input] 段）应用到导航手感，
-- 并**推给引擎**（`input.set_nav_settings`）：阈值成为设备层轴判定阈值，重复节奏供
-- `input.action_repeat` 使用。引擎未提供这些 API（旧引擎）或文件缺失/非法时保持默认。
-- 返回生效值。
function M.apply_nav_settings()
  if type(input.nav_settings) == "function" then
    local ok, values = pcall(input.nav_settings)
    if ok and type(values) == "table" then
      local threshold = tonumber(values.threshold)
      if threshold and threshold >= 0 and threshold <= 1 then
        nav.threshold = threshold
      end
      local delay = tonumber(values.delay_ms)
      if not delay then
        -- 旧字段（帧）→ 毫秒，兼容旧引擎/旧设置文本。
        local frames = tonumber(values.delay_frames)
        delay = frames and frames * 1000 / 60 or nil
      end
      if delay then
        nav.delay_ms = math.max(0, math.min(NAV_MAX_MS, math.floor(delay)))
      end
      local repeat_ms = tonumber(values.repeat_ms)
      if not repeat_ms then
        local frames = tonumber(values.repeat_frames)
        repeat_ms = frames and frames * 1000 / 60 or nil
      end
      if repeat_ms then
        nav.repeat_ms = math.max(NAV_MIN_REPEAT_MS, math.min(NAV_MAX_MS, math.floor(repeat_ms)))
      end
    end
  end
  if type(input.set_nav_settings) == "function" then
    pcall(input.set_nav_settings, nav.threshold, nav.delay_ms, nav.repeat_ms)
  end
  return M.nav_settings()
end

-- 当前导航手感（只读副本）。
function M.nav_settings()
  return {
    threshold = nav.threshold,
    delay_ms = nav.delay_ms,
    repeat_ms = nav.repeat_ms,
  }
end

function M.nav_threshold()
  return nav.threshold
end

function M.nav_delay_ms()
  return nav.delay_ms
end

function M.nav_repeat_ms()
  return nav.repeat_ms
end

-- ---------- 设备判定 ----------

-- 最近一次产生真实输入的设备（引擎在边沿发生时记录，见 M22/W3）。
-- 未动过任何输入 / 引擎缺少该 API 时按键鼠显示（旧引擎兜底）。
function M.device()
  if forced_device then
    return forced_device
  end
  if type(input.last_device) == "function" then
    local ok, device = pcall(input.last_device)
    if ok and device == DEVICE_GAMEPAD then
      return DEVICE_GAMEPAD
    end
  end
  return DEVICE_KEYBOARD
end

-- 测试/调试用：强制某套说明（`"keyboard"`/`"gamepad"`），`nil` 恢复自动检测。
function M.set_device(device)
  if device == DEVICE_KEYBOARD or device == DEVICE_GAMEPAD then
    forced_device = device
  else
    forced_device = nil
  end
  return M.device()
end

function M.is_gamepad()
  return M.device() == DEVICE_GAMEPAD
end

-- ---------- 绑定 → 可读名字 ----------

-- v1 的可读名（`key:up`/`key:space`/`key:esc`/单字符）只在**兼容入口**出现
-- （旧设置文件、老脚本的 `input.rebind`），用来把老写法映射到同一套显示名。
local V1_KEY_ALIASES = {
  up = "arrowup",
  down = "arrowdown",
  left = "arrowleft",
  right = "arrowright",
  esc = "escape",
  ["return"] = "enter",
  ["+"] = "equal",
  ["-"] = "minus",
  ["="] = "equal",
  [":"] = "semicolon",
  [";"] = "semicolon",
  ["'"] = "quote",
  ['"'] = "quote",
  [","] = "comma",
  ["<"] = "comma",
  ["."] = "period",
  [">"] = "period",
  ["/"] = "slash",
  ["?"] = "slash",
  ["\\"] = "backslash",
  ["|"] = "backslash",
  ["`"] = "backquote",
  ["~"] = "backquote",
  ["["] = "bracketleft",
  ["{"] = "bracketleft",
  ["]"] = "bracketright",
  ["}"] = "bracketright",
  ["_"] = "minus",
  ["!"] = "digit1",
  ["@"] = "digit2",
  ["#"] = "digit3",
  ["$"] = "digit4",
  ["%"] = "digit5",
  ["^"] = "digit6",
  ["&"] = "digit7",
  ["*"] = "digit8",
  ["("] = "digit9",
  [")"] = "digit0",
}

function M.binding_hint(text)
  local value = tostring(text or "")
  local key = value:match("^key:(.+)$")
  if key then
    if KEYBOARD_KEY_NAMES[key] then
      return KEYBOARD_KEY_NAMES[key]
    end
    local canonical = V1_KEY_ALIASES[key]
    if canonical and KEYBOARD_KEY_NAMES[canonical] then
      return KEYBOARD_KEY_NAMES[canonical]
    end
    local v1code = key:match("^other:(%d+)$")
    if v1code then
      return "键" .. v1code
    end
    return derive_key_name(canonical or key)
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
