local M = {}

-- Production demos keep their state isolated from the classic 2D demo.

M.menu_scene = "menu"
M.production_scene = "production"
M.player_name = "Player"
M.map_name = "Tilemap"
M.enemy_prefixes = { "SlimeGreen", "SlimeRed" }
M.coin_prefix = "Coin"

M.scene_name = "menu"
M.map_id = nil
M.tile_size = 32
M.move_speed = 240
M.enemy_speed = 80
M.max_health = 100
M.attack_range = 56
M.attack_cone = 0.707
M.facing = { x = 0, y = -1 }

M.enemies = {}
M.coins = {}
M.coin_total = 0
M.score = 0
M.event_count = 0
M.music_started = false
M.game_active = false
M.game_over = false
M.game_win = false
M.fresh_round = false

function M.find_all(prefix)
    local list = {}
    local i = 1
    while true do
        local entity_id = entity.find(prefix .. i)
        if not entity_id or not entity.is_alive(entity_id) then
            break
        end
        table.insert(list, entity_id)
        i = i + 1
    end
    return list
end

function M.find_all_from_prefixes(prefixes)
    local list = {}
    for _, prefix in ipairs(prefixes) do
        local items = M.find_all(prefix)
        for _, entity_id in ipairs(items) do
            table.insert(list, entity_id)
        end
    end
    return list
end

function M.alive_count(list)
    local count = 0
    for _, entity_id in ipairs(list) do
        if entity.is_alive(entity_id) then
            count = count + 1
        end
    end
    return count
end

function M.is_in(list, entity_id)
    for _, current in ipairs(list) do
        if current == entity_id then
            return true
        end
    end
    return false
end

function M.current_scene()
    local loaded = scene.reload()
    if loaded then
        M.scene_name = loaded
    end
    return M.scene_name
end

return M
