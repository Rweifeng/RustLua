local M = {}

local items = nil
local items_count = 0

function M.on_create()
    items = data.load("data/demo_items.json")
    for id, definition in pairs(items) do
        item.define(id, definition)
        items_count = items_count + 1
    end
    inv.create("player", 5)
    inv.add("player", "potion", 2)
    inv.add("player", "sword", 1)
    inv.create("chest", 3)
    container.open(entity.find("Chest"), 3)
    container.move("player", 1, "chest", 1)
    ui.text_at("data_status", 20, 620, "JSON data, schema, inventory and containers", 2)
    log.info("data_inventory: loaded and moved items")
end

function M.update()
    if input.just_pressed("space") then
        inv.add("player", "potion", 1)
    end
    local potion = item.def("potion")
    return string.format(
        "data_ok items=%d potions=%d chest_name=%s",
        items_count,
        inv.count("player", "potion"),
        potion.name
    )
end

return M
