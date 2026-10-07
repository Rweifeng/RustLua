-- shop.lua：商店经济运行时（M17-04）。
--
-- 数据定义见 data/shops.lua；新增商店只改数据，不改本文件与 Rust 核心（验收项①）。
-- 本模块同时负责：
--   * 营业时间判定（clock.now().minute_of_day 落在 hours 窗口内）
--   * 原子购买 / 出售（金币、每日限购、背包容量任一不满足都不产生半完成状态）
--   * 购买价按**店主阵营的声望档位**打折（M34-01，效果数据见 data/relationships.lua
--     的 reputation_tiers[].effects.price_off；零声望 = 原价，出售价不受影响）
--   * 每日库存刷新（on_day_start → 限购计数清零，随存档持久化）
--   * 商店面板 UI（纯键盘：W/S 或方向键选择 / F 切换买卖 / +/- 数量 /
--     空格确认 / Esc 关闭；M20-02 方向键走引擎焦点环，选中行超出可视窗口时
--     列表按 ScrollState 滚动）
--   * 与店主 NPC 的交互目标注册
--
-- 输入在 input.lua 的 Shop 分支读取后调用本模块，本模块不 require input
-- （否则与 input → shop 形成 require 环）。
--
-- 引用完整性由 validate_all 在加载期校验（硬风险①），运行时不做静默兜底。
local state = require("core.state")
local npcs = require("data.npcs")
local places = require("data.places")
local items = require("data.items")
local tools = require("data.tools")
local interaction = require("systems.interaction")
local player = require("player.player")
local npc = require("world.npc")
-- M34-01：档位接线——商店价格读**店主阵营**的声望折扣（只读查询，不写关系状态）。
local relationship = require("player.relationship")
-- M34-02：端午药集——收购价读今日节气的行情加成（同样是只读查询，节气本身不入档）。
local festival = require("world.festival")
-- M19-02：交易成功/失败音效（缺失文件或无设备时静默降级）；M20-02 走 UI 总线。
local audio = require("core.audio")
-- M20-02：列表焦点/滚动导航（引擎 ui_navigation）。
local nav = require("ui.panel_nav")
-- M20-09：键鼠/手柄两套操作说明（本模块不 require ui.input，避免 require 环）。
local hints = require("ui.hints")
-- M30：面板文字按真实字宽裁剪（长物品名/大额金币不再画到面板外）。
local ui_text = require("ui.ui_text")
-- M32 竖版：面板矩形与行偏移的唯一来源（`ui/layout.lua`）。
local layout = require("ui.layout")
-- M65/B2 预设化：商店面板已整体搬入 `assets/prefabs/shop.prefab`（对账门 =
-- `jhyt_m65_shop_prefab`，黄金表 116 行钉死迁移前画面；"搬运不改画面"）。
-- 面板贴图/几何/字号全部来自预设，脚本运行期只写 `prefab.set_text` / `set_rect` /
-- `set_visible`，不再画动态层（docs/48 换肤贴图随之并入预设，skin 模块不再需要）。

local M = {}

local TILE = 32
local INTERACT_RANGE = 56        -- 世界单位，约 1.75 格（与 dialogue 一致）
local MINUTES_PER_DAY = 1440
local ROW_LIMIT = 9              -- 与数字键 1..9 一致
local MAX_QUANTITY = 99
local ROW_H = 34

local FOCUS = "shp:focus"
local SCROLL = "shp:scroll"

-- 面板几何（M32 竖版）：672×720 的单列面板，来自 `ui/layout.lua`。
-- 横屏时是"640×440、1280×720 居中"的双列宽面板；竖屏只有 720 宽，于是改为
-- 单列 9 行（行宽 544px，物品名 + 单价 + 限购/持有都在一行内放得下）。
local PANEL = layout.PANEL.shop

-- 预设路径与句柄：每世界 spawn 一次（幂等）；换场景世界重建、登记表清空后自动重铺。
local PRESET_PATH = "assets/prefabs/shop.prefab"
local shop_handle = nil

-- 每日限购计数：shop_id → { item_id → 已购数量 }。随存档持久化。
local used = {}
-- 面板状态：{ shop, tab = "buy"|"sell", index, quantity, msg }；nil 表示未打开。
local ui_state = nil

-- 前向声明（定义在 select/move 附近；Lua 词法作用域只认已声明的 local）。
local sync_focus
local ring_step

local REASON_TEXT = {
  closed = "未营业",
  poor = "金币不足",
  full = "背包已满",
  limit = "已达每日限购",
  not_enough = "数量不足",
  not_sold = "本店不出售该物品",
  not_bought = "本店不收购该物品",
  bad_count = "数量无效",
  unknown_shop = "未知商店",
  no_effect = "该物品没有使用效果"
}

-- ---------- 数据访问 ----------

local registry = nil
local function defs()
  if registry == nil then
    local ok, mod = pcall(require, "data.shops")
    registry = (ok and type(mod) == "table") and mod or false
  end
  return registry or {}
end

function M.get(id)
  if type(id) ~= "string" then
    return nil
  end
  local def = defs()[id]
  return type(def) == "table" and def or nil
end

-- 注册表 id 升序列表（遍历/存档顺序确定）。
function M.ids()
  local out = {}
  for id, def in pairs(defs()) do
    if type(id) == "string" and type(def) == "table" then
      out[#out + 1] = id
    end
  end
  table.sort(out)
  return out
end

local function item_def(item_id)
  if type(item_id) ~= "string" then
    return nil
  end
  return items[item_id] or tools[item_id]
end

local function positive(value)
  local n = tonumber(value)
  return n ~= nil and n > 0
end

-- 商品单价：显式 price 优先，缺省回落到物品定义 buy。
local function stock_price(entry)
  local price = tonumber(entry.price)
  if price == nil then
    local def = item_def(entry.item)
    price = def and tonumber(def.buy) or nil
  end
  return price
end

-- 店主阵营的声望折扣（整数百分数，docs/33 §8.2 的 price_off 效果）；
-- 店主无阵营 → 0。陌生档 effects 为空 = 原价，所以零声望时逐字节等于改动前。
local function keeper_price_off(def)
  local keeper = def and def.keeper
  local faction = keeper and relationship.faction_of(keeper)
  if not faction then
    return 0
  end
  return relationship.reputation_price_off(faction)
end

-- 成交价 = 标价按店主阵营档位打折：整数结算（price * off / 100 四舍五入后扣除），
-- 最低 1 文，杜绝"打折打成 0 文"的免费刷物。出售不受折扣影响（§8.2 只写购买折扣）。
local function deal_price(def, entry)
  local price = stock_price(entry)
  local off = keeper_price_off(def)
  if price == nil or off <= 0 then
    return price
  end
  return math.max(1, price - math.floor(price * off / 100 + 0.5))
end

-- 收购单价 = 物品 sell 价按今日节气行情加价（端午药集 +20%），整数结算（四舍五入）。
-- 无行情 = 物品 sell 价原样，所以零改动时逐字节等于改动前；收购**不受声望折扣影响**
-- （§8.2 只写购买折扣，把折扣也套到收购上等于两头让利，那是另一个数值决定）。
local function sell_price(item_id)
  local base = tonumber((item_def(item_id) or {}).sell)
  local bonus = festival.sell_bonus(item_id)
  if base == nil or bonus <= 0 then
    return base
  end
  return math.max(1, base + math.floor(base * bonus / 100 + 0.5))
end

local function stock_entry(def, item_id)
  for _, entry in ipairs(def.stock or {}) do
    if entry.item == item_id then
      return entry
    end
  end
  return nil
end

local function buys_item(def, item_id)
  for _, id in ipairs(def.buys or {}) do
    if id == item_id then
      return true
    end
  end
  return false
end

local function used_of(shop_id, item_id)
  local counts = used[shop_id]
  return counts and counts[item_id] or 0
end

local function mark_used(shop_id, item_id, count)
  used[shop_id] = used[shop_id] or {}
  used[shop_id][item_id] = used_of(shop_id, item_id) + count
end

-- ---------- 营业时间 ----------

-- 左闭右开的分钟窗口判定；缺省 hours 视为全天营业。
function M.is_open(shop_id)
  local def = M.get(shop_id)
  if not def then
    return false, "unknown_shop"
  end
  local hours = def.hours
  if type(hours) ~= "table" then
    return true, "ok"
  end
  local open = tonumber(hours.open) or 0
  local close = tonumber(hours.close) or MINUTES_PER_DAY
  local minute = clock.now().minute_of_day
  if close <= open then
    return false, "closed"
  end
  if minute < open or minute >= close then
    return false, "closed"
  end
  return true, "ok"
end

-- ---------- 库存 / 商品视图 ----------

-- 本店在售商品（含当日剩余限购），顺序与数据一致。
function M.stock(shop_id)
  local def = M.get(shop_id)
  local out = {}
  if not def then
    return out
  end
  for _, entry in ipairs(def.stock or {}) do
    local limit = tonumber(entry.limit)
    local used_count = used_of(shop_id, entry.item)
    local remaining = nil
    if limit then
      remaining = math.max(0, limit - used_count)
    end
    out[#out + 1] = {
      item = entry.item,
      price = deal_price(def, entry),
      limit = limit,
      used = used_count,
      remaining = remaining
    }
  end
  return out
end

-- 本店当前购买折扣（百分数，只读）：店主阵营声望档位决定，零声望为 0。
function M.price_off(shop_id)
  return keeper_price_off(M.get(shop_id))
end

-- 本店收购的商品（当前背包内持有的部分）。
function M.buys(shop_id)
  local def = M.get(shop_id)
  local out = {}
  if not def then
    return out
  end
  for _, item_id in ipairs(def.buys or {}) do
    local price = sell_price(item_id)
    out[#out + 1] = { item = item_id, price = price, have = inv.count(state.inventory, item_id) }
  end
  return out
end

-- ---------- 交易（原子） ----------

-- 购买：营业 → 商品存在 → 限购 → 余额 → 入包（失败回滚） → 扣款 → 计限购。
-- 入包用 inv.add 的 leftover 判定容量；任何一步失败都不改金币与背包。
function M.buy(shop_id, item_id, count)
  local def = M.get(shop_id)
  if not def then
    return false, "unknown_shop"
  end
  local units = math.floor(tonumber(count) or 0)
  if units <= 0 then
    return false, "bad_count"
  end
  local open, reason = M.is_open(shop_id)
  if not open then
    return false, reason
  end
  local entry = stock_entry(def, item_id)
  if not entry then
    return false, "not_sold"
  end
  local price = deal_price(def, entry)
  if not positive(price) then
    return false, "not_sold"
  end
  local limit = tonumber(entry.limit)
  if limit and used_of(shop_id, item_id) + units > limit then
    return false, "limit"
  end
  local cost = price * units
  if (tonumber(state.money) or 0) < cost then
    return false, "poor"
  end
  local leftover = tonumber(inv.add(state.inventory, item_id, units)) or 0
  if leftover > 0 then
    local added = units - leftover
    if added > 0 then
      inv.remove(state.inventory, item_id, added)
    end
    return false, "full"
  end
  state.money = (tonumber(state.money) or 0) - cost
  mark_used(shop_id, item_id, units)
  state.last_action = "buy_" .. item_id
  audio.ui("ui_confirm")
  return true, "ok"
end

-- 出售：营业 → 本店收购 → 持有足够 → 移除（失败回滚） → 进账。
function M.sell(shop_id, item_id, count)
  local def = M.get(shop_id)
  if not def then
    return false, "unknown_shop"
  end
  local units = math.floor(tonumber(count) or 0)
  if units <= 0 then
    return false, "bad_count"
  end
  local open, reason = M.is_open(shop_id)
  if not open then
    return false, reason
  end
  if not buys_item(def, item_id) then
    return false, "not_bought"
  end
  local price = sell_price(item_id)
  if not positive(price) then
    return false, "not_bought"
  end
  if inv.count(state.inventory, item_id) < units then
    return false, "not_enough"
  end
  local removed = tonumber(inv.remove(state.inventory, item_id, units)) or 0
  if removed ~= units then
    if removed > 0 then
      inv.add(state.inventory, item_id, removed)
    end
    return false, "not_enough"
  end
  state.money = (tonumber(state.money) or 0) + price * units
  state.last_action = "sell_" .. item_id
  -- 熟练度（docs/33 §7.1，0.9）：卖出成功计江湖线 XP（交易往来）。
  pcall(function()
    require("systems.mastery").add_xp("jianghu", 10)
  end)
  audio.ui("ui_confirm")
  return true, "ok"
end

-- ---------- 每日刷新 ----------

-- 每日库存刷新：限购计数清零（由 main.on_day_start 调用）。
function M.on_day_start()
  used = {}
end

function M.mark_used(shop_id, item_id, count)
  return mark_used(shop_id, item_id, count)
end

function M.used(shop_id, item_id)
  return used_of(shop_id, item_id)
end

-- ---------- 存档 ----------

local function snapshot_used()
  local out = {}
  for shop_id, counts in pairs(used) do
    if type(shop_id) == "string" then
      local one = {}
      for item_id, n in pairs(counts) do
        local value = tonumber(n)
        if type(item_id) == "string" and value and value > 0 then
          one[item_id] = value
        end
      end
      out[shop_id] = one
    end
  end
  return out
end

function M.capture()
  return { used = snapshot_used() }
end

function M.restore(data)
  used = {}
  if type(data) ~= "table" or type(data.used) ~= "table" then
    return
  end
  for shop_id, counts in pairs(data.used) do
    if type(shop_id) == "string" and type(counts) == "table" then
      local one = {}
      for item_id, n in pairs(counts) do
        local value = tonumber(n)
        if type(item_id) == "string" and value and value > 0 then
          one[item_id] = value
        end
      end
      used[shop_id] = one
    end
  end
end

-- ---------- 面板 UI ----------

-- 预设实例生命周期：`preset_alive` 用 pcall 探活根——世界重建会清掉实例登记表，
-- 旧 handle 直接调用是报错不是返回 nil（title_menu 同款 M63c 教训）。打开/关闭 =
-- 根节点显隐（`set_visible` 子树语义，藏父带子），不撤重铺：频繁开合走
-- spawn/despawn 会触发槽位 LIFO 复用，打乱绘制序（title_menu.prefab 注释同款警告）。
local function preset_alive()
  if not shop_handle then
    return false
  end
  local ok, alive = pcall(function()
    return prefab.root(shop_handle) ~= nil
  end)
  return ok and alive
end

local function preset_spawn()
  if not preset_alive() then
    shop_handle = prefab.spawn(PRESET_PATH)
  end
  return shop_handle
end

local function preset_show()
  if preset_alive() then
    prefab.set_visible(shop_handle, "ShopRoot", true)
  end
end

local function preset_hide()
  if preset_alive() then
    prefab.set_visible(shop_handle, "ShopRoot", false)
  end
end

-- 隐藏面板全部元素（场景切换会重建世界，重复调用安全）。
-- 必须定义在 M.reset 之前：Lua 词法作用域只认「已声明」的 local。
local function clear_ui()
  -- M65/B2：面板本体在预设实例里，关闭 = 藏根（子树语义带子）。
  preset_hide()
  -- 兼容热重载残留的旧动态层（预设化前 render 画过这些 id）。
  ui.clear("shp:box")
  ui.clear("shp:skin_bg")
  ui.clear("shp:rule")
  ui.clear("shp:sel")
  for _, id in ipairs({ "shp:divider", "shp:seal", "shp:close",
    "shp:tab_buy", "shp:tab_sell", "shp:step_minus", "shp:step_plus",
    "shp:scroll_up", "shp:scroll_down" }) do
    ui.clear(id)
  end
  for _, id in ipairs({ "shp:title", "shp:money", "shp:tab1", "shp:tab2", "shp:qty", "shp:msg", "shp:hint" }) do
    ui_text.hide("shp", id)
  end
  for i = 1, ROW_LIMIT do
    ui_text.hide("shp", "shp:row" .. i)
  end
  ui_text.reset("shp")
end

-- 新档 / 切图复位：清空限购计数并关闭面板（模式由调用方复位）。
function M.reset()
  used = {}
  ui_state = nil
  clear_ui()
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
end

-- 当前标签页的行数据（购买＝在售商品；出售＝背包内本店收购的物品）。
function M.rows()
  if not ui_state then
    return {}
  end
  local shop_id = ui_state.shop
  if ui_state.tab == "buy" then
    return M.stock(shop_id)
  end
  local out = {}
  for _, entry in ipairs(M.buys(shop_id)) do
    if entry.have and entry.have > 0 then
      out[#out + 1] = entry
    end
  end
  return out
end

function M.ui_active()
  return ui_state ~= nil
end

function M.tab()
  return ui_state and ui_state.tab or nil
end

function M.quantity()
  return ui_state and ui_state.quantity or 0
end

function M.selected()
  return ui_state and ui_state.index or 0
end

function M.row_limit()
  return ROW_LIMIT
end

local function clamp_quantity()
  local rows = M.rows()
  local row = rows[ui_state.index]
  local max = MAX_QUANTITY
  if ui_state.tab == "buy" and row and row.remaining and row.remaining < max then
    max = row.remaining
  elseif ui_state.tab == "sell" and row and (row.have or 0) < max then
    max = row.have or 0
  end
  if max < 1 then
    max = 1
  end
  local qty = tonumber(ui_state.quantity) or 1
  if qty > max then
    qty = max
  end
  if qty < 1 then
    qty = 1
  end
  ui_state.quantity = qty
end

function M.select(index)
  if not ui_state then
    return false
  end
  local i = math.floor(tonumber(index) or 0)
  if i < 1 or i > #M.rows() then
    return false
  end
  ui_state.index = i
  ui_state.quantity = 1
  clamp_quantity()
  sync_focus()
  return true
end

-- 焦点环与当前序号同步（M20-02；行数随标签页/背包变化）。
function sync_focus()
  if not ui_state then
    return
  end
  nav.open(FOCUS, #M.rows(), 1)
  nav.select(FOCUS, ui_state.index)
end

-- 焦点环方向移动（`step` 行，正数向下）；引擎 API 不可用返回 nil。
function ring_step(step)
  if not ui_state then
    return nil
  end
  local direction = step > 0 and "down" or "up"
  local index = ui_state.index
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
  return index
end

-- 相对移动选择：焦点环可用时按方向逐行移动（到边界环绕），否则退回钳制语义。
function M.move(delta)
  if not ui_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  local rows = M.rows()
  if step == 0 or #rows == 0 then
    return false
  end
  sync_focus()
  local target = ring_step(step)
  if target == nil then
    target = math.max(1, math.min(#rows, ui_state.index + step))
  end
  if target == ui_state.index then
    return false
  end
  ui_state.index = target
  ui_state.quantity = 1
  clamp_quantity()
  return true
end

-- 方向键移动（M20-02）：直接交给引擎焦点环（到边界环绕）。
function M.move_dir(direction)
  if not ui_state or #M.rows() == 0 then
    return false
  end
  sync_focus()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  ui_state.index = index
  ui_state.quantity = 1
  clamp_quantity()
  return true
end

function M.adjust(delta)
  if not ui_state then
    return false
  end
  local step = math.floor(tonumber(delta) or 0)
  if step == 0 then
    return false
  end
  ui_state.quantity = (ui_state.quantity or 1) + step
  clamp_quantity()
  return true
end

function M.toggle_tab()
  if not ui_state then
    return false
  end
  ui_state.tab = ui_state.tab == "buy" and "sell" or "buy"
  ui_state.index = 1
  ui_state.quantity = 1
  ui_state.msg = nil
  clamp_quantity()
  sync_focus()
  return true
end

local function feedback(ok, reason, verb)
  if ok then
    return verb .. "成功"
  end
  return REASON_TEXT[reason] or ("失败：" .. tostring(reason))
end

-- 确认当前选中行的交易（数量按面板数量）。
function M.confirm()
  if not ui_state then
    return false, "unknown_shop"
  end
  local row = M.rows()[ui_state.index]
  if not row then
    ui_state.msg = "无可交易项"
    M.render()
    return false, "not_sold"
  end
  local qty = ui_state.quantity or 1
  local ok, reason
  if ui_state.tab == "buy" then
    ok, reason = M.buy(ui_state.shop, row.item, qty)
    ui_state.msg = feedback(ok, reason, "购买")
  else
    ok, reason = M.sell(ui_state.shop, row.item, qty)
    ui_state.msg = feedback(ok, reason, "出售")
  end
  -- 失败反馈音（成功音在 buy/sell 内按结果播放）。
  if not ok then
    audio.ui("ui_cancel")
  end
  clamp_quantity()
  M.render()
  return ok, reason
end

-- M20-09：底部提示按当前设备二选一，按键名从**当前绑定**推导（改键后自动同步）。
-- 切换买卖页在世界外是 `place`(F)、在手柄上是上下文复用的 `attack`(X)，故两者都列。
-- 数字直选 1..ROW_LIMIT 的按键名（键鼠 "1/2/…"、手柄则是绑定到 slot_* 的按钮，
-- 默认没有 → nil 时该项自动跳过）。
local function digits_label()
  return hints.binding_labels({ "slot_1", "slot_2", "slot_3", "slot_4", "slot_5", "slot_6", "slot_7", "slot_8", "slot_9" }, nil, "/")
end

local function hint_line()
  local confirm_label = hints.binding_labels({ "confirm" })
  local cancel_label = hints.binding_labels({ "cancel" })
  -- 切换买卖页在世界外是 `place`(F)、在手柄上是上下文复用的 `attack`(X)，故两者都列。
  local switch_label = hints.binding_labels({ "place", "attack" })
  return hints.panel_line({
    { keyboard = "W/S", alt_keyboard = "方向键", gamepad = "左摇杆/DPad", label = "选择" },
    { keyboard = "1-9", gamepad = digits_label(), label = "定位" },
    { keyboard = switch_label, gamepad = switch_label, label = "切换买卖" },
    { keyboard = "+/-", gamepad = "+/-", label = "数量" },
    { keyboard = confirm_label, gamepad = confirm_label, label = "确认" },
    { keyboard = cancel_label, gamepad = cancel_label, label = "关闭" },
  })
end

-- 绘制商店面板：预设实例每世界 spawn 一次（幂等），运行期只改文字/矩形/显隐。
-- 静态几何（面板矩形、页签/步进/滚动箭头/关闭/印章、文字字号）全部在
-- `assets/prefabs/shop.prefab`；本函数每帧写的内容与迁移前动态层一致
-- （对账 = `jhyt_m65_shop_prefab` 黄金表 A/B 两态，逐值复现，bug-for-bug）。
function M.render()
  if not ui_state then
    return false
  end
  local def = M.get(ui_state.shop)
  if not def then
    return false
  end
  local x, y, w, h = PANEL.x, PANEL.y, PANEL.w, PANEL.h
  local BOX = layout.BOX
  local handle = preset_spawn()
  if not handle then
    return false
  end
  -- 页签二选一（叠图互斥：预设里 on/off 两个实体同矩形，脚本控制谁可见）。
  local buy_tab = ui_state.tab == "buy"
  prefab.set_visible(handle, "ShopTabBuyOn", buy_tab)
  prefab.set_visible(handle, "ShopTabBuyOff", not buy_tab)
  prefab.set_visible(handle, "ShopTabSellOn", not buy_tab)
  prefab.set_visible(handle, "ShopTabSellOff", buy_tab)
  -- 滚动箭头：行数超出一页才出现（与迁移前 `if #rows > ROW_LIMIT` 同口径）。
  local rows = M.rows()
  local over = #rows > ROW_LIMIT
  prefab.set_visible(handle, "ShopScrollUp", over)
  prefab.set_visible(handle, "ShopScrollDown", over)
  -- 选中高亮：绝对行号矩形。既有画面事实——行号超出 9 槽时 y 落在画面外
  -- （B 态 sel y=1534）、A 态高亮被 skin_bg 整块盖住，均为迁移前实测行为，
  -- 照抄进预设，修不修由用户裁定（账记 docs/69）。
  -- 注意 `prefab.set_rect` 签名是 (left, top, right, bottom) 四边，不是 (x,y,w,h)。
  local sel_x = x + BOX.list_dx
  local sel_y = y + 100 + (ui_state.index - 1) * ROW_H
  prefab.set_rect(handle, "ShopSel", sel_x, sel_y, sel_x + (w - BOX.list_dx * 2), sel_y + ROW_H)
  -- 文字内容（几何/字号在预设里）。
  prefab.set_text(handle, "ShopTitle", tostring(def.name))
  prefab.set_text(handle, "ShopMoney", "金币 " .. tostring(state.money) .. " 文")
  prefab.set_text(handle, "ShopTabLabel1", buy_tab and "[购买]" or " 购买 ")
  prefab.set_text(handle, "ShopTabLabel2", buy_tab and " 出售 " or "[出售]")
  prefab.set_text(handle, "ShopQty", "数量 × " .. tostring(ui_state.quantity))
  sync_focus()
  local first = nav.window(SCROLL, #rows, ROW_LIMIT, ROW_H, ui_state.index)
  for slot = 1, ROW_LIMIT do
    local id = "ShopRow" .. slot
    local index = first + slot - 1
    local row = rows[index]
    if row then
      local idef = item_def(row.item) or {}
      local label = tostring(index) .. ". " .. tostring(idef.name or row.item) .. "  " .. tostring(row.price) .. " 文"
      if ui_state.tab == "buy" then
        if row.limit then
          label = label .. "  限购剩余 " .. tostring(row.remaining)
        else
          label = label .. "  不限购"
        end
      else
        label = label .. "  持有 " .. tostring(row.have)
      end
      label = (index == ui_state.index) and ("> " .. label) or ("  " .. label)
      prefab.set_text(handle, id, label)
      prefab.set_visible(handle, id, true)
    else
      -- 空槽位藏行（无矩形 → 探针读不到，等价迁移前 `ui_text.hide`）。
      prefab.set_visible(handle, id, false)
    end
  end

  prefab.set_text(handle, "ShopMsg", tostring(ui_state.msg or ""))
  prefab.set_text(handle, "ShopHint", hint_line())
  return true
end

function M.open_ui(shop_id)
  local def = M.get(shop_id)
  if not def then
    return false
  end
  ui_state = { shop = shop_id, tab = "buy", index = 1, quantity = 1, msg = nil }
  state.set_mode("Shop")
  clamp_quantity()
  audio.ui("ui_click")
  -- 关屏只藏根（实例还在），重开先恢复根可见再整屏重写内容。
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
  nav.close(FOCUS)
  nav.scroll_close(SCROLL)
  if state.is_mode("Shop") then
    state.set_mode("Playing")
  end
  audio.ui("ui_cancel")
  return true
end

-- ---------- 交互目标 ----------

local function player_world()
  local pos = player.position()
  if type(pos) ~= "table" then
    return nil
  end
  return tonumber(pos.x), tonumber(pos.y)
end

local registered = false
function M.ensure_target()
  if registered then
    return
  end
  registered = true
  interaction.register({
    id = "shop_keeper",
    priority = 20,
    query = function()
      local map_id = map.current() or scene.reload()
      if not map_id then
        return nil
      end
      local px, py = player_world()
      if not px or not py then
        return nil
      end
      local best, best_distance = nil, nil
      for _, shop_id in ipairs(M.ids()) do
        local def = M.get(shop_id)
        local keeper = def and def.keeper
        if keeper and npcs.get(keeper) then
          local info = npc.query(keeper)
          if info and info.map == map_id and info.live and info.cell_x and info.cell_y then
            local nx = (info.cell_x + 0.5) * TILE
            local ny = (info.cell_y + 0.5) * TILE
            local dx, dy = nx - px, ny - py
            local distance = math.sqrt(dx * dx + dy * dy)
            if distance <= INTERACT_RANGE and (best_distance == nil or distance < best_distance) then
              best, best_distance = { id = shop_id, shop = shop_id, npc = info }, distance
            end
          end
        end
      end
      if not best then
        return nil
      end
      best.distance = best_distance
      best.facing = 1
      return best
    end,
    can_interact = function(candidate)
      return M.is_open(candidate.shop)
    end,
    prompt = function(candidate, reason)
      if reason == "closed" then
        return "未营业"
      end
      local def = M.get(candidate.shop)
      return "E: " .. tostring(def and def.name or "商店")
    end,
    interact = function(candidate)
      if not M.open_ui(candidate.shop) then
        return "no_shop"
      end
      return "shop"
    end
  })
end

-- ---------- 加载期校验 ----------

function M.validate_all()
  local issues = {}
  local reg = defs()
  if type(reg) ~= "table" or next(reg) == nil then
    issues[#issues + 1] = "data.shops 为空或加载失败"
    return false, issues
  end
  if reg.version ~= 1 then
    issues[#issues + 1] = "data/shops.lua:version: unsupported version " .. tostring(reg.version)
  end
  for shop_id, def in pairs(reg) do
    if shop_id ~= "version" then
      local where = "商店 " .. tostring(shop_id)
      if type(def) ~= "table" then
        issues[#issues + 1] = where .. " 必须是表"
      else
        if def.id ~= shop_id then
          issues[#issues + 1] = where .. " 的 id 字段不匹配：" .. tostring(def.id)
        end
        if type(def.name) ~= "string" or def.name == "" then
          issues[#issues + 1] = where .. " 缺少 name"
        end
        if type(def.keeper) ~= "string" or not npcs.get(def.keeper) then
          issues[#issues + 1] = where .. " 的 keeper 引用未知 NPC：" .. tostring(def.keeper)
        end
        if def.place ~= nil and (type(def.place) ~= "string" or not places.get(def.place)) then
          issues[#issues + 1] = where .. " 的 place 引用未知地点：" .. tostring(def.place)
        end
        if def.hours ~= nil then
          if type(def.hours) ~= "table" then
            issues[#issues + 1] = where .. " 的 hours 必须是表"
          else
            local open = tonumber(def.hours.open)
            local close = tonumber(def.hours.close)
            if not open or not close then
              issues[#issues + 1] = where .. " 的 hours.open/close 必须是数值"
            elseif open < 0 or close > MINUTES_PER_DAY or close <= open then
              issues[#issues + 1] = where .. " 的 hours 窗口非法：" .. tostring(open) .. ".." .. tostring(close)
            end
          end
        end

        local sold = {}
        if type(def.stock) ~= "table" or #def.stock == 0 then
          issues[#issues + 1] = where .. " 的 stock 必须是非空表"
        else
          for i, entry in ipairs(def.stock) do
            local ew = where .. " 商品 " .. tostring(i)
            if type(entry) ~= "table" then
              issues[#issues + 1] = ew .. " 必须是表"
            else
              if not item_def(entry.item) then
                issues[#issues + 1] = ew .. " 引用未知物品：" .. tostring(entry.item)
              elseif sold[entry.item] then
                issues[#issues + 1] = ew .. " 物品重复上架：" .. tostring(entry.item)
              else
                sold[entry.item] = true
              end
              if not positive(stock_price(entry)) then
                issues[#issues + 1] = ew .. " 的 price 必须为正数（或物品 buy > 0）：" .. tostring(entry.item)
              end
              if entry.limit ~= nil and not positive(entry.limit) then
                issues[#issues + 1] = ew .. " 的 limit 必须为正数"
              end
            end
          end
        end

        if type(def.buys) ~= "table" or #def.buys == 0 then
          issues[#issues + 1] = where .. " 的 buys 必须是非空表"
        else
          for i, item_id in ipairs(def.buys) do
            local bw = where .. " 收购项 " .. tostring(i)
            if not item_def(item_id) then
              issues[#issues + 1] = bw .. " 引用未知物品：" .. tostring(item_id)
            elseif not positive((item_def(item_id) or {}).sell) then
              issues[#issues + 1] = bw .. " 的物品缺少正数 sell 价：" .. tostring(item_id)
            end
            if sold[item_id] then
              issues[#issues + 1] = bw .. " 与在售商品重复（可套利）：" .. tostring(item_id)
            end
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
