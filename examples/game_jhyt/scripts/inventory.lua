local state = require("state")

local M = {}

function M.setup()
  local items = require("data.items")
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

function M.use(item_id)
  if inv.count(state.inventory, item_id) <= 0 then
    return false
  end
  inv.remove(state.inventory, item_id, 1)
  state.last_action = "use_" .. item_id
  return true
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
