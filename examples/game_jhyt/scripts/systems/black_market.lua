-- systems/black_market.lua：《江湖有田》黑市抉择循环运行时
-- （docs/54 阶段3「黑市抉择循环」，2026-10-06 2.2）。
--
-- 阶段定位：本阶段黑市是**纯数据 + 系统**（headless 直接驱动）。黑市场景/摊位 NPC/
-- 交互面板 UI 属改造计划阶段4美术管线——本系统不注册 interaction 目标、不切场景、
-- 不占用 state.mode（VALID_MODES 在 core/state.lua，本阶段不改共享白名单），只暴露
-- is_open/buy/sell/resolve_event 给驱动方（headless 样例与将来的黑市交互）。
--
-- 运行期状态（capture/restore 随 save_manager 持久；reset 新档清空）：
--   favor   黑市声望分（0..100，档位由 data.favor_tiers 派生）；连续交易推高档位 →
--           解锁更好货品；被抓/报官则档位下降。
--   trades  累计黑市成交笔数（诊断/图鉴）。
--   log     有界成交流水（诊断用，上限 32）。
--   debug_rng 仅 headless 样例用：nil=正常 math.random；true=必被抓；false=绝不被抓。
--
-- 动作语义（与 systems/shop.lua / shipping.lua 同口径：**失败不写 last_action**，
-- 成功才写；买入写 "blackmarket_buy_<id>"，卖出写 "blackmarket_sell_<id>"）：
--   * buy(item, count)：营业 → 在售 → 档位已解锁 → 余额 → 入包（失败回滚）→ 扣款 →
--       风险掷骰。被抓＝货物没收（入包的物品移除）+ 阵营声望罚 + favor 跌落；
--       未被抓＝favor 上涨 + 阵营声望影响。返回 (true, "ok"|"caught", detail)。
--   * sell(item, count)：营业 → 黑市收购 → 持有足够 → 移除（失败回滚）→ 进账 →
--       风险掷骰。被抓＝货物追回 + 声望罚 + favor 跌落。
--   * resolve_event(id, choice)：抉择事件三选后果入账（钱/给物/favor/阵营声望）。
local state = require("core.state")
local data = require("data.black_market")
local relationship = require("player.relationship")

local inv = inv

local M = {}

local MINUTES_PER_DAY = 1440
local MAX_FAVOR = 100
local LOG_LIMIT = 32

-- 运行期状态
local favor = 0
local trades = 0
local log = {}
-- headless 风险掷骰接缝：nil = 正常 math.random；true = 必被抓；false = 绝不被抓。
local debug_rng = nil

-- ---------- 内部工具 ----------

local function risk_chance(grade)
  return tonumber((data.risk or {})[grade]) or 0
end

local function was_caught(grade)
  if debug_rng ~= nil then
    return debug_rng == true
  end
  return math.random() <= risk_chance(grade)
end

-- 阵营声望入账（负分＝罚）；未知阵营/零步静默跳过（校验门已挡住坏数据）。
local function apply_rep(rep)
  if type(rep) ~= "table" then
    return
  end
  for faction, delta in pairs(rep) do
    local step = tonumber(delta)
    if step and step ~= 0 then
      relationship.add_reputation(faction, step, "black_market", "trade")
    end
  end
end

local function push_log(entry)
  log[#log + 1] = entry
  while #log > LOG_LIMIT do
    table.remove(log, 1)
  end
end

local function stock_entry(item_id)
  for _, e in ipairs(data.stock or {}) do
    if e.item == item_id then
      return e
    end
  end
  return nil
end

local function buys_entry(item_id)
  for _, e in ipairs(data.buys or {}) do
    if e.item == item_id then
      return e
    end
  end
  return nil
end

-- ---------- 只读访问器（headless 断言/诊断用） ----------

function M.favor()
  return favor
end

-- 当前黑市声望档位条目（派生）；取最后一个 min <= favor 的档位。
function M.favor_tier()
  local tiers = data.favor_tiers or {}
  local tier = tiers[1]
  for _, entry in ipairs(tiers) do
    if favor >= (tonumber(entry.min) or 0) then
      tier = entry
    end
  end
  return tier
end

-- 当前在售（含档位锁定标记）：min_favor 未达的货品 locked=true，不可买。
function M.stock()
  local out = {}
  for _, e in ipairs(data.stock or {}) do
    local need = tonumber(e.min_favor) or 0
    out[#out + 1] = {
      item = e.item,
      price = e.price,
      risk = e.risk,
      locked = favor < need,
      need = need
    }
  end
  return out
end

function M.buys_view()
  local out = {}
  for _, e in ipairs(data.buys or {}) do
    out[#out + 1] = { item = e.item, price = e.price, risk = e.risk }
  end
  return out
end

-- 风险档概率（headless 断言风险档位逻辑用）。
function M.risk_chance(grade)
  return risk_chance(grade)
end

function M.log()
  local out = {}
  for i, entry in ipairs(log) do
    out[i] = entry
  end
  return out
end

-- ---------- 营业时间 ----------

-- 左闭右开分钟窗口判定（与 systems/shop.lua 同口径）。深夜窗口外一律不营业。
function M.is_open()
  local hours = data.hours
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

-- ---------- 交易（原子） ----------

-- 买入：营业 → 在售 → 档位解锁 → 余额 → 入包（失败回滚）→ 扣款 → 风险掷骰。
function M.buy(item_id, count)
  local open, reason = M.is_open()
  if not open then
    return false, reason
  end
  local units = math.floor(tonumber(count) or 0)
  if units <= 0 then
    return false, "bad_count"
  end
  local entry = stock_entry(item_id)
  if not entry then
    return false, "not_sold"
  end
  local price = tonumber(entry.price)
  if not price or price <= 0 then
    return false, "not_sold"
  end
  local need = tonumber(entry.min_favor) or 0
  if favor < need then
    return false, "locked"
  end
  local cost = price * units
  if (tonumber(state.money) or 0) < cost then
    return false, "poor"
  end
  -- 先试装入包：一件也装不下则不扣款、不掷骰（失败不写 last_action）。
  local leftover = tonumber(inv.add(state.inventory, item_id, units)) or 0
  if leftover >= 1 then
    local added = units - leftover
    if added > 0 then
      inv.remove(state.inventory, item_id, added)
    end
    return false, "full"
  end
  state.money = (tonumber(state.money) or 0) - cost

  -- 风险掷骰：被抓＝货物没收（移除刚入包的全部 units）+ 声望罚 + favor 跌落。
  if was_caught(entry.risk) then
    inv.remove(state.inventory, item_id, units)
    local pen = data.caught_penalty or {}
    apply_rep(pen.rep)
    favor = math.max(0, favor - (tonumber(pen.favor) or 5))
    trades = trades + 1
    state.last_action = "blackmarket_buy_" .. item_id
    push_log({ kind = "buy_caught", item = item_id, count = units, cost = cost })
    return true, "caught", { caught = true }
  end

  favor = math.min(MAX_FAVOR, favor + (tonumber(entry.favor_gain) or 1) * units)
  apply_rep(entry.rep)
  trades = trades + 1
  state.last_action = "blackmarket_buy_" .. item_id
  push_log({ kind = "buy", item = item_id, count = units, cost = cost })
  return true, "ok", { caught = false }
end

-- 卖出：营业 → 黑市收购 → 持有足够 → 移除（失败回滚）→ 进账 → 风险掷骰。
function M.sell(item_id, count)
  local open, reason = M.is_open()
  if not open then
    return false, reason
  end
  local units = math.floor(tonumber(count) or 0)
  if units <= 0 then
    return false, "bad_count"
  end
  local entry = buys_entry(item_id)
  if not entry then
    return false, "not_bought"
  end
  local price = tonumber(entry.price)
  if not price or price <= 0 then
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
  local income = price * units
  state.money = (tonumber(state.money) or 0) + income

  -- 被抓＝货物追回（退回背包）+ 声望罚 + favor 跌落。
  if was_caught(entry.risk) then
    inv.add(state.inventory, item_id, units)
    local pen = data.caught_penalty or {}
    apply_rep(pen.rep)
    favor = math.max(0, favor - (tonumber(pen.favor) or 5))
    trades = trades + 1
    state.last_action = "blackmarket_sell_" .. item_id
    push_log({ kind = "sell_caught", item = item_id, count = units, income = income })
    return true, "caught", { caught = true }
  end

  favor = math.min(MAX_FAVOR, favor + (tonumber(entry.favor_gain) or 1) * units)
  apply_rep(entry.rep)
  trades = trades + 1
  state.last_action = "blackmarket_sell_" .. item_id
  push_log({ kind = "sell", item = item_id, count = units, income = income })
  return true, "ok", { caught = false }
end

-- ---------- 抉择事件 ----------

function M.event(id)
  for _, e in ipairs(data.events or {}) do
    if e.id == id then
      return e
    end
  end
  return nil
end

-- 三选后果入账：接受得钱/得物、favor 与阵营声望按数据变动；拒绝空表＝无事发生；
-- 报官＝favor 大跌、village 声望上涨（正义回报）。
function M.resolve_event(id, choice)
  local ev = M.event(id)
  if not ev then
    return false, "no_event"
  end
  local out = (ev.choices or {})[choice]
  if type(out) ~= "table" then
    return false, "bad_choice"
  end
  local detail = {}
  local money = tonumber(out.money) or 0
  if money ~= 0 then
    state.money = (tonumber(state.money) or 0) + money
    detail.money = money
  end
  if out.give_item then
    inv.add(state.inventory, out.give_item, tonumber(out.give_count) or 1)
    detail.give_item = out.give_item
  end
  local fav = tonumber(out.favor) or 0
  if fav ~= 0 then
    favor = math.max(0, math.min(MAX_FAVOR, favor + fav))
    detail.favor = fav
  end
  apply_rep(out.rep)
  state.last_action = "blackmarket_event_" .. tostring(choice) .. "_" .. tostring(id)
  push_log({ kind = "event_" .. tostring(choice), id = id })
  return true, "ok", detail
end

-- ---------- 跨天挂点（main.on_day_start 调用） ----------

-- 黑市每日无持久窗口状态：favor/trades 随存档；本钩子仅作跨天扩展位
-- （将来「每日随机刷不刷黑市」的概率挂点落这里）。当前为空操作，保留挂点语义。
function M.on_day_start()
end

-- ---------- 存档 ----------

function M.capture()
  local log_data = {}
  for i, entry in ipairs(log) do
    log_data[i] = entry
  end
  return { favor = favor, trades = trades, log = log_data }
end

function M.restore(data_in)
  favor = 0
  trades = 0
  log = {}
  if type(data_in) ~= "table" then
    return
  end
  favor = math.max(0, math.min(MAX_FAVOR, tonumber(data_in.favor) or 0))
  trades = tonumber(data_in.trades) or 0
  if type(data_in.log) == "table" then
    for _, entry in ipairs(data_in.log) do
      if type(entry) == "table" then
        log[#log + 1] = entry
      end
    end
  end
end

-- 新档复位：清 favor/trades/log，并复位 headless 风险接缝（不留到下一局）。
function M.reset()
  favor = 0
  trades = 0
  log = {}
  debug_rng = nil
end

-- ---------- headless 调试接缝（样例用，不入档） ----------

-- 固定风险掷骰结果：nil 恢复正常随机；true 必被抓；false 绝不被抓。
function M.debug_set_rng(v)
  debug_rng = v
end

-- 直接设定黑市声望分（档位解锁断言用）；clamp 到 0..MAX。
function M.debug_set_favor(v)
  favor = math.max(0, math.min(MAX_FAVOR, tonumber(v) or 0))
end

-- ---------- 加载期校验（与 systems/shop.lua 同约定：返回 true, issues） ----------

function M.validate_all()
  local issues = {}
  -- 数据层引用完整性（物品/阵营/事件窗口）。
  local ok, data_valid, data_issues = pcall(data.validate_all)
  if not ok then
    issues[#issues + 1] = "data.black_market.validate_all crashed: " .. tostring(data_valid)
  elseif data_valid ~= true then
    for _, issue in ipairs(data_issues or {}) do
      issues[#issues + 1] = tostring(issue)
    end
  end
  -- 运行期不变量：favor 恒在 [0, MAX_FAVOR]。
  local f = tonumber(favor) or 0
  if f < 0 or f > MAX_FAVOR then
    issues[#issues + 1] = "black_market: favor 越界 " .. tostring(f)
  end
  return #issues == 0, issues
end

return M
