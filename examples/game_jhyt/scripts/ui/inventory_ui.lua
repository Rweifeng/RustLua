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
-- M67/B4：面板静态骨架搬进 `assets/prefabs/ui/inventory.prefab`，不再 require ui.skin。

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

-- ---------- 预设（M67/B4） ----------
-- 静态骨架搬进 `assets/prefabs/ui/inventory.prefab`（对账门 = jhyt_m67_inventory_prefab，
-- 黄金表 70 行钉死迁移前画面），render 只写会变的部分：选中行矩形、行文本、容量/负重、
-- 详情/消息/提示、空背包与空槽的显隐。title 固定写死在预设里，不再逐帧写。
-- 开/关屏 = 根节点显隐（`prefab.set_visible` 子树语义，藏父带子、藏节点不建布局项），
-- 不撤重铺，避免槽位 LIFO 复用打乱绘制序（M63c 教训）。
local PRESET_PATH = "assets/prefabs/ui/inventory.prefab"
local preset_handle = nil
local preset_spawned = false

-- M71 修复：场景切换重建 ECS 世界后旧句柄失效，纯布尔标志漏判 → set_text 报「没有 handle N」；
-- 改成对句柄真实探活（`prefab.root` 失效句柄抛错，pcall 兜底）。
local function preset_live()
  if not preset_handle then
    return false
  end
  local ok, alive = pcall(function() return prefab.root(preset_handle) ~= nil end)
  return ok and alive == true
end

local function preset_spawn()
  if preset_handle and preset_live() then
    return true
  end
  preset_handle = prefab.spawn(PRESET_PATH)
  preset_spawned = true
  return true
end

local function preset_show()
  preset_spawn()
  prefab.set_visible(preset_handle, "InvRoot", true)
  return true
end

local function preset_hide()
  if preset_live() then
    prefab.set_visible(preset_handle, "InvRoot", false)
  end
  return true
end

-- 验收（jhyt_m19_ui 面板断言）取 spawn 句柄；open 前为 nil。
function M.preset_ref()
  return preset_handle
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
  preset_spawn()
  local P = preset_handle
  -- 选中行底衬：y 随选中行（index*40）移动；矩形 = 迁移前 `ui.panel("bp:sel", …)` 的
  -- 几何（left, top, right, bottom 四边，`prefab.set_rect` 签名）。
  local sel_top = y + BOX.list_dy + ui_state.index * ROW_H
  prefab.set_rect(P, "InvSel", x + BOX.list_dx, sel_top, x + w - BOX.list_dx, sel_top + ROW_H)
  -- 容量/负重：标题行右半区（竖版面板宽 592，右半区仍有 300px 以上）。
  prefab.set_text(
    P,
    "InvCap",
    "容量 " .. tostring(snapshot.occupied) .. "/" .. tostring(snapshot.capacity)
      .. " · 负重 " .. tostring(snapshot.weight)
  )

  -- 空背包互斥：有物品时隐藏"（空）"。
  prefab.set_visible(P, "InvEmpty", #rows == 0)

  for i = 0, ROW_LIMIT - 1 do
    local row = rows[i + 1]
    local row_name = "InvRow" .. (i + 1)
    local slot_name = "InvSlot" .. (i + 1)
    if row then
      local kind = KIND_TEXT[M.kind_of(row.item)] or "物品"
      local label = string.format("%d. %s x%d  [%s]", i + 1, M.name_of(row.item), row.count, kind)
      if i == ui_state.index then
        label = "> " .. label
      end
      prefab.set_text(P, row_name, label)
      prefab.set_visible(P, row_name, true)
      -- 非选中行画槽框；选中行不画（右侧另有 slot_selected 槽框）。
      prefab.set_visible(P, slot_name, i ~= ui_state.index)
    else
      prefab.set_visible(P, row_name, false)
      prefab.set_visible(P, slot_name, false)
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
    prefab.set_text(P, "InvDetail", text)
    prefab.set_visible(P, "InvDetail", true)
    -- 选中行右侧：槽框（slot_selected）与品质框（40×40 原尺寸），y 随选中行移动。
    local side_top = sel_top + (ROW_H - 40) / 2
    prefab.set_rect(P, "InvSelSlot", x + w - BOX.list_dx - 84, side_top, x + w - BOX.list_dx - 84 + 40, side_top + 40)
    prefab.set_rect(P, "InvQuality", x + w - BOX.list_dx - 42, side_top, x + w - BOX.list_dx - 42 + 40, side_top + 40)
    prefab.set_visible(P, "InvSelSlot", true)
    prefab.set_visible(P, "InvQuality", true)
  else
    prefab.set_visible(P, "InvDetail", false)
    prefab.set_visible(P, "InvSelSlot", false)
    prefab.set_visible(P, "InvQuality", false)
  end

  prefab.set_text(P, "InvMsg", tostring(ui_state.msg or ""))
  prefab.set_text(P, "InvHint", hint_line())
  return true
end

-- ---------- 打开 / 关闭 ----------

function M.open()
  ui_state = { index = 0, msg = nil }
  state.set_mode("Inventory")
  audio.ui("ui_click")
  preset_show()
  M.render()
  return true
end

function M.close()
  if not ui_state then
    return false
  end
  ui_state = nil
  clear_ui()
  preset_hide()
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
  preset_hide()
  nav.close(FOCUS)
  return true
end

return M
