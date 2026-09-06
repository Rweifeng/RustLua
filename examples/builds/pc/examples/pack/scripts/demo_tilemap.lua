local M = {}

local map_id = nil

function M.on_create()
    map_id = entity.find("RuntimeTilemap")
    if not map_id or not entity.is_alive(map_id) then
        map_id = tilemap.create(8, 6, 32, "assets/textures/tileset.png", "RuntimeTilemap")
    end
    for x = 0, 7 do
        tilemap.set_cell(map_id, x, 5, (x % 6) + 1)
        tilemap.set_solid(map_id, x, 5, false)
    end
    tilemap.set_meta(map_id, 2, 4, "kind", "chest")
    ui.text_at("tilemap_status", 20, 620, "Tilemap cells, collision and metadata", 2)
    log.info("tilemap: created/updated map")
end

function M.update()
    local info = tilemap.get(map_id)
    if input.just_pressed("space") then
        tilemap.set_cell(map_id, 4, 4, 6)
        tilemap.set_solid(map_id, 4, 4, true)
    end
    if input.just_pressed("r") then
        tilemap.set_cell(map_id, 4, 4, 0)
        tilemap.set_solid(map_id, 4, 4, false)
    end
    return string.format(
        "tilemap_ok %dx%d solid_4_4=%s meta=%s",
        info.width,
        info.height,
        tostring(tilemap.is_solid(map_id, 4, 4)),
        tostring(tilemap.get_meta(map_id, 2, 4, "kind"))
    )
end

return M
