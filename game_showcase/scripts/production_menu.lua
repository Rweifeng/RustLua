local state = require("production_state")
local save_manager = require("production_save_manager")

local M = {}

function M.on_create()
    state.current_scene()
    log.info("menu scene ready")
end

function M.update()
    if input.just_pressed("space") then
        scene.load(state.production_scene)
    elseif input.just_pressed("f2") then
        save_manager.load_saved_scene(state.production_scene)
    end
    return "menu_ready"
end

function M.on_click(name)
    if name == "StartButton" then
        scene.load(state.production_scene)
    elseif name == "LoadButton" then
        save_manager.load_saved_scene(state.production_scene)
    else
        log.info("menu button clicked: " .. tostring(name))
    end
end

return M
