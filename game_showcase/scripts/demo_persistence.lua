local M = {}

local events = 0

function M.on_create()
    event.on("showcase", function(data)
        events = events + 1
        log.info("persistence event: " .. tostring(data and data.message))
    end)
    save.write("scene", "persistence")
    save.blob_write("player", { level = 2, inventory = { "potion", "sword" } })
    save.slot("showcase")
    save.commit()
    map.schema("town", { visits = "number" })
    map.state("town", { visits = 1 })
    ui.text_at("persistence_status", 20, 620, "Save blob, slot, map state, event bus", 2)
end

function M.update()
    if input.just_pressed("space") then
        event.emit("showcase", { message = "manual" })
    elseif input.just_pressed("l") then
        save.load()
    elseif input.just_pressed("c") then
        save.commit()
    end
    local player = save.blob_read("player")
    local town = map.state("town")
    return string.format(
        "persistence_ok level=%s visits=%s events=%d version=%s",
        tostring(player and player.level),
        tostring(town and town.visits),
        events,
        tostring(save.version())
    )
end

return M
