-- 敌人（史莱姆）职责：收集敌人列表、追击玩家。
local state = require("state")

local M = {}

function M.find_all()
    state.enemies = state.find_all("Slime")
    return state.enemies
end

--- 追击玩家：每只存活的史莱姆朝玩家位置移动
function M.chase(player_id)
    if not player_id or not entity.is_alive(player_id) then
        return false
    end
    local p = transform.get(player_id)
    for _, slime in ipairs(state.enemies) do
        if entity.is_alive(slime) then
            local sp = transform.get(slime)
            local dx = p.x - sp.x
            local dy = p.y - sp.y
            local len = math.sqrt(dx * dx + dy * dy)
            if len > 1 then
                physics.set_velocity(
                    slime,
                    dx / len * state.enemy_speed,
                    dy / len * state.enemy_speed
                )
            end
        end
    end
    return state.alive_count(state.enemies) > 0
end

return M
