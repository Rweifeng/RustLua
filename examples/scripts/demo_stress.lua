local M = {}

local spawned = 0
local angle = 0

function M.on_create()
    local side = 16
    for y = 0, side - 1 do
        for x = 0, side - 1 do
            local id = entity.spawn("StressSprite" .. tostring(spawned))
            sprite.set(id, "assets/textures/coin.png;size=16")
            transform.set(id, 180 + x * 28, 130 + y * 24)
            spawned = spawned + 1
        end
    end
    ui.text_at("stress_status", 20, 620, "256 sprites: ECS, transform and culling", 2)
end

function M.update()
    angle = angle + time.delta() * 0.8
    for index = 1, spawned do
        local id = entity.find("StressSprite" .. tostring(index - 1))
        if id and entity.is_alive(id) then
            local row = math.floor((index - 1) / 16)
            local col = (index - 1) % 16
            transform.set(id, 180 + col * 28 + math.sin(angle + row * 0.2) * 8, 130 + row * 24)
        end
    end
    return "stress_ok sprites=" .. tostring(spawned)
end

return M
