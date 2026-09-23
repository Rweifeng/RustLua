-- pause_menu.lua：《江湖有田》暂停菜单（M19-02 建立，M20-02 追加「设置」页）。
--
-- 暂停由 M16-01 状态机保证（mode = Pause：input.update 早退，移动与世界冻结）。
-- 主菜单项（纯键盘：W/S 或方向键选择、空格确认、+/- 主音量、Esc/P 继续）：
--   继续游戏 / 保存进度 / 读取进度 / 音量 / 返回标题
--
-- 主菜单的「音量」行按空格（或确认）进入**设置页**（M20-02）：
--   主音量 / BGM / 音效 / 界面音效（-/+ 或左右方向键调整，改动立刻生效）
--   / 保存设置（写回 <project>/settings.toml 的 [audio] 段）/ 返回
-- 设置页里 Esc 返回主菜单，P 直接继续游戏。分组音量以 settings.toml 为准，
-- 主音量同时随存档 provider "audio" 持久化（M19-02）。
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

-- 底部提示：按当前设备二选一，按键名从**当前绑定**推导（改键后自动同步）。
local function hint_line()
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
  local confirm_label = hints.binding_labels({ "confirm" })
  local back_label = hints.binding_labels({ "cancel" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = "+/-", alt_keyboard = "左右方向键", gamepad = "左右推杆", label = "调整音量" },
    { keyboard = confirm_label, gamepad = confirm_label, label = "确认/保存" },
    { keyboard = back_label, gamepad = back_label, label = "返回" },
  })
end
-- M21 主角形象系统：形象行的显示名与轮换都走 player/appearances。
local player = require("player.player")
local appearances = require("data.appearances")

local M = {}

local PANEL = layout.PANEL.pause
local ROW_H = 44
local ITEMS = { "resume", "save", "load", "appearance", "volume", "title" }
local LABEL = {
  resume = "继续游戏",
  save = "保存进度",
  load = "读取进度",
  -- 形象行的显示名由 row_label 动态拼成「形象  青衫少侠」（M21 主角形象系统）
  appearance = "形象",
  volume = "音量",
  title = "返回标题"
}

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

-- 主菜单状态：{ index, msg }；设置页状态：{ index, msg }；nil = 未打开。
local ui_state = nil
local settings_state = nil

-- 前向声明（设置页关/清理由文件后半段实现；Lua 词法作用域只认已声明的 local）。
local clear_ui
local clear_settings_ui

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
  ui.clear("pse:rule")
  ui.clear("pse:sel")
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
  ui.clear("pse:s:rule")
  ui.clear("pse:s:sel")
  for _, id in ipairs({ "pse:s:title", "pse:s:subtitle", "pse:s:msg", "pse:s:hint" }) do
    ui_text.hide("pse", id)
  end
  for i = 1, #SETTINGS_ITEMS do
    ui_text.hide("pse", "pse:s:row" .. i)
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

-- 主菜单选择移动（W/S 或方向键）；设置页打开时转发到设置页。
function M.move(delta)
  if not ui_state then
    return false
  end
  if settings_state then
    return M.settings_move(delta)
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
  ui.panel(id_prefix .. ":box", x, y, w, h, 0.08, 0.09, 0.12, 0.96)
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

function M.render()
  if not ui_state then
    return false
  end
  if settings_state then
    return M.render_settings()
  end
  local ROW_W = layout.list_width(PANEL)
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
  end
  ui_text.draw("pse", "pse:s:msg", x + BOX.title_dx, y + h + BOX.msg_dy, 15.0, ROW_W, tostring(settings_state.msg or ""))
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
  clear_ui()
  clear_settings_ui()
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
  clear_ui()
  clear_settings_ui()
  return true
end

return M
