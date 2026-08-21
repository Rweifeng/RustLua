log.info("project script loaded")

local player_name = "Player"
local move_speed = 240 -- world units per second

function update()
    local dt = time.delta()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local player = entity.find(player_name)
    local position = "?"
    if player and entity.is_alive(player) then
        transform.move(player, horizontal * move_speed * dt, vertical * move_speed * dt)
        local p = transform.get(player)
        position = string.format("(%.1f,%.1f)", p.x, p.y)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s",
        dt,
        horizontal,
        vertical,
        tostring(player ~= nil),
        position
    )
end
