local state = require("state")
local items = require("data.items")

local M = {}

local MAX_ENERGY = 100
local MAX_INNER_POWER = 100

function M.setup()
  for id, definition in pairs(items) do
    item.define(id, definition)
  end
  local tools = require("data.tools")
  for id, definition in pairs(tools) do
    item.define(id, definition)
  end
  inv.create(state.inventory, 20)
  inv.add(state.inventory, "seed_lingzhi", 5)
  inv.add(state.inventory, "seed_rice", 5)
  inv.add(state.inventory, "rice", 2)
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
