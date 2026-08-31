local state = require("state")

local M = {}

function M.setup()
  local items = require("data.items")
  for id, definition in pairs(items) do
    item.define(id, definition)
  end
  inv.create(state.inventory, 20)
  inv.add(state.inventory, "seed_lingzhi", 5)
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

return M
