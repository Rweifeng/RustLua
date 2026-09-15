local state = require("core.state")
local items = require("data.items")
local stats = require("player.stats")

local M = {}

local MAX_ENERGY = 100
local MAX_INNER_POWER = 100

-- 上限的只读访问口（M28）：跨天重置（`main.on_day_start`）与战败回庄
-- （`systems/death.lua`）都要把体力/内力拉回上限，不能再各自写死 100。
function M.max_energy()
  return MAX_ENERGY
end

function M.max_inner_power()
  return MAX_INNER_POWER
end

-- 物品定义注册（`data/items.lua` + `data/tools.lua` → 引擎注册表）。
-- **在模块加载时就登记**，而不是等到开新档：引擎 `item.def` 是唯一的显示名查询口
-- （HUD 的物品名、快捷栏、装备槽都走它），登记必须早于任何显示行为。
-- `item.define` 幂等（同 id 覆盖），因此热重载后重复登记也无副作用。
local function register_items()
  for id, definition in pairs(items) do
    item.define(id, definition)
  end
  for id, definition in pairs(require("data.tools")) do
    item.define(id, definition)
  end
end

register_items()

-- 开新档：背包与初始物资（物品定义已在模块加载时登记，这里不再重复）。
function M.setup()
  inv.create(state.inventory, 20)
  -- 灵芝菌种（M19-01 调整）：5→8。**每块田一茬收 1 灵芝**（`data/crops.lua`
  -- `yield = 1`，与 docs/33 §4.1 一致；M23 之前这句注释按"收 2"写、而数据里根本没有
  -- `yield` 字段，实际恒收 1）。MVP 剧本里灵芝要分四处用：
  -- 医馆委托 3（可选支线）+ 药庐订单 2 + 主线二段交药材 2 + 炼回复药 2 = 9，
  -- 山道当日采集（3）+ 次日刷新的两处采集点（`respawn_days = 1`）是主要来源，
  -- 8 粒菌种（4 块田 × 1 = 4 灵芝/茬）是多日路线的补足与教学，并留「误种一次
  -- 仍可完成」的空间（数值校核见 docs/46 §10）。
  inv.add(state.inventory, "seed_lingzhi", 8)
  -- 稻种（M19-01 调整）：3→4。稻谷要同时满足「市集订单 2 斗」与「主线二段的一斗口粮」
  -- 两处消耗，**每块田一茬收 2**（`yield = 2`），4 粒留出一次重播余量，
  -- 也避免一次误种把 MVP 剧本走死。
  inv.add(state.inventory, "seed_rice", 4)
  inv.add(state.inventory, "rice", 2)
  -- 水壶（M19-02 补齐 MVP 闭环）：farm.act 浇水要求 tool 槽装备 watering_can，
  -- 而此前没有任何获取途径（商店不售工具、无任务奖励）→ 作物永远无法成长，
  -- 「播种→浇水→睡觉→收获→交付→制作」剧本不可完成。新档直接发放 1 把，
  -- 由玩家在背包面板（I → 选中 → E 装备）切换，同时验证背包/装备交互。
  inv.add(state.inventory, "watering_can", 1)
  inv.equip(state.inventory, "tool", "hoe")
end

-- 使用效果只存在于 Lua 侧数据（data.items[id].use）：引擎 item.define 会忽略
-- 未知字段，故不能从 item.def 回读。无 use 字段即不可使用（种子/收获物/工具）。
local function use_effect(item_id)
  local def = items[item_id]
  if def and type(def.use) == "table" then
    return def.use
  end
  return nil
end

-- 使用物品（M17-04 硬风险③）：先确认确实能产生效果，再应用效果，最后消耗 1 个。
-- 无效果或用满上限时拒绝并保留物品，绝不出现「只删物品不生效」的半完成状态；
-- 效果结果写入 state，随 state.capture 存档快照一致恢复。
-- M18-04 扩展：use.buff = { stat=, amount=, days= } → stats.add_buff（临时
-- Buff 按游戏日过期）；属性模型当前只挂玩家（"player"）。Buff 非法时按
-- add_buff 的 reason 拒绝且不消耗（与「先应用后消耗」语义一致）。
function M.use(item_id)
  if type(item_id) ~= "string" or item_id == "" then
    return false, "unknown_item"
  end
  if inv.count(state.inventory, item_id) <= 0 then
    return false, "not_enough"
  end
  local effect = use_effect(item_id)
  if not effect then
    return false, "no_effect"
  end

  local applied = false
  local energy = tonumber(effect.energy)
  if energy and energy ~= 0 then
    local before = tonumber(state.energy) or 0
    local after = math.max(0, math.min(MAX_ENERGY, before + energy))
    if after ~= before then
      state.energy = after
      applied = true
    end
  end
  local inner_power = tonumber(effect.inner_power)
  if inner_power and inner_power ~= 0 then
    local before = tonumber(state.inner_power) or 0
    local after = math.max(0, math.min(MAX_INNER_POWER, before + inner_power))
    if after ~= before then
      state.inner_power = after
      applied = true
    end
  end
  if effect.buff ~= nil then
    if type(effect.buff) ~= "table" then
      return false, "bad_buff"
    end
    local ok, reason = stats.add_buff("player", effect.buff)
    if not ok then
      return false, reason or "bad_buff"
    end
    applied = true
  end
  if not applied then
    return false, "no_effect"
  end

  inv.remove(state.inventory, item_id, 1)
  state.last_action = "use_" .. item_id
  return true, "ok"
end

-- 存档快照（save_manager 调用）：槽位 + 命名装备槽整体镜像。
-- 注意：槽位必须稠密输出（空槽 = {item="",count=0}）。引擎的 Lua↔JSON blob
-- 序列化里，稀疏数字 key 表会退化为 object、回读后 key 变字符串，导致
-- inv.set 的 index 参数类型错；1..capacity 全填充则保持 JSON array。
function M.capture()
  local data = inv.get(state.inventory)
  local capacity = data.capacity or 20
  local slots = {}
  for index = 1, capacity do
    local stack = data.slots and data.slots[index]
    if stack then
      slots[index] = { item = stack.item, count = stack.count }
    else
      slots[index] = { item = "", count = 0 }
    end
  end
  return {
    capacity = capacity,
    slots = slots,
    equipment = inv.equip_list(state.inventory)
  }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  local capacity = tonumber(data.capacity) or 20
  if not inv.exists(state.inventory) then
    inv.create(state.inventory, capacity)
  end
  for index = 1, inv.capacity(state.inventory) do
    inv.set(state.inventory, index, nil, nil)
  end
  for index = 1, capacity do
    local slot = data.slots and data.slots[index]
    if slot and slot.item and slot.item ~= "" then
      inv.set(state.inventory, index, slot.item, tonumber(slot.count) or 1)
    end
  end
  for slot, item_id in pairs(data.equipment or {}) do
    inv.equip(state.inventory, slot, item_id)
  end
end

return M
