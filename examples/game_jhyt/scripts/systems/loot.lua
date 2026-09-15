-- systems/loot.lua：敌人掉落与"当日待入库"（M28 / 计划 G3）。
--
-- 掉落的经济口径（docs/33 §6.3「死亡与恢复」）：
--   * 铜钱：击杀当场入账（`state.money`），死亡时按 5% 损失（见 `systems/death.lua`）。
--   * 物品：先进**当日待入库袋**（`state.vars` 的 `loot_p_<item>` 计数），
--     睡觉跨天时 `bank()` 一次性入库；死亡会损失袋中物品的 50%。
--     "未入库"就是字面意思：今天打到的猎物还没搬回山庄，路上翻车就掉一半。
--
-- 为什么用 `state.vars` 标量：它随 `core/state.lua` 的 provider 自动进出存档，
-- 不需要新增 provider，也不会出现"读档后待入库袋丢失但背包物品还在"的半状态。
--
-- 随机数：内部 LCG（`loot_rng` 变量持久化）。**同一存档、同一击杀顺序 → 同一掉落**，
-- headless 才能断言具体数量；`M.set_seed()` 供测试固定序列。
local state = require("core.state")
local drops = require("data.drops")

local M = {}

local RNG_VAR = "loot_rng"
local DEFAULT_SEED = 20260914
local PENDING_PREFIX = "loot_p_"
-- LCG 模数（numerical recipes 的 32 位线性同余），只用来产生 [0,1) 数值。
local RNG_MOD = 2147483648
local RNG_MUL = 1103515245
local RNG_ADD = 12345

local function pending_var(item_id)
  return PENDING_PREFIX .. tostring(item_id)
end

--- 复位（新档）：种子回默认值，待入库袋清空。
function M.reset()
  for _, entry in ipairs(M.pending_list()) do
    state.set_var(pending_var(entry.item), 0, "loot", "reset")
  end
  state.set_var(RNG_VAR, DEFAULT_SEED, "loot", "reset")
  return true
end

--- 固定随机序列（headless / 调试）：只影响之后的掉落。
function M.set_seed(seed)
  state.set_var(RNG_VAR, math.floor(tonumber(seed) or DEFAULT_SEED), "loot", "set_seed")
  return M.seed()
end

function M.seed()
  return tonumber(state.get_var(RNG_VAR)) or DEFAULT_SEED
end

-- [0, 1) 的确定性随机数：状态存在存档里，读档后序列继续。
local function next_unit()
  local seed = tonumber(state.get_var(RNG_VAR)) or DEFAULT_SEED
  seed = (seed * RNG_MUL + RNG_ADD) % RNG_MOD
  state.set_var(RNG_VAR, seed, "loot", "roll")
  return seed / RNG_MOD
end

-- 闭区间整数 [min, max]：min == max 时不消耗随机数（必掉项不扰动后续序列）。
local function roll_int(min, max)
  local lo = math.floor(tonumber(min) or 0)
  local hi = math.floor(tonumber(max) or 0)
  if hi <= lo then
    return lo
  end
  local span = hi - lo + 1
  local value = lo + math.floor(next_unit() * span)
  if value > hi then
    value = hi
  end
  return value
end

--- 只读掷骰（不改状态）：`{ money = n, items = { {item, count}, ... } }`。
--- 注意它会推进随机序列——要"预览不掉落"请用 `M.table_of(enemy_id)` 读表。
function M.roll(enemy_id)
  local entry = drops.get(enemy_id)
  if not entry then
    return nil
  end
  local out = { enemy = enemy_id, money = 0, items = {} }
  if type(entry.money) == "table" then
    out.money = roll_int(entry.money.min, entry.money.max)
  end
  for _, drop in ipairs(entry.items or {}) do
    local chance = tonumber(drop.chance) or 1
    if next_unit() <= chance then
      local count = roll_int(drop.min, drop.max)
      if count > 0 then
        out.items[#out.items + 1] = { item = drop.id, count = count }
      end
    end
  end
  return out
end

--- 掉落的静态口径（不掷骰）：给任务日志/文档对齐与 headless 结构断言用。
function M.table_of(enemy_id)
  local entry = drops.get(enemy_id)
  if not entry then
    return nil
  end
  return {
    enemy = entry.enemy,
    money = entry.money,
    items = entry.items
  }
end

local function add_pending(item_id, count)
  local amount = math.max(0, math.floor(tonumber(count) or 0))
  if amount <= 0 then
    return 0
  end
  local current = tonumber(state.get_var(pending_var(item_id))) or 0
  state.set_var(pending_var(item_id), current + amount, "loot", "pending")
  return amount
end

--- 击杀结算：铜钱入账 + 物品进待入库袋 + HUD 反馈（`loot_<item>` / `loot_money`）。
--- 返回 `{ money = n, items = {...} }`（无掉落记录时返回 nil，不写任何状态）。
function M.on_kill(enemy_id)
  local rolled = M.roll(enemy_id)
  if not rolled then
    return nil
  end
  if rolled.money > 0 then
    state.money = (tonumber(state.money) or 0) + rolled.money
  end
  local names = {}
  for _, drop in ipairs(rolled.items) do
    add_pending(drop.item, drop.count)
    names[#names + 1] = tostring(drop.item) .. " x" .. tostring(drop.count)
  end
  -- HUD 一行只放得下一条：有物品时显示第一件物品（前缀 loot_ 按物品名解析），
  -- 只掉钱时显示 loot_money；完整明细写日志。
  if rolled.items[1] then
    state.last_action = "loot_" .. tostring(rolled.items[1].item)
  elseif rolled.money > 0 then
    state.last_action = "loot_money"
  end
  log.info(string.format(
    "[loot] kill enemy=%s money=%d items=%s",
    tostring(enemy_id), rolled.money,
    (#names > 0) and table.concat(names, ",") or "-"))
  return rolled
end

--- 待入库数量。
function M.pending(item_id)
  return tonumber(state.get_var(pending_var(item_id))) or 0
end

--- 待入库清单（按物品 id 稳定排序）：`{ { item, count }, ... }`。
function M.pending_list()
  local list = {}
  for name, value in pairs(state.vars or {}) do
    if type(name) == "string" and name:sub(1, #PENDING_PREFIX) == PENDING_PREFIX then
      local count = tonumber(value) or 0
      local item_id = name:sub(#PENDING_PREFIX + 1)
      if count > 0 and item_id ~= "" then
        list[#list + 1] = { item = item_id, count = count }
      end
    end
  end
  table.sort(list, function(a, b)
    return a.item < b.item
  end)
  return list
end

function M.pending_count()
  local total = 0
  for _, entry in ipairs(M.pending_list()) do
    total = total + entry.count
  end
  return total
end

--- 待入库的说明文案（HUD / 日志）：空袋返回 ""。
function M.pending_text()
  local parts = {}
  for _, entry in ipairs(M.pending_list()) do
    parts[#parts + 1] = tostring(entry.item) .. " x" .. tostring(entry.count)
  end
  return table.concat(parts, "、")
end

--- 入库（跨天调用）：待入库物品搬进背包，袋清空。返回入库清单。
function M.bank()
  local banked = {}
  for _, entry in ipairs(M.pending_list()) do
    local ok = pcall(inv.add, state.inventory, entry.item, entry.count)
    if ok then
      banked[#banked + 1] = { item = entry.item, count = entry.count }
      state.set_var(pending_var(entry.item), 0, "loot", "bank")
    else
      log.warn("[loot] 入库失败：" .. tostring(entry.item) .. " x" .. tostring(entry.count))
    end
  end
  if #banked > 0 then
    log.info("[loot] bank " .. M.pending_text_of(banked))
  end
  return banked
end

--- 把清单拼成文本（`{ {item, count}, ... }` → "wolf_pelt x1、boar_meat x2"）。
function M.pending_text_of(list)
  local parts = {}
  for _, entry in ipairs(list or {}) do
    parts[#parts + 1] = tostring(entry.item) .. " x" .. tostring(entry.count)
  end
  return table.concat(parts, "、")
end

--- 死亡惩罚：损失待入库物品的 50%（向下取整；1 件时损失 0 = 手里那件总能留下）。
--- 返回 `{ { item, count }, ... }` 实际损失的清单。
function M.lose_on_death()
  local lost = {}
  for _, entry in ipairs(M.pending_list()) do
    local lose = math.floor(entry.count * 0.5)
    if lose > 0 then
      state.set_var(pending_var(entry.item), entry.count - lose, "loot", "death_loss")
      lost[#lost + 1] = { item = entry.item, count = lose }
    end
  end
  if #lost > 0 then
    log.warn("[loot] 战败损失未入库掉落：" .. M.pending_text_of(lost))
  end
  return lost
end

return M
