-- systems/shipping.lua：《江湖有田》出货箱（镖局收单，docs/33 §4.1.7，2026-10-06 0.8）。
--
-- 与星露谷出货箱同构、武侠化为"镖局收单"：山庄门口货箱，白天把背包里的
-- 可卖物放入（立即离开背包），睡前统一结算（on_day_start 挂点），按物品 sell
-- 价入账，并写出货记录（图鉴/成就数据源）。
--
-- 设计要点：
--   * 入箱范围 = 所有 sell > 0 的物品（收获物/树果/战利品等）；种子/工具/
--     肥料 sell=0 不可入箱（与商店套利约束同向）。
--   * 结算价 = 物品 sell（与商店收购同价，无折价）；**不依赖商店 buys 表**——
--     任何可卖物都能入箱，是"免跑商店的批量出货 + 记录"。
--   * 未入箱的物品睡前不入账（留在背包）；放入后当天反悔可再取出。
--   * 记录三件套（全部随存档经 save_manager 持久）：
--       pending   { item_id → count }   当前货箱内待结算清单
--       stats     { item_id → total }   累计出货量（图鉴/成就数据源）
--       log       最近 LOG_LIMIT 条当日结算明细（date/items/total）
--   * 结算幂等：pending 为空时 settle() 返回 nil，可安全每日调用。
--
-- UI：独立 "Shipping" 模式（ui/input.lua 分发），列表 = 背包可卖物
-- （名称/持有/单价），空格/A 确认 = 放 1 个入箱，Esc/B 关闭。
local state = require("core.state")
-- inv / ui / grid / scene / clock / transform 是引擎注入的全局模块（与
-- player/inventory.lua、systems/farm.lua 同一用法），此处仅取局部引用。
local inv = inv
local ui = ui
local items = require("data.items")
local tools = require("data.tools")
local interaction = require("systems.interaction")
local player = require("player.player")
local calendar = require("world.calendar")
local audio = require("core.audio")
local nav = require("ui.panel_nav")
local ui_text = require("ui.ui_text")
local layout = require("ui.layout")
-- M69/B6：面板静态骨架搬进 `assets/prefabs/shipping.prefab`，不再 require ui.skin。

local M = {}

local TILE = 32
local ROW_LIMIT = 9 -- 与数字键 1..9 一致
local ROW_H = 34
local LOG_LIMIT = 30 -- 出货记录保留条数（防存档膨胀）

local FOCUS = "ship:focus"
local SCROLL = "ship:scroll"
local PANEL = layout.PANEL.shop

-- 货箱所在格（山庄门口内侧，出口格 (16,1) 左侧一格；玩家站在 (15,2)
-- 任一相邻格朝它按 E 即可交互）。与地图/出口不重叠。
local CRATE_CELL = { x = 15, y = 2 }

-- 出货记录日期的季节中文名（与 data/festivals 季节序一致）。
local SEASON_CN = { spring = "春", summer = "夏", autumn = "秋", winter = "冬" }

-- 运行时状态（capture/restore 随存档持久；reset 清空）
local pending = {} -- item_id → count
local stats = {} -- item_id → total_count
local log = {} -- { { date=, items={ {id=,count=,amount=},... }, total= }, ... }
local ui_state = nil -- { index, msg }

-- 阶段 2 分系统扩展物品（data/items_<system>.lua，如钓鱼的鱼）：与
-- player/inventory.lua 同一批可选模块，缺失即跳过。纯追加回退——既有物品仍优先
-- 命中 data.items/data.tools，顺序与数值不变（M42 锚点不受影响）。
local ITEM_EXT_MODULES = {
  "data.items_animal",
  "data.items_fish",
  "data.items_mine",
  "data.items_cooking",
  "data.items_stage3"
}

local function ext_item_def(item_id)
  for _, module_name in ipairs(ITEM_EXT_MODULES) do
    local ok, ext = pcall(require, module_name)
    if ok and type(ext) == "table" and ext.items and ext.items[item_id] then
      return ext.items[item_id]
    end
  end
  return nil
end

local function item_def(item_id)
  if type(item_id) ~= "string" then
    return nil
  end
  return items[item_id] or tools[item_id] or ext_item_def(item_id)
end

-- 入箱价：物品 sell（必须 > 0 才可入箱）。
local function sell_price(item_id)
  local def = item_def(item_id)
  local sell = def and tonumber(def.sell) or 0
  return sell > 0 and sell or 0
end

local function inventory_items()
  -- 背包全部非空槽位（`inv.get` 返回 { id, capacity, slots={...}, occupied }，slots 是
  -- 稀疏表——引擎只对非空槽 set——所以用 pairs 遍历 data.slots。顺序对 shippable 无影响：
  -- shippable 按物品 id 排序，先收集后排序。**M69 顺手修**：原 `ipairs(data)` 遍历的是
  -- 返回表本身的数字键（恒空），出货箱 0.8 上线后从未真正列出过可入箱物。）
  local out = {}
  local data = inv.get(state.inventory)
  if type(data) ~= "table" then
    return out
  end
  for _, slot in pairs(data.slots or {}) do
    if slot and slot.item and (tonumber(slot.count) or 0) > 0 then
      out[#out + 1] = { item = slot.item, count = tonumber(slot.count) }
    end
  end
  return out
end

-- 可入箱清单：sell > 0 且背包有持有（按物品 id 排序，UI/验收顺序稳定）。
function M.shippable()
  local by_id = {}
  for _, slot in ipairs(inventory_items()) do
    if sell_price(slot.item) > 0 then
      by_id[slot.item] = (by_id[slot.item] or 0) + slot.count
    end
  end
  local ids = {}
  for id in pairs(by_id) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  local out = {}
  for _, id in ipairs(ids) do
    out[#out + 1] = { item = id, have = by_id[id], price = sell_price(id) }
  end
  return out
end

-- 当前待结算金额（预览；不含未入箱物品）。
function M.pending_total()
  local total = 0
  for item_id, count in pairs(pending) do
    total = total + sell_price(item_id) * count
  end
  return total
end

-- 放入货箱：sell > 0 且背包持有足够 → 移出背包计入 pending。
-- 返回值 (true,"ok") 或 (false,reason)；reason ∈ not_shippable / not_enough。
function M.add(item_id, count)
  local units = math.floor(tonumber(count) or 0)
  if units <= 0 then
    return false, "bad_count"
  end
  if sell_price(item_id) <= 0 then
    return false, "not_shippable"
  end
  if (tonumber(inv.count(state.inventory, item_id)) or 0) < units then
    return false, "not_enough"
  end
  inv.remove(state.inventory, item_id, units)
  pending[item_id] = (pending[item_id] or 0) + units
  state.last_action = "ship_" .. item_id
  audio.ui("ui_confirm")
  return true, "ok"
end

-- 反悔取出：把 pending 里 count 个放回背包（不超持有）。
function M.remove(item_id, count)
  local units = math.floor(tonumber(count) or 0)
  local have = tonumber(pending[item_id]) or 0
  if units <= 0 or have <= 0 then
    return false, "not_in_box"
  end
  if units > have then
    units = have
  end
  pending[item_id] = have - units
  if pending[item_id] <= 0 then
    pending[item_id] = nil
  end
  inv.add(state.inventory, item_id, units)
  state.last_action = "unbox_" .. item_id
  return true, "ok"
end

-- 睡前统一结算（main.lua on_day_start 挂点，在 loot.bank 之后、目标结算之前）：
-- pending 按 sell 价入账 → 累计 stats → 追加当日记录 → 清空 pending。
-- 幂等：pending 为空返回 nil。
function M.settle()
  local entries = {}
  local total = 0
  for item_id, count in pairs(pending) do
    if type(item_id) == "string" and count > 0 then
      local price = sell_price(item_id)
      local amount = price * count
      total = total + amount
      stats[item_id] = (stats[item_id] or 0) + count
      entries[#entries + 1] = { id = item_id, count = count, amount = amount }
    end
  end
  if #entries == 0 then
    pending = {}
    return nil
  end
  table.sort(entries, function(a, b)
    return a.id < b.id
  end)
  state.money = (tonumber(state.money) or 0) + total
  local season = SEASON_CN[calendar.current()] or "?"
  local day = tonumber(clock.now().day) or 1
  log[#log + 1] = {
    date = season .. " " .. calendar.day_in_season(day) .. " 日",
    items = entries,
    total = total,
  }
  while #log > LOG_LIMIT do
    table.remove(log, 1)
  end
  pending = {}
  audio.ui("ui_confirm")
  return { total = total, entries = entries }
end

-- 待结算清单访问器（验收/诊断用；调用方只读）。
function M.pending()
  return pending
end

-- 出货记录访问器（图鉴/成就数据源）。
function M.log()
  return log
end

-- 累计出货量：item_id → total_count（图鉴/成就数据源）。
function M.stats()
  return stats
end

function M.crate_cell()
  return CRATE_CELL
end

-- ---------- 存档 ----------

function M.capture()
  local pending_data, stats_data = {}, {}
  for id, count in pairs(pending) do
    if type(id) == "string" and count > 0 then
      pending_data[id] = count
    end
  end
  for id, count in pairs(stats) do
    if type(id) == "string" and count > 0 then
      stats_data[id] = count
    end
  end
  local log_data = {}
  for index, entry in ipairs(log) do
    local safe_items = {}
    for _, it in ipairs(entry.items or {}) do
      safe_items[#safe_items + 1] = {
        id = it.id,
        count = it.count,
        amount = it.amount,
      }
    end
    log_data[index] = { date = entry.date, items = safe_items, total = entry.total }
  end
  return { pending = pending_data, stats = stats_data, log = log_data }
end

function M.restore(data_in)
  pending, stats, log = {}, {}, {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.pending) == "table" then
    for id, count in pairs(data_in.pending) do
      if type(id) == "string" and tonumber(count) and count > 0 and sell_price(id) > 0 then
        pending[id] = math.floor(count)
      end
    end
  end
  if type(data_in.stats) == "table" then
    for id, count in pairs(data_in.stats) do
      if type(id) == "string" and tonumber(count) and count > 0 then
        stats[id] = math.floor(count)
      end
    end
  end
  if type(data_in.log) == "table" then
    for _, entry in ipairs(data_in.log) do
      if type(entry) == "table" and type(entry.date) == "string" and type(entry.items) == "table" then
        log[#log + 1] = entry
      end
    end
    while #log > LOG_LIMIT do
      table.remove(log, 1)
    end
  end
end

function M.reset()
  pending, stats, log = {}, {}, {}
  ui_state = nil
  clear_ui()
  preset_hide()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
end

-- ---------- UI（"Shipping" 模式；ui/input.lua 分发） ----------

local function clear_ui()
  ui.clear("ship:box")
  ui.clear("ship:skin_bg")
  ui.clear("ship:rule")
  ui.clear("ship:sel")
  for _, id in ipairs({ "ship:divider", "ship:seal", "ship:close", "ship:scroll_up", "ship:scroll_down" }) do
    ui.clear(id)
  end
  for _, id in ipairs({ "ship:title", "ship:money", "ship:sum", "ship:msg", "ship:hint" }) do
    ui_text.hide("ship", id)
  end
  for i = 1, ROW_LIMIT do
    ui_text.hide("ship", "ship:row" .. i)
  end
  ui_text.reset("ship")
end

-- ---------- 预设（M69/B6） ----------
-- 静态骨架搬进 `assets/prefabs/shipping.prefab`（对账门 = jhyt_m69_shipping_prefab，黄金表
-- 46 行钉死迁移前画面），render 只写会变的部分：选中行矩形、金币/待结算/反馈文字、出货行
-- 文本、滚动箭头的条件显隐与行数不足时的行隐藏。title 与 hint 固定写死在预设。
-- 开/关屏 = 根节点显隐（`prefab.set_visible` 子树语义），不撤重铺（M63c 教训）。
local PRESET_PATH = "assets/prefabs/shipping.prefab"
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
  prefab.set_visible(preset_handle, "ShipRoot", true)
  return true
end

local function preset_hide()
  if preset_live() then
    prefab.set_visible(preset_handle, "ShipRoot", false)
  end
  return true
end

function M.ui_active()
  return ui_state ~= nil
end

function M.selected()
  return ui_state and ui_state.index or 0
end

function M.row_limit()
  return ROW_LIMIT
end

local function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #M.shippable(), 1)
  nav.select(FOCUS, ui_state.index)
end

function M.select(index)
  if not ui_state then
    return false
  end
  local i = math.floor(tonumber(index) or 0)
  if i < 1 or i > #M.shippable() then
    return false
  end
  ui_state.index = i
  sync_focus()
  return true
end

function M.move(delta)
  if not ui_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  local rows = M.shippable()
  if step == 0 or #rows == 0 then
    return false
  end
  sync_focus()
  local direction = step > 0 and "down" or "up"
  local index = nav.move(FOCUS, direction)
  if not index then
    index = math.max(1, math.min(#rows, ui_state.index + step))
  end
  if index == ui_state.index then
    return false
  end
  ui_state.index = index
  return true
end

function M.move_dir(direction)
  if not ui_state or #M.shippable() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  ui_state.index = index
  return true
end

-- 确认：把当前高亮物品放 1 个入箱（msg 反馈；放入成功自动重绘）。
function M.confirm()
  if not ui_state then
    return false
  end
  local rows = M.shippable()
  local row = rows[ui_state.index]
  if not row then
    return false
  end
  local ok, reason = M.add(row.item, 1)
  ui_state.msg = ok and (row.item .. " 已放入货箱") or (reason or "无法放入")
  if ok then
    audio.ui("ui_confirm")
    if ui_state.index > #M.shippable() then
      ui_state.index = math.max(1, #M.shippable())
    end
    sync_focus()
  end
  return ok
end

function M.open_ui()
  if ui_state then
    return false
  end
  ui_state = { index = 1, msg = nil }
  state.set_mode("Shipping")
  sync_focus()
  audio.ui("ui_click")
  preset_show()
  M.render()
  return true
end

function M.close_ui()
  if not ui_state then
    return false
  end
  ui_state = nil
  clear_ui()
  preset_hide()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  if state.is_mode("Shipping") then
    state.set_mode("Playing")
  end
  audio.ui("ui_cancel")
  return true
end

function M.render()
  if not ui_state then
    return false
  end
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local BOX = layout.BOX
  local ROW_W = layout.list_width(PANEL)
  sync_focus()
  preset_spawn()
  local P = preset_handle
  -- 选中行底衬：y 随选中行（(index-1)*34）移动；shipping 的 sel 首行是 y+100
  -- （不是 list_dy=60），矩形 = 迁移前 `ui.panel("ship:sel", …)` 的几何。
  local sel_top = y + 100 + (ui_state.index - 1) * ROW_H
  prefab.set_rect(P, "ShipSel", x + BOX.list_dx, sel_top, x + w - BOX.list_dx, sel_top + ROW_H)
  prefab.set_text(P, "ShipMoney", "金币 " .. tostring(state.money) .. " 文")
  prefab.set_text(P, "ShipSum", "箱内待结算 " .. tostring(M.pending_total()) .. " 文")

  local rows = M.shippable()
  -- 滚动箭头条件显隐：行数超过一页才画（黄金表 A 态 10 种 > 9 显示）。
  local scrollable = #rows > ROW_LIMIT
  prefab.set_visible(P, "ShipScrollUp", scrollable)
  prefab.set_visible(P, "ShipScrollDown", scrollable)

  local first = nav.window(SCROLL, #rows, ROW_LIMIT, ROW_H, ui_state.index)
  for i = 1, ROW_LIMIT do
    local row = rows[first + i - 1]
    local id = "ShipRow" .. i
    if row then
      local line = tostring(row.item) .. " × " .. tostring(row.have) .. "  单价 " .. tostring(row.price)
      if i == ui_state.index - first + 1 then
        line = "→ " .. line
      end
      prefab.set_text(P, id, line)
      prefab.set_visible(P, id, true)
    else
      -- M69/C5 顺手修：原 `for i = 1, math.min(ROW_LIMIT, #rows)` 没有 else 隐藏分支，
      -- 行数从 9 掉到 3 时第 4~9 行留着上一帧的字；预设化后行不足即隐藏。
      prefab.set_visible(P, id, false)
    end
  end
  prefab.set_text(P, "ShipMsg", tostring(ui_state.msg or ""))
  return true
end

-- ---------- 交互目标 ----------

local registered = false
function M.ensure_target()
  if registered then
    return
  end
  registered = true
  interaction.register({
    id = "shipping",
    priority = 10, -- 高于农田 0、低于出口 20（门口按 E 必能离开）
    query = function(facing)
      local map_id = scene.reload()
      if map_id ~= "manor" then
        return nil, "not_manor"
      end
      local pos = player.position()
      if not pos then
        return nil, "no_player"
      end
      local cell = grid.facing_cell(pos.x, pos.y, facing, TILE)
      if not cell then
        return nil, "no_player"
      end
      if cell[1] == CRATE_CELL.x and cell[2] == CRATE_CELL.y then
        return { cell = { x = cell[1], y = cell[2] }, distance = 1, facing = 1 }
      end
      return nil, "no_crate"
    end,
    can_interact = function(candidate)
      return candidate ~= nil
    end,
    prompt = function()
      return "E: 镖局货箱（出货）"
    end,
    interact = function()
      if not M.open_ui() then
        return "already_open"
      end
      return "shipping"
    end,
  })
end

-- ---------- 加载期校验 ----------

-- 纯系统模块：无可注册数据表；validate_all 只做自检（货箱格在庄园地图范围内，
-- 且不与出口格 (16,1) 重合——重合会让出口 priority 20 永远抢走交互）。
function M.validate_all()
  local report = {}
  if CRATE_CELL.x < 0 or CRATE_CELL.y < 0 then
    report[#report + 1] = "shipping: crate cell 非法"
  end
  if CRATE_CELL.x == 16 and CRATE_CELL.y == 1 then
    report[#report + 1] = "shipping: crate cell 与山庄出口格 (16,1) 重合"
  end
  return table.concat(report, "\n")
end

return M
