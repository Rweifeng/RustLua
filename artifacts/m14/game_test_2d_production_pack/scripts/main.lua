local state = require("state")
local menu = require("menu")
local gameplay = require("gameplay")

local function route_scene()
    return state.current_scene()
end

function on_create()
    local current = route_scene()
    if current == state.menu_scene then
        menu.on_create()
    else
        gameplay.on_create()
    end
end

function update()
    local current = route_scene()
    if current == state.menu_scene then
        return menu.update()
    end
    return gameplay.update()
end

function on_click(name)
    local current = route_scene()
    if current == state.menu_scene then
        menu.on_click(name)
        return
    end
    if name == "BackToMenuButton" then
        scene.load(state.menu_scene)
    else
        log.info("button clicked: " .. tostring(name))
    end
end
