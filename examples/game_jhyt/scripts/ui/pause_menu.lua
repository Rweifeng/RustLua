-- pause_menu.lua：《江湖有田》暂停菜单（M19-02 建立，M20-02 追加「设置」页，
-- M33 追加「帮助」页）。
--
-- 暂停由 M16-01 状态机保证（mode = Pause：input.update 早退，移动与世界冻结）。
-- 主菜单项（纯键盘：W/S 或方向键选择、空格确认、+/- 主音量、Esc/P 继续）：
--   继续游戏 / 保存进度 / 读取进度 / 形象 / 音量 / 帮助 / 返回标题
--
-- 主菜单的「音量」行按空格（或确认）进入**设置页**（M20-02）：
--   主音量 / BGM / 音效 / 界面音效（-/+ 或左右方向键调整，改动立刻生效）
--   / 保存设置（写回 <project>/settings.toml 的 [audio] 段）/ 返回
-- 设置页里 Esc 返回主菜单，P 直接继续游戏。分组音量以 settings.toml 为准，
-- 主音量同时随存档 provider "audio" 持久化（M19-02）。
--
-- 「帮助」行按确认进入**帮助页**（M33）：完整操作说明 + 玩法说明。操作说明原先常驻
-- HUD 底部通栏（M20-09），占玩法画面，现在收进这一页；页内是静态行表（没有选择环），
-- 确认/取消都返回主菜单。
--
-- 说明**按平台分叉**：PC 那一侧是键位表（键鼠/手柄，按键名从当前绑定推导），触屏
-- （WebGL/微信）那一侧是"屏上有哪些控件、点了出什么"——那上面根本没有键盘，键位表
-- 是噪声。触屏条目从 `layout.TOUCH` 现推（`M.touch_help_rows`），三条面板底部提示同
-- 一分叉（`TOUCH_HINT`）。
--
-- 选择移动复用引擎 `ui.focus_*` 焦点环（core/ui_navigation.rs，M20-02），
-- 方向键到边界环绕；引擎缺少该 API 时退回既有的取模语义。
--
-- 存档用 save_manager.save()/load()（B-2 注册中心，M17-01）；读取后由 main.lua
-- 暴露的 `jianghu_reload_map()` 重放本图 overlay（农田/建筑/NPC 位置），
-- 返回标题用 `jianghu_return_to_title()`（复位一次性引导标志后回到主入口门面 main）。
-- 两个钩子缺失时降级为「不重放 / 仅 scene.load('main')」，不会中断运行。
local state = require("core.state")
local save_manager = require("core.save_manager")
local audio = require("core.audio")
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环）。
local hints = require("ui.hints")
-- M30：面板文字按真实字宽裁剪（设置项/消息/提示行）。
local ui_text = require("ui.ui_text")
-- M32 竖版：面板矩形来自 `ui/layout.lua`（唯一定义处，改版式只改那一处）。
local layout = require("ui.layout")
-- docs/48 ui 换肤接线：面板背景/印章/关闭/音量滑轨贴图。
local skin = require("ui.skin")

-- 触屏形态判定：懒 require `ui.virtual_pad`（它只依赖 layout/state，与本模块无环），
-- 取不到或 `available()` 为假都按键鼠/手柄表显示——画面上没有圆钮时不该教人点圆钮。
--
-- `forced_touch` 是给验收用的旁路：桌面运行时 `app.platform()` 恒为 `"desktop"`，
-- 没有这个口子的话，触屏那一整套文案（帮助页 7 行 + 三条面板提示）在任何机器验收里
-- 都**不会被画到屏幕上**，也就量不到"有没有被裁成省略号"。
local forced_touch = nil

local function touch_host()
  if forced_touch ~= nil then
    return forced_touch
  end
  local ok, vp = pcall(require, "ui.virtual_pad")
  return ok and type(vp) == "table" and vp.available() == true
end

-- 验收/调试：强制触屏形态文案（`true`/`false`），`nil` 恢复按平台判定。见 `M.set_touch`。

-- 触屏底部提示：这台设备上没有任何一条键盘提示用得上，写成一行屏上真有入口的动作。
local TOUCH_HINT = {
  menu = "十字键选择 · 中央确定 · 右上关闭继续",
  settings = "十字键选择 · 中央确定保存 · 音量± 在右下动作条",
  help = "右上关闭返回主菜单",
}

-- 底部提示：按当前设备二选一，按键名从**当前绑定**推导（改键后自动同步）。
local function hint_line()
  if touch_host() then
    return TOUCH_HINT.menu
  end
  local confirm_label = hints.binding_labels({ "confirm" })
  local continue_label = hints.binding_labels({ "cancel", "pause" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = confirm_label, gamepad = confirm_label, label = "确认" },
    { keyboard = "+/-", gamepad = "+/-", label = "主音量" },
    { keyboard = continue_label, gamepad = continue_label, label = "继续" },
  })
end

local function settings_hint_line()
  if touch_host() then
    return TOUCH_HINT.settings
  end
  local confirm_label = hints.binding_labels({ "confirm" })
  local back_label = hints.binding_labels({ "cancel" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = "+/-", alt_keyboard = "左右方向键", gamepad = "左右推杆", label = "调整音量" },
    { keyboard = confirm_label, gamepad = confirm_label, label = "确认/保存" },
    { keyboard = back_label, gamepad = back_label, label = "返回" },
  })
end

-- 帮助页底部只有一条动作（任何键都返回），提示跟着当前绑定写。
local function help_hint_line()
  if touch_host() then
    return TOUCH_HINT.help
  end
  local back_label = hints.binding_labels({ "cancel" })
  return hints.panel_line({
    { keyboard = back_label, gamepad = back_label, label = "返回" },
  })
end
-- M21 主角形象系统：形象行的显示名与轮换都走 player/appearances。
local player = require("player.player")
local appearances = require("data.appearances")

local M = {}

local PANEL = layout.PANEL.pause
local ROW_H = 44
local ITEMS = { "resume", "save", "load", "appearance", "volume", "help", "title" }
local LABEL = {
  resume = "继续游戏",
  save = "保存进度",
  load = "读取进度",
  -- 形象行的显示名由 row_label 动态拼成「形象  青衫少侠」（M21 主角形象系统）
  appearance = "形象",
  volume = "音量",
  help = "帮助",
  title = "返回标题"
}

-- 帮助页：原先常驻 HUD 底部通栏的键位说明搬到这里（完整说明不占玩法画面）。
-- 键位表来自 `hints.guide_rows`（按**当前绑定**推导，改键后自动同步），
-- 玩法说明是静态文案，因此本页**没有选择环**：确认与取消都返回主菜单。
local HELP_NOTES = {
  { "劳作", "翻土 / 播种 / 浇水 / 收获消耗体力，体力不足只能休息" },
  { "作息", "睡觉结束当日：作物成长一天、体力回满、自动存档" },
  { "战斗", "武学招式消耗内力，普攻伤害随武器加成" },
}
-- 「改键」这条按平台分叉：触屏宿主根本没有 settings.toml 可编辑，把 PC 的改绑说明
-- 抄给触屏玩家，等于教他做一件在这台设备上做不到的事。
local HELP_NOTES_KEY = {
  { "改键", "工程 settings.toml 的 [input] 段可改绑，本页键名随绑定更新" },
}
local HELP_NOTES_TOUCH = {
  { "改键", "触屏没有改绑入口，请在 PC 版 settings.toml 改" },
}
-- 触屏形态（WebGL / 微信）没有键盘，键位表在那上面是噪声，因此换成"屏上有什么、
-- 点了出什么"。**条目名一律从 `layout.TOUCH` 现推**（与画在屏幕上的那一套同源）：
-- 上一版是手抄的"交互 / 攻击 / 背包 / 任务 / 暂停"，按钮簇从 2×2 方块改成左上弧 +
-- 「更多」+ 九格动作盘之后就和画面不符了——再抄一次还会再错一次。
local function touch_labels(list, separator)
  local parts = {}
  for _, entry in ipairs(list or {}) do
    local label = entry.label or entry.action or entry.key
    if label then
      parts[#parts + 1] = tostring(label)
    end
  end
  return table.concat(parts, separator or "/")
end

-- 面板动作条的标签**并集**（背包装备/选种、商店换页/数量±、暂停音量±）。`modes` 是
-- 哈希表，按模式名排序再收，避免每次拼出不同顺序的文案（那样排版门与字体子集都会漂）。
local function panel_action_labels(panel_actions)
  local modes = (panel_actions or {}).modes or {}
  local names = {}
  for mode in pairs(modes) do
    names[#names + 1] = mode
  end
  table.sort(names)
  local seen, parts = {}, {}
  for _, mode in ipairs(names) do
    for _, entry in ipairs(modes[mode] or {}) do
      local label = tostring(entry.label or entry.action or "")
      if label ~= "" and not seen[label] then
        seen[label] = true
        parts[#parts + 1] = label
      end
    end
  end
  return table.concat(parts, "/")
end

function M.touch_help_rows()
  local touch = layout.TOUCH or {}
  return {
    { "移动", "左下摇杆" },
    { "圆钮", "右下：" .. touch_labels(touch.buttons) },
    { "招式", "点右栏卡片出招，格挡那张要按住" },
    { "更多", "招式栏空格「更多」，点开动作盘" },
    { "动作盘", touch_labels((touch.action or {}).items) },
    { "面板", "十字键选择、中央确定、右上关闭" },
    { "动作条", "面板右下：" .. panel_action_labels(touch.panel_actions) },
  }
end
local HELP_ROW_H = 28
local HELP_FONT = 18.0
-- 键位/说明的"值"列起始偏移：标签列与值列各用自己的宽度预算（都过排版门）。
local HELP_VALUE_DX = 108
-- 帮助页行数的清理上界（实际行数 = 2 个小标题 + 键位行 + 4 条玩法说明）。
local HELP_ROW_CAP = 20

-- 设置页（M20-02）：四路音量 + 保存 + 返回。
local SETTINGS_ITEMS = { "master", "bgm", "sfx", "ui", "save", "back" }
local SETTINGS_LABEL = {
  master = "主音量",
  bgm = "BGM",
  sfx = "音效",
  ui = "界面音效",
  save = "保存设置",
  back = "返回"
}
local AUDIO_ITEMS = { master = true, bgm = true, sfx = true, ui = true }
local STEP = 0.05

local FOCUS = "pse:focus"
local SETTINGS_FOCUS = "pse:settings:focus"

-- 主菜单状态：{ index, msg }；设置页状态：{ index, msg }；帮助页状态：{}；nil = 未打开。
local ui_state = nil
local settings_state = nil
local help_state = nil

-- 前向声明（设置页关/清理由文件后半段实现；Lua 词法作用域只认已声明的 local）。
local clear_ui
local clear_settings_ui
local clear_help_ui

-- 上一页的名字（"menu"/"settings"/"help"）。`ui.panel` / `ui.text_at` 画出的元素**跨帧持久**，
-- 只有 `ui.clear` / `ui.hide` 才收得掉；而每帧的 `M.render()` 只画当前页，于是"进入子页"
-- 这件事本身不清旧页 → 主菜单的标题与七行文字会留在帮助页/设置页同一块面板上（重叠、无报错）。
-- 因此把清理收在渲染入口的**换页那一刻**：每帧的重复渲染不再付清理代价，新增子页也不会漏清。
local rendered_page = nil

local function switch_page(page)
  if rendered_page == page then
    return
  end
  if rendered_page == "menu" then
    clear_ui()
  elseif rendered_page == "settings" then
    clear_settings_ui()
  elseif rendered_page == "help" then
    clear_help_ui()
  end
  rendered_page = page
end

function M.row_limit()
  return #ITEMS
end

function M.ui_active()
  return ui_state ~= nil
end

function M.index()
  return ui_state and ui_state.index or 0
end

function M.msg()
  return ui_state and ui_state.msg or nil
end

function M.item()
  if not ui_state then
    return nil
  end
  return ITEMS[ui_state.index + 1]
end

-- ---------- 设置页 ----------

function M.settings_active()
  return settings_state ~= nil
end

function M.settings_index()
  return settings_state and settings_state.index or 0
end

function M.settings_item()
  if not settings_state then
    return nil
  end
  return SETTINGS_ITEMS[settings_state.index + 1]
end

function M.settings_msg()
  return settings_state and settings_state.msg or nil
end

function M.settings_row_limit()
  return #SETTINGS_ITEMS
end

-- 设置页行文案：音量行显示百分比，其余为动作名。
function M.settings_rows()
  local out = {}
  for i, key in ipairs(SETTINGS_ITEMS) do
    local label = SETTINGS_LABEL[key] or key
    if AUDIO_ITEMS[key] then
      local value = audio.bus_volume(key)
      local percent = value and math.floor(value * 100 + 0.5) or 0
      label = string.format("%s  %d%%（- / +）", label, percent)
    end
    out[i] = { key = key, label = label }
  end
  return out
end

-- 把设置页焦点环与当前序号同步（行数固定，主要是重建项矩形）。
local function sync_settings_focus()
  if not settings_state then
    return
  end
  nav.open(SETTINGS_FOCUS, #SETTINGS_ITEMS, 1)
  nav.select(SETTINGS_FOCUS, settings_state.index + 1)
end

local function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #ITEMS, 1)
  nav.select(FOCUS, ui_state.index + 1)
end

-- 焦点环方向移动（step 行，正数向下）；引擎 API 不可用返回 nil。
local function ring_move(id, from_index, step)
  local direction = step > 0 and "down" or "up"
  local index = from_index
  local moved = false
  for _ = 1, math.abs(step) do
    local next_index = nav.move(id, direction)
    if not next_index then
      break
    end
    index = next_index
    moved = true
  end
  if not moved then
    return nil
  end
  return index
end

-- 设置页选择移动（W/S 或方向键）；返回是否移动。
function M.settings_move(delta)
  if not settings_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 then
    return false
  end
  sync_settings_focus()
  local index = ring_move(SETTINGS_FOCUS, settings_state.index + 1, step)
  if index then
    settings_state.index = index - 1
    return true
  end
  -- 降级：旧的取模语义。
  local target = (settings_state.index + step) % #SETTINGS_ITEMS
  if target < 0 then
    target = target + #SETTINGS_ITEMS
  end
  settings_state.index = target
  return true
end

function M.settings_select(index)
  if not settings_state then
    return false
  end
  local i = math.floor(tonumber(index) or 0)
  if i < 0 or i >= #SETTINGS_ITEMS then
    return false
  end
  settings_state.index = i
  return true
end

-- 音量 ±：选中音量行时调该分组（立刻生效，同时刷新文案与音效）。
-- 选中「保存设置」/「返回」时不调整，仅提示。
function M.settings_adjust(delta)
  if not settings_state then
    return false
  end
  local step = tonumber(delta)
  if not step then
    return false
  end
  local key = M.settings_item()
  if not AUDIO_ITEMS[key] then
    settings_state.msg = "在音量行上用 - / + 调整"
    M.render()
    return false
  end
  local value = audio.adjust_bus_volume(key, step * STEP)
  if value == nil then
    settings_state.msg = "音频分组不可用"
  else
    local percent = math.floor(value * 100 + 0.5)
    settings_state.msg = string.format("%s %d%%", SETTINGS_LABEL[key], percent)
  end
  audio.ui("ui_click")
  M.render()
  return true
end
-- 保存设置到 <project>/settings.toml（[audio] 段；其余段原样保留）。
function M.settings_save()
  local ok, detail = audio.save_settings()
  local msg
  if ok then
    msg = "设置已保存"
  elseif detail == "missing" or detail == "unsupported" then
    msg = "设置保存不可用（" .. tostring(detail) .. "）"
  else
    msg = "保存失败：" .. tostring(detail)
  end
  audio.ui(ok and "ui_confirm" or "ui_cancel")
  if settings_state then
    settings_state.msg = msg
  end
  M.render()
  return ok, detail
end

function M.settings_close()
  if not settings_state then
    return false
  end
  settings_state = nil
  clear_settings_ui()
  nav.close(SETTINGS_FOCUS)
  nav.close(SETTINGS_SCROLL)
  audio.ui("ui_cancel")
  M.render()
  return true
end

-- 设置页确认（空格）：保存行→保存；返回行→返回；音量行→提示用 -/+。
function M.settings_confirm()
  if not settings_state then
    return false, "not_open"
  end
  local key = M.settings_item()
  if key == "save" then
    return M.settings_save()
  elseif key == "back" then
    M.settings_close()
    return true, "back"
  elseif AUDIO_ITEMS[key] then
    settings_state.msg = "用 - / + 调整（当前 "
      .. tostring(math.floor((audio.bus_volume(key) or 0) * 100 + 0.5)) .. "%）"
    M.render()
    return true, key
  end
  return false, "unknown"
end

function M.open_settings()
  if not ui_state then
    return false
  end
  settings_state = { index = 0, msg = nil, window = 1 }
  audio.ui("ui_click")
  M.render()
  return true
end

-- ---------- 主菜单 ----------

function M.rows()
  local out = {}
  for i, key in ipairs(ITEMS) do
    local label = LABEL[key]
    if key == "volume" then
      label = label .. string.format("  %d%%（- / +）", math.floor(audio.volume() * 100 + 0.5))
    elseif key == "appearance" then
      -- M21 主角形象：直接显示当前形象的**中文名**，按确认在可选形象间轮换。
      local entry = player.appearance_entry()
      local current = (appearances.get(player.appearance_id()) or {})
      label = label .. "  " .. tostring(current.name or (entry and entry.display) or "?")
    end
    out[i] = { key = key, label = label }
  end
  return out
end

function clear_ui()
  ui.clear("pse:box")
  ui.clear("pse:skin_bg")
  ui.clear("pse:rule")
  ui.clear("pse:sel")
  -- 换肤贴图走 `ui.image`（跨帧持久）：不清就会在面板关闭后留残影。
  for _, id in ipairs({ "pse:divider", "pse:seal", "pse:close", "pse:key_hint" }) do
    ui.clear(id)
  end
  for _, id in ipairs({ "pse:title", "pse:msg", "pse:hint" }) do
    ui_text.hide("pse", id)
  end
  for i = 1, #ITEMS do
    ui_text.hide("pse", "pse:row" .. i)
  end
  ui_text.reset("pse")
end

function clear_settings_ui()
  ui.clear("pse:s:box")
  ui.clear("pse:s:skin_bg")
  ui.clear("pse:s:rule")
  ui.clear("pse:s:sel")
  -- 换肤贴图走 `ui.image`（跨帧持久）：滑轨/手柄/按钮/提示条都要显式清理。
  for _, id in ipairs({ "pse:s:divider", "pse:s:seal", "pse:s:close", "pse:s:key_hint",
    "pse:s:slot", "pse:s:btn_save", "pse:s:btn_back" }) do
    ui.clear(id)
  end
  for i = 1, #SETTINGS_ITEMS do
    ui.clear("pse:s:track" .. i)
    ui.clear("pse:s:handle" .. i)
  end
  for _, id in ipairs({ "pse:s:title", "pse:s:subtitle", "pse:s:msg", "pse:s:hint" }) do
    ui_text.hide("pse", id)
  end
  for i = 1, #SETTINGS_ITEMS do
    ui_text.hide("pse", "pse:s:row" .. i)
  end
  ui_text.reset("pse")
end

-- 帮助页清理：行数随绑定变化（改键后某套设备可能少一行），因此按**上界**清理——
-- `ui.hide` 对没画过的 id 是幂等的（引擎侧只是标不可见）。
function clear_help_ui()
  for _, id in ipairs({ "pse:h:box", "pse:h:skin_bg", "pse:h:rule", "pse:h:sel",
    "pse:h:divider", "pse:h:seal", "pse:h:close", "pse:h:key_hint" }) do
    ui.clear(id)
  end
  for _, id in ipairs({ "pse:h:title", "pse:h:hint" }) do
    ui_text.hide("pse", id)
  end
  for i = 1, HELP_ROW_CAP do
    ui_text.hide("pse", "pse:h:row" .. i)
    ui_text.hide("pse", "pse:h:val" .. i)
  end
  ui_text.reset("pse")
end

function M.select(index)
  if not ui_state then
    return false
  end
  local i = math.floor(tonumber(index) or 0)
  if i < 0 or i >= #ITEMS then
    return false
  end
  ui_state.index = i
  return true
end

-- 主菜单选择移动（W/S 或方向键）；设置页打开时转发到设置页，帮助页无选择环。
function M.move(delta)
  if not ui_state then
    return false
  end
  if settings_state then
    return M.settings_move(delta)
  end
  if help_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 then
    return false
  end
  sync_focus()
  local index = ring_move(FOCUS, ui_state.index + 1, step)
  if index then
    ui_state.index = index - 1
    return true
  end
  local target = (ui_state.index + step) % #ITEMS
  if target < 0 then
    target = target + #ITEMS
  end
  ui_state.index = target
  return true
end

-- 面板内部几何：标题 / 分隔线 / 列表 / 消息 / 提示统一取自 `layout.BOX`（M32 竖版）。
local BOX = layout.BOX
local RULE_H = 2
-- 分隔线与行底衬：极淡的白，只做"标题 / 列表"的分层，不抢内容。
local RULE_COLOR = { 0.72, 0.75, 0.82, 0.22 }
local ROW_FILL_SELECTED = { 0.85, 0.72, 0.42, 0.20 }

-- 面板骨架：背板 + 标题 + 标题下的分隔线。返回 (x, y, w, h)。
local function draw_frame(id_prefix, title, row_fill)
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  -- docs/48 ui 换肤接线：主面板纸面背景（9-slice）叠在纯色兜底之上。
  skin.panel(id_prefix .. ":skin_bg", x, y, w, h, "panel_main")
  ui.panel(id_prefix .. ":box", x, y, w, h, 0.08, 0.09, 0.12, 0.96)
  -- docs/48 ui 换肤接线：标题分隔线用 common 竹纹分隔贴图（叠在原纯色线之上）。
  skin.icon(id_prefix .. ":divider", x + BOX.title_dx, y + BOX.title_dy + BOX.title_font + 6, w - BOX.title_dx * 2, 8, "divider")
  ui.panel(
    id_prefix .. ":rule",
    x + BOX.title_dx,
    y + BOX.title_dy + BOX.title_font + 14,
    w - BOX.title_dx * 2,
    RULE_H,
    RULE_COLOR[1],
    RULE_COLOR[2],
    RULE_COLOR[3],
    RULE_COLOR[4]
  )
  ui_text.draw("pse", id_prefix .. ":title", x + BOX.title_dx, y + BOX.title_dy, BOX.title_font, w - BOX.title_dx * 2, title)
  -- docs/48 ui 换肤接线：标题右端印章（24×24）与右上关闭按钮（16×16）。
  skin.icon(id_prefix .. ":seal", x + w - BOX.title_dx - 44, y + BOX.title_dy + 2, 24, 24, "icon_seal")
  skin.icon(id_prefix .. ":close", x + w - BOX.title_dx - 16, y + BOX.title_dy + 2, 16, 16, "btn_close")
  if row_fill then
    ui.panel(
      id_prefix .. ":sel",
      x + BOX.list_dx,
      row_fill[1],
      w - BOX.list_dx * 2,
      row_fill[2],
      ROW_FILL_SELECTED[1],
      ROW_FILL_SELECTED[2],
      ROW_FILL_SELECTED[3],
      ROW_FILL_SELECTED[4]
    )
  else
    ui.clear(id_prefix .. ":sel")
  end
  return x, y, w, h
end

-- ---------- 帮助页 ----------

function M.help_active()
  return help_state ~= nil
end

-- 触屏形态判定在文件头（`touch_host`）：本模块的三条面板提示与帮助页共用同一个判定。
function M.set_touch(forced)
  if forced == nil then
    forced_touch = nil
  else
    forced_touch = forced and true or false
  end
  return touch_host()
end

-- 帮助页的行表：小标题 +「标签 / 值」两列。静态表（没有选择环，因此不建焦点环）。
-- 键位来自 `hints.guide_rows`：当前设备优先，该设备一个绑定都没有时回落另一套
-- （手柄没连时 `guide_rows("gamepad")` 为空，这里显示键鼠表而不是空白页）。
function M.help_rows()
  local rows = {}
  local notes = HELP_NOTES_KEY
  if touch_host() then
    rows[#rows + 1] = { kind = "section", text = "触屏操作" }
    for _, entry in ipairs(M.touch_help_rows()) do
      rows[#rows + 1] = { kind = "key", label = entry[1], value = entry[2] }
    end
    notes = HELP_NOTES_TOUCH
  else
    local device = hints.device()
    local guide = hints.guide_rows(device)
    if #guide == 0 then
      device = device == "gamepad" and "keyboard" or "gamepad"
      guide = hints.guide_rows(device)
    end
    rows[#rows + 1] = { kind = "section", text = "操作 · " .. hints.device_label(device) }
    for _, entry in ipairs(guide) do
      rows[#rows + 1] = { kind = "key", label = entry.label, value = entry.keys }
    end
  end
  rows[#rows + 1] = { kind = "section", text = "玩法" }
  for _, entry in ipairs(HELP_NOTES) do
    rows[#rows + 1] = { kind = "key", label = entry[1], value = entry[2] }
  end
  for _, entry in ipairs(notes) do
    rows[#rows + 1] = { kind = "key", label = entry[1], value = entry[2] }
  end
  return rows
end

function M.open_help()
  if not ui_state then
    return false
  end
  if settings_state then
    M.settings_close()
  end
  help_state = {}
  audio.ui("ui_click")
  M.render()
  return true
end

function M.help_close()
  if not help_state then
    return false
  end
  help_state = nil
  clear_help_ui()
  audio.ui("ui_cancel")
  M.render()
  return true
end

-- 帮助页绘制：键位两列 + 玩法说明 + 底部一行返回提示。
function M.render_help()
  if not help_state then
    return false
  end
  local ROW_W = layout.list_width(PANEL)
  switch_page("help")
  ui_text.begin("pse")
  local rows = M.help_rows()
  local x, y, w, h = draw_frame("pse:h", "帮助", nil)
  for i = 1, #rows do
    local row = rows[i]
    local row_y = y + BOX.list_dy + (i - 1) * HELP_ROW_H
    if row.kind == "section" then
      ui_text.draw("pse", "pse:h:row" .. i, x + BOX.list_dx, row_y, HELP_FONT, ROW_W, row.text)
    else
      -- 标签列与值列各拿一份预算：值列多长取决于玩家怎么改键，两列分开才量得准
      -- （合成一列时超宽只会画成省略号，画面上看不出排版坏了）。
      ui_text.draw("pse", "pse:h:row" .. i, x + BOX.list_dx, row_y, HELP_FONT, HELP_VALUE_DX - 8, row.label)
      ui_text.draw("pse", "pse:h:val" .. i, x + BOX.list_dx + HELP_VALUE_DX, row_y, HELP_FONT,
        w - BOX.list_dx * 2 - HELP_VALUE_DX - 8, row.value)
    end
  end
  -- docs/48 ui 换肤接线：提示条背景与主菜单/设置页同款。
  skin.panel("pse:h:key_hint", x + BOX.title_dx, y + h + BOX.hint_dy - 6, ROW_W, BOX.hint_font + 12, "key_hint")
  ui_text.draw("pse", "pse:h:hint", x + BOX.title_dx, y + h + BOX.hint_dy, BOX.hint_font, ROW_W, help_hint_line())
  return true
end

function M.render()
  if not ui_state then
    return false
  end
  if settings_state then
    return M.render_settings()
  end
  if help_state then
    return M.render_help()
  end
  local ROW_W = layout.list_width(PANEL)
  switch_page("menu")
  ui_text.begin("pse")
  local rows = M.rows()
  -- 选中行的底衬矩形（列表从 y+BOX.list_dy 起，行高 ROW_H）。
  local sel_y = PANEL.y + BOX.list_dy + ui_state.index * ROW_H
  local x, y, w, h = draw_frame("pse", "暂停", { sel_y, ROW_H })
  for i = 1, #ITEMS do
    local id = "pse:row" .. i
    local row = rows[i]
    local label = row and row.label or ""
    label = (i - 1 == ui_state.index) and ("> " .. label) or ("  " .. label)
    ui_text.draw("pse", id, x + BOX.list_dx, y + BOX.list_dy + (i - 1) * ROW_H, 18.0, ROW_W, label)
  end
  ui_text.draw("pse", "pse:msg", x + BOX.title_dx, y + h + BOX.msg_dy, 15.0, ROW_W, tostring(ui_state.msg or ""))
  -- docs/48 ui 换肤接线：按键提示条背景（key_hint 9-slice）。
  skin.panel("pse:key_hint", x + BOX.title_dx, y + h + BOX.hint_dy - 6, ROW_W, BOX.hint_font + 12, "key_hint")
  ui_text.draw("pse", "pse:hint", x + BOX.title_dx, y + h + BOX.hint_dy, BOX.hint_font, ROW_W, hint_line())
  return true
end

-- 设置页绘制（M20-02）：四路音量 + 保存 + 返回。
function M.render_settings()
  if not settings_state then
    return false
  end
  local ROW_W = layout.list_width(PANEL)
  sync_settings_focus()
  switch_page("settings")
  ui_text.begin("pse")
  local rows = M.settings_rows()
  local sel_y = PANEL.y + BOX.list_dy + settings_state.index * ROW_H
  local x, y, w, h = draw_frame("pse:s", "设置", { sel_y, ROW_H })
  for i = 1, #SETTINGS_ITEMS do
    local id = "pse:s:row" .. i
    local row = rows[i]
    local label = row and row.label or ""
    label = (i - 1 == settings_state.index) and ("> " .. label) or ("  " .. label)
    ui_text.draw("pse", id, x + BOX.list_dx, y + BOX.list_dy + (i - 1) * ROW_H, 18.0, ROW_W, label)
    -- docs/48 ui 换肤接线：音量行的滑轨 + 手柄（四路音量各自真实 bus 值，master/bgm/sfx/ui）。
    local bus = SETTINGS_ITEMS[i]
    if bus == "save" then
      -- docs/48 ui 换肤接线：存档槽（save_slot 9s，48×24）画在"保存"行尾 + 主按钮（btn_primary）。
      skin.panel("pse:s:slot", x + w - BOX.list_dx - 148, y + BOX.list_dy + (i - 1) * ROW_H + 2, 96, 24, "save_slot")
      skin.panel("pse:s:btn_save", x + w - BOX.list_dx - 48, y + BOX.list_dy + (i - 1) * ROW_H + 2, 40, 24, "btn_primary")
    elseif bus == "back" then
      -- docs/48 ui 换肤接线：返回行尾副按钮（btn_secondary）。
      skin.panel("pse:s:btn_back", x + w - BOX.list_dx - 48, y + BOX.list_dy + (i - 1) * ROW_H + 2, 40, 24, "btn_secondary")
    elseif bus ~= "save" and bus ~= "back" then
      local row_y = y + BOX.list_dy + (i - 1) * ROW_H + (ROW_H - 8) / 2
      skin.panel("pse:s:track" .. i, x + w - BOX.list_dx - 116, row_y, 116, 8, "slider_track")
      local ratio = audio.bus_volume(bus)
      if type(ratio) ~= "number" then
        ratio = 0.5
      end
      skin.icon(
        "pse:s:handle" .. i,
        x + w - BOX.list_dx - 116 + math.floor(ratio * 100),
        row_y - 2,
        12,
        12,
        "slider_handle"
      )
    end
  end
  ui_text.draw("pse", "pse:s:msg", x + BOX.title_dx, y + h + BOX.msg_dy, 15.0, ROW_W, tostring(settings_state.msg or ""))
  -- docs/48 ui 换肤接线：按键提示条背景（key_hint 9-slice）。
  skin.panel("pse:s:key_hint", x + BOX.title_dx, y + h + BOX.hint_dy - 6, ROW_W, BOX.hint_font + 12, "key_hint")
  ui_text.draw("pse", "pse:s:hint", x + BOX.title_dx, y + h + BOX.hint_dy, BOX.hint_font, ROW_W, settings_hint_line())
  return true
end

-- 音量 ±（设置页打开时调选中分组；否则调主音量——M19-02 语义不变）。
function M.adjust_volume(delta)
  if settings_state then
    return M.settings_adjust(delta)
  end
  local step = tonumber(delta)
  if not step then
    return false
  end
  audio.adjust_volume(step * audio.VOLUME_STEP)
  audio.ui("ui_click")
  if ui_state then
    ui_state.msg = string.format("音量 %d%%", math.floor(audio.volume() * 100 + 0.5))
  end
  M.render()
  return true
end

-- 保存进度（写盘 + 提交，失败给出原因）。
function M.save_game()
  local ok = save_manager.save()
  local msg = ok and "已保存" or "保存失败（详见日志）"
  audio.ui(ok and "ui_confirm" or "ui_cancel")
  if ui_state then
    ui_state.msg = msg
  end
  M.render()
  return ok, msg
end

-- 读取进度：仅当存档场景与当前场景一致时可直接恢复；否则提示回标题读档
-- （跨图读档由门面 BtnContinue 的路径负责，见 main.lua）。
function M.load_game()
  local ok, reason = save_manager.load()
  audio.ui(ok and "ui_confirm" or "ui_cancel")
  if ok then
    if type(jianghu_reload_map) == "function" then
      jianghu_reload_map()
    end
    if ui_state then
      ui_state.msg = "读取成功"
    end
    M.render()
    return true, "ok"
  end
  local msg = "读取失败：" .. tostring(reason or "unknown")
  if ui_state then
    ui_state.msg = msg
  end
  M.render()
  return false, reason
end

local function return_to_title()
  M.close()
  if type(jianghu_return_to_title) == "function" then
    jianghu_return_to_title()
    return true
  end
  -- 降级：至少回到标题门面（一次性引导标志未复位时下次进入沿用当前运行态）。
  return scene.load("main")
end

-- 确认当前菜单项；返回 (是否处理, 动作名)。
function M.confirm()
  if not ui_state then
    return false, "not_open"
  end
  if settings_state then
    return M.settings_confirm()
  end
  if help_state then
    -- 帮助页是静态行表：确认与取消一样返回主菜单。
    M.help_close()
    return true, "help_back"
  end
  local key = M.item()
  if key == "resume" then
    M.close()
    return true, "resume"
  elseif key == "save" then
    local ok = M.save_game()
    return ok, "save"
  elseif key == "load" then
    local ok = M.load_game()
    return ok, "load"
  elseif key == "volume" then
    -- 音量行上按确认＝打开设置页（M20-02：四路音量 ± + 保存）。
    M.open_settings()
    return true, "settings"
  elseif key == "help" then
    -- 帮助页（M33）：完整操作说明收在这里，HUD 上不再常驻键位通栏。
    M.open_help()
    return true, "help"
  elseif key == "appearance" then
    -- M21 主角形象：按确认轮换到下一个可选形象（立刻重挂序列帧），
    -- 结果写进 state.appearance → 随存档持久化。
    local next_id = appearances.next_id(player.appearance_id())
    local ok, err = player.set_appearance(next_id)
    if ok then
      local entry = appearances.get(next_id) or {}
      ui_state.msg = "形象：" .. tostring(entry.name or next_id)
      M.render()
      return true, "appearance"
    end
    ui_state.msg = "换装失败：" .. tostring(err)
    M.render()
    return false, "appearance"
  elseif key == "title" then
    return_to_title()
    return true, "title"
  end
  return false, "unknown"
end

function M.open()
  ui_state = { index = 0, msg = nil }
  settings_state = nil
  help_state = nil
  state.set_mode("Pause")
  audio.ui("ui_click")
  M.render()
  return true
end

function M.close()
  if not ui_state then
    return false
  end
  ui_state = nil
  settings_state = nil
  help_state = nil
  clear_ui()
  clear_settings_ui()
  clear_help_ui()
  rendered_page = nil
  nav.close(FOCUS)
  nav.close(SETTINGS_FOCUS)
  nav.close(SETTINGS_SCROLL)
  if state.is_mode("Pause") then
    state.set_mode(state.return_mode or "Playing")
  end
  audio.ui("ui_cancel")
  return true
end

function M.reset()
  ui_state = nil
  settings_state = nil
  help_state = nil
  clear_ui()
  clear_settings_ui()
  clear_help_ui()
  rendered_page = nil
  return true
end

return M
