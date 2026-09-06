local M = {}

local clock_id = nil

function M.on_create()
    local player = entity.find("Farmer")
    tag.set(player, "kind", "farmer")
    item.define("seed_turnip", {
        name = "Turnip Seed",
        description = "A fast spring crop",
        stack = 99,
        buy = 12,
        sell = 5,
        texture = "assets/textures/coin.png",
        kind = "seed",
    })
    inv.create("hotbar", 8)
    inv.add("hotbar", "seed_turnip", 3)
    shop.set_catalog("general", { seed_turnip = { buy = 12, sell = 5 } })
    map.schema("farm", { day = "number" })
    map.state("farm", { day = 1 })
    interact.prompt("E: inspect plot")
    ui.text_at("gameplay_status", 20, 620, "Grid, clock, tags, interaction, inventory", 2)
    log.info("gameplay_api: registered item/inventory/shop/map state")
end

function M.on_day_start()
    log.info("gameplay_api: new day")
end

function M.update()
    local now = clock.now()
    local cell = grid.world_to_cell(128, 128, 32)
    if input.just_pressed("e") then
        local hits = interact.query(128, 128, 48)
        log.info("gameplay_api interactions=" .. tostring(#hits))
    end
    if input.just_pressed("space") then
        clock.add(30)
    end
    if input.just_pressed("d") then
        day.sleep()
    end
    return string.format(
        "gameplay_api_ok day=%d minute=%d cell=%d,%d seeds=%d price=%d",
        now.day,
        now.minute,
        cell[1],
        cell[2],
        inv.count("hotbar", "seed_turnip"),
        shop.price("general", "seed_turnip").buy
    )
end

return M
