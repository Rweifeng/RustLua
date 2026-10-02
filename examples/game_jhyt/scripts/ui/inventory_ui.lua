-- inventory_ui.lua：《江湖有田》背包面板（M19-02；导航 M20-02）。
--
-- 与 shop.lua 同款纯键盘面板范式（render 每次操作整体重绘；场景切换会清空
-- 动态 UI，故不能只增量更新）：
--   W/S      上/下移动选择
--   A/D      左/右列跳动（每列 10 行）
--   方向键   上/下/左/右移动选择（走引擎焦点环 `ui.focus_*`，到边界环绕）
--   1..9     直接定位到第 N 项
--   U/空格   使用物品（inventory.use，失败显示原因）
--   E        装备到 tool/weapon 槽（inv.equip）
--   C        把选中的种子设为当前播种种子（state.selected_seed）
--   I/Esc    关闭
--
-- 只读展示：名称/数量/分类；容量与负重。
--
-- 输入在 input.lua 的 Inventory 分支读取后调用本模块，本模块不 require input
-- （否则与 input → inventory_ui 形成 require 环）。
local state = require("core.state")
local inventory = require("player.inventory")
-- M19-02：面板开闭/操作结果音效（缺文件或无设备时静默降级）。
-- M20-02：界面音走 UI 总线（audio.ui），玩法音效走 SFX 总线。
local audio = require("core.audio")
-- M20-02：列表焦点/滚动导航（引擎 ui_navigation）。
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环）。
local hints = require("ui.hints")
-- M30：行/详情/提示按真实字宽裁剪并登记（长物品名与长说明不再画到面板外）。
local ui_text = require("ui.ui_text")
-- M32 竖版：面板矩形与行偏移的唯一来源（`ui/layout.lua`）。
local layout = require("ui.layout")
-- docs/48 ui 换肤接线：面板背景/槽框/品质框/印章贴图。
local skin = require("ui.skin")

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
  local use_label = hints.binding_labels({ "use" })
  local equip_label = hints.binding_labels({ "interact" })
  local seed_label = hints.binding_labels({ "seed" })
  local close_label = hints.binding_labels({ "inventory", "cancel" })
  return hints.panel_line({
    {
      keyboard = "W/S",
      alt_keyboard = "上下方向键",
      gamepad = "左摇杆/DPad",
      label = "选择",
    },
    { keyboard = "1-9", gamepad = digits_label(), label = "定位" },
    { keyboard = use_label, gamepad = use_label, label = "使用" },
    { keyboard = equip_label, gamepad = equip_label, label = "装备" },
    { keyboard = seed_label, gamepad = seed_label, label = "选种子" },
    { keyboard = close_label, gamepad = close_label, label = "关闭" },
  })
end

local M = {}

-- 竖版版面（M32）：单列 10 行。横屏是"两列 × 10 行"（640×484 面板），竖屏只有 720 宽，
-- 两列会把物品名压到 250px 以内（长名字必截断），所以改成**单列 10 行**：
-- 行矮一点（40px）、列宽翻倍（544px），信息量不减而可读性更好。
local PANEL = layout.PANEL.inventory
local ROWS_PER_COL = 10
local COLS = 1
local ROW_LIMIT = ROWS_PER_COL * COLS
local COL_X = { layout.BOX.list_dx }
local ROW_H = 40

local FOCUS = "bp:focus"

local KIND_TEXT = {
  crop = "作物",
  seed = "种子",
  consumable = "丹药",
  material = "材料",
  tool = "工具",
  weapon = "兵器"
}

local REASON_TEXT = {
  unknown_item = "未知物品",
  not_enough = "数量不足",
  no_effect = "该物品没有使用效果",
  bad_buff = "增益效果非法",
  no_inventory = "背包不存在",
  not_equippable = "该物品不能装备",
  not_seed = "该物品不是种子"
}

-- 面板状态：{ index, msg }；nil = 未打开。
local ui_state = nil

-- ---------- 物品访问 ----------

local function data_of(item_id)
  if type(item_id) ~= "string" or item_id == "" then
    return nil
  end
  local ok, def = pcall(item.def, item_id)
  if ok and type(def) == "table" then
    return def
  end
  local items = require("data.items")
  local tools = require("data.tools")
  return items[item_id] or tools[item_id]
end

function M.name_of(item_id)
  local def = data_of(item_id)
  return (def and def.name) or tostring(item_id)
end

function M.kind_of(item_id)
  local def = data_of(item_id)
  return def and def.kind or nil
end

-- 背包快照：{ capacity, occupied, weight, slots = { {slot,item,count}... } }。
function M.snapshot()
  local ok, data = pcall(inv.get, state.inventory)
  if not ok or type(data) ~= "table" then
    return { capacity = 0, occupied = 0, weight = 0, slots = {} }
  end
  local capacity = tonumber(data.capacity) or 0
  local by_index = {}
  local occupied, weight = 0, 0
  for index, stack in pairs(data.slots or {}) do
    if type(stack) == "table" and type(stack.item) == "string" and stack.item ~= "" then
      local count = tonumber(stack.count) or 0
      by_index[tonumber(index) or 0] = { slot = tonumber(index) or 0, item = stack.item, count = count }
      occupied = occupied + 1
      weight = weight + count
    end
  end
  local slots = {}
  for index = 1, capacity do
    local entry = by_index[index]
    if entry then
      slots[#slots + 1] = entry
    end
  end
  return { capacity = capacity, occupied = occupied, weight = weight, slots = slots }
end

function M.rows()
  return M.snapshot().slots
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

function M.msg()
  return ui_state and ui_state.msg or nil
end

-- 焦点环与当前序号同步（M20-02）：网格按 2 列布局，方向键左右也有效。
local function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #M.rows(), COLS)
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

local function current_row()
  if not ui_state then
    return nil
  end
  -- ui_state.index 是 0 基序号，rows 是 1 基数组（渲染/使用/装备共用同一映射）。
  return M.rows()[ui_state.index + 1]
end

local function clear_ui()
  ui.clear("bp:box")
  ui.clear("bp:skin_bg")
  ui.clear("bp:rule")
  ui.clear("bp:sel")
  -- 换肤贴图走 `ui.image`（跨帧持久，需显式清理）：漏一条就会在面板关闭后
  -- 原地留一块残影（实测：key_hint 提示条残成画面底部米色横条）。
  for _, id in ipairs({ "bp:divider", "bp:seal", "bp:key_hint", "bp:sel_slot", "bp:quality" }) do
    ui.clear(id)
  end
  for i = 1, ROW_LIMIT do
    ui.clear("bp:slot" .. i)
  end
  for _, id in ipairs({ "bp:title", "bp:cap", "bp:msg", "bp:hint", "bp:empty", "bp:detail" }) do
    ui_text.hide("bp", id)
  end
  for i = 1, ROW_LIMIT do
    ui_text.hide("bp", "bp:row" .. i)
  end
  ui_text.reset("bp")
end

-- ---------- 操作 ----------

-- 选择：index 为 0 基序号（0 = 第一项）；越界返回 false。
function M.select(index)
  if not ui_state then
    return false
  end
  local i = math.floor(tonumber(index) or 0)
  local rows = M.rows()
  if i < 0 or i >= #rows then
    return false
  end
  ui_state.index = i
  ui_state.msg = nil
  sync_focus()
  return true
end

-- 相对移动（delta 为行数；跨列用 ROWS_PER_COL）。焦点环可用时按方向逐行移动
-- （到边界环绕），否则退回既有的钳制语义。
function M.move(delta)
  if not ui_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  local rows = M.rows()
  if #rows == 0 or step == 0 then
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
  ui_state.msg = nil
  return true
end

-- 方向键移动（M20-02）：直接交给引擎焦点环（到边界环绕）。
-- M32 竖版是**单列**列表：左右方向键不再有意义（横屏两列时是"跳到另一列"），
-- 这里显式忽略左右轴并把上下轴前推给焦点环，避免"按了右键什么都不发生"被当成卡顿。
function M.move_dir(direction)
  if not ui_state then
    return false
  end
  if direction ~= "up" and direction ~= "down" then
    return false
  end
  if #M.rows() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  ui_state.index = index - 1
  ui_state.msg = nil
  return true
end

local function feedback(ok, reason, verb)
  if ok then
    return verb .. "成功"
  end
  return REASON_TEXT[reason] or ("失败：" .. tostring(reason))
end

-- 使用选中物品（inventory.use 是 M17-04 的唯一入口，失败时物品不消耗）。
function M.use()
  if not ui_state then
    return false, "not_open"
  end
  local row = current_row()
  if not row then
    ui_state.msg = "背包为空"
    M.render()
    return false, "not_enough"
  end
  local ok, reason = inventory.use(row.item)
  ui_state.msg = feedback(ok, reason, "使用 " .. M.name_of(row.item))
  audio.ui(ok and "ui_confirm" or "ui_cancel")
  M.render()
  return ok, reason
end

-- 装备到 tool/weapon 槽：槽位由物品 kind 决定（tool → tool，weapon → weapon）。
function M.equip()
  if not ui_state then
    return false, "not_open"
  end
  local row = current_row()
  if not row then
    ui_state.msg = "背包为空"
    M.render()
    return false, "not_enough"
  end
  local kind = M.kind_of(row.item)
  local slot = (kind == "tool" or kind == "weapon") and kind or nil
  if not slot then
    ui_state.msg = M.name_of(row.item) .. "：" .. REASON_TEXT.not_equippable
    audio.ui("ui_cancel")
    M.render()
    return false, "not_equippable"
  end
  local ok, err = pcall(inv.equip, state.inventory, slot, row.item)
  if not ok then
    ui_state.msg = "装备失败：" .. tostring(err)
    audio.ui("ui_cancel")
    M.render()
    return false, "no_inventory"
  end
  state.last_action = "equip_" .. row.item
  ui_state.msg = "已装备 " .. M.name_of(row.item) .. "（" .. slot .. "）"
  audio.ui("ui_confirm")
  M.render()
  return true, "ok"
end

-- 把选中的种子设为当前播种种子（farm.lua 读 state.selected_seed）。
function M.choose_seed()
  if not ui_state then
    return false, "not_open"
  end
  local row = current_row()
  if not row then
    return false, "not_enough"
  end
  if M.kind_of(row.item) ~= "seed" then
    ui_state.msg = M.name_of(row.item) .. "：" .. REASON_TEXT.not_seed
    audio.ui("ui_cancel")
    M.render()
    return false, "not_seed"
  end
  state.selected_seed = row.item
  state.last_action = "select_seed_" .. row.item
  ui_state.msg = "已选中种子：" .. M.name_of(row.item)
  audio.ui("ui_confirm")
  M.render()
  return true, "ok"
end

-- ---------- 绘制 ----------

function M.render()
  if not ui_state then
    return false
  end
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local BOX = layout.BOX
  local snapshot = M.snapshot()
  local rows = snapshot.slots
  -- 行数会随物品变化：每次重绘同步焦点环项数（保持当前选择）。
  sync_focus()
  ui_text.begin("bp")
  -- docs/48 ui 换肤接线：主面板纸面背景（9-slice）叠在纯色兜底之上。
  skin.panel("bp:skin_bg", x, y, w, h, "panel_main")
  ui.panel("bp:box", x, y, w, h, 0.10, 0.11, 0.15, 0.94)
  -- 标题下的分隔线 + 选中行的底衬（竖版单列，靠底衬定位"当前选中哪一行"）。
  -- docs/48 ui 换肤接线：标题分隔线用 common 竹纹分隔贴图（叠在原纯色线之上）。
  skin.icon("bp:divider", x + BOX.title_dx, y + BOX.title_dy + BOX.title_font + 6, w - BOX.title_dx * 2, 8, "divider")
  ui.panel("bp:rule", x + BOX.title_dx, y + BOX.title_dy + BOX.title_font + 14, w - BOX.title_dx * 2, 2, 0.72, 0.75, 0.82, 0.22)
  ui.panel(
    "bp:sel",
    x + BOX.list_dx,
    y + BOX.list_dy + ui_state.index * ROW_H,
    w - BOX.list_dx * 2,
    ROW_H,
    0.85,
    0.72,
    0.42,
    0.20
  )
  ui_text.draw("bp", "bp:title", x + BOX.title_dx, y + BOX.title_dy, BOX.title_font, layout.list_width(PANEL), "背包")
  -- docs/48 ui 换肤接线：标题右端印章（24×24）。
  skin.icon("bp:seal", x + w - BOX.title_dx - 24, y + BOX.title_dy + 2, 24, 24, "icon_seal")
  -- 容量/负重：标题行右半区（竖版面板宽 592，右半区仍有 300px 以上）。
  ui_text.draw(
    "bp",
    "bp:cap",
    x + w - 300,
    y + BOX.corner_y,
    BOX.corner_font,
    276.0,
    "容量 " .. tostring(snapshot.occupied) .. "/" .. tostring(snapshot.capacity)
      .. " · 负重 " .. tostring(snapshot.weight)
  )

  if #rows == 0 then
    ui_text.draw("bp", "bp:empty", x + BOX.list_dx, y + BOX.list_dy + ROW_H, 18.0, layout.list_width(PANEL), "（空）")
  else
    ui_text.hide("bp", "bp:empty")
  end

  for i = 0, ROW_LIMIT - 1 do
    local id = "bp:row" .. (i + 1)
    local row = rows[i + 1]
    if row then
      local col = math.floor(i / ROWS_PER_COL)
      local line = math.floor(i % ROWS_PER_COL)
      local kind = KIND_TEXT[M.kind_of(row.item)] or "物品"
      local label = string.format("%d. %s x%d  [%s]", i + 1, M.name_of(row.item), row.count, kind)
      if i == ui_state.index then
        label = "> " .. label
      end
      -- 列宽（M32）：单列时整行可用 `w - 2*24`（行尾只留一个内边距）。
      local col_w = col == 0 and (w - COL_X[1] - 24) or layout.list_width(PANEL)
      -- docs/48 ui 换肤接线：非选中行槽框（16×16，画在行首左外侧；选中行另有 slot_selected）。
      if i ~= ui_state.index then
        skin.icon("bp:slot" .. (i + 1), x + COL_X[col + 1] - 22, y + BOX.list_dy + line * ROW_H + 2, 16, 16, "slot")
      end
      ui_text.draw("bp", id, x + COL_X[col + 1], y + BOX.list_dy + line * ROW_H, 16.0, col_w, label)
    else
      ui_text.hide("bp", id)
    end
  end

  -- 详情：选中物品的说明 + 使用效果提示。
  local row = rows[ui_state.index + 1]
  if row then
    local def = data_of(row.item) or {}
    -- 详情里的操作提示也按当前设备取名（改键后同步），避免写死 U/E/C。
    local use_key = hints.binding_labels({ "use" })
    local equip_key = hints.binding_labels({ "interact" })
    local seed_key = hints.binding_labels({ "seed" })
    local effect = type(def.use) == "table" and (" · 可使用（" .. tostring(use_key or "") .. "）") or ""
    local equip = (def.kind == "tool" or def.kind == "weapon")
        and (" · 可装备（" .. tostring(equip_key or "") .. "）")
      or ""
    local seed = def.kind == "seed" and (" · 设为种子（" .. tostring(seed_key or "") .. "）") or ""
    local text = tostring(def.description or "") .. effect .. equip .. seed
    -- docs/48 ui 换肤接线：选中行的槽框（slot_selected）与品质框（凡品，40×40 原尺寸）。
  skin.icon("bp:sel_slot", x + w - BOX.list_dx - 84, y + BOX.list_dy + ui_state.index * ROW_H + (ROW_H - 40) / 2, 40, 40, "slot_selected")
  skin.icon("bp:quality", x + w - BOX.list_dx - 42, y + BOX.list_dy + ui_state.index * ROW_H + (ROW_H - 40) / 2, 40, 40, "quality_frame_0")
  ui_text.draw("bp", "bp:detail", x + BOX.list_dx, y + h + BOX.detail_dy, 14.0, layout.list_width(PANEL), text)
  else
    ui_text.hide("bp", "bp:detail")
  end

  ui_text.draw("bp", "bp:msg", x + BOX.list_dx, y + h + BOX.msg_dy, 16.0, layout.list_width(PANEL), tostring(ui_state.msg or ""))
  -- docs/48 ui 换肤接线：按键提示条背景（key_hint 9-slice）。
  skin.panel("bp:key_hint", x + BOX.list_dx, y + h + BOX.hint_dy - 6, layout.list_width(PANEL), BOX.hint_font + 12, "key_hint")
  ui_text.draw("bp", "bp:hint", x + BOX.list_dx, y + h + BOX.hint_dy, BOX.hint_font, layout.list_width(PANEL), hint_line())
  return true
end

-- ---------- 打开 / 关闭 ----------

function M.open()
  ui_state = { index = 0, msg = nil }
  state.set_mode("Inventory")
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
  if state.is_mode("Inventory") then
    state.set_mode("Playing")
  end
  audio.ui("ui_cancel")
  return true
end

function M.reset()
  ui_state = nil
  clear_ui()
  nav.close(FOCUS)
  return true
end

return M
