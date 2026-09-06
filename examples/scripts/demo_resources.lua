local M = {}

local token = nil

function M.on_create()
    token = asset.load("assets/textures/coin.png")
    ui.text_at("resources_status", 20, 620, "Asset handles and runtime sprite creation", 2)
    local id = entity.spawn("ResourceSprite")
    sprite.set(id, "assets/textures/coin.png;size=64")
    transform.set(id, 220, 180)
end

function M.update()
    if input.just_pressed("r") and token then
        asset.release(token)
        token = nil
    elseif not token then
        token = asset.load("assets/textures/coin.png")
    end
    return "resources_ok token=" .. tostring(token)
end

return M
