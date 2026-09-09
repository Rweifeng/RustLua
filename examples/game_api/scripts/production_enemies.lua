local state = require("production_state")

local M = {}

function M.find_all()
    state.enemies = state.find_all_from_prefixes(state.enemy_prefixes)
    return state.enemies
end

function M.chase(player_id)
    if not player_id or not entity.is_alive(player_id) then
        return false
    end
    local player_position = transform.get(player_id)
    for _, enemy_id in ipairs(state.enemies) do
        if entity.is_alive(enemy_id) then
            local enemy_position = transform.get(enemy_id)
            local dx = player_position.x - enemy_position.x
            local dy = player_position.y - enemy_position.y
            local length = math.sqrt(dx * dx + dy * dy)
            if length > 1 then
                physics.set_velocity(
                    enemy_id,
                    dx / length * state.enemy_speed,
                    dy / length * state.enemy_speed
                )
            end
        end
    end
    return state.alive_count(state.enemies) > 0
end

return M
