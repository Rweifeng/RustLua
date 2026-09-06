local M = {}

local current = scene.reload() or "index"
local active = nil

local function module_for(scene_name)
    if scene_name == "menu" then
        return require("production_menu")
    elseif scene_name == "production" then
        return require("production_gameplay")
    elseif scene_name == "main" or scene_name == "arena" then
        return require("game")
    elseif scene_name == "demo3d" then
        return require("demo3d")
    elseif scene_name == "game3d" then
        return require("game3d")
    elseif scene_name == "ui_layout" then
        return require("demo_ui")
    elseif scene_name == "ui_test" then
        return require("demo_ui_events")
    elseif scene_name == "ui_dynamic" then
        return require("demo_ui_dynamic")
    elseif scene_name == "tilemap" then
        return require("demo_tilemap")
    elseif scene_name == "effects" then
        return require("demo_effects")
    elseif scene_name == "gameplay_api" then
        return require("demo_gameplay_api")
    elseif scene_name == "data_inventory" then
        return require("demo_data")
    elseif scene_name == "persistence" then
        return require("demo_persistence")
    elseif scene_name == "resources" then
        return require("demo_resources")
    elseif scene_name == "stress" then
        return require("demo_stress")
    elseif scene_name == "physics3d" then
        return require("demo_physics3d")
    end
    return nil
end

local function return_index()
    scene.load("index")
end

local function maybe_return_index()
    if current ~= "index" and input.just_pressed("escape") then
        return_index()
        return true
    end
    return false
end

function M.on_create()
    current = scene.reload() or "index"
    active = module_for(current)
    if active and active.on_create then
        active.on_create()
    end
end

function M.update()
    if maybe_return_index() then
        return "showcase-return-index"
    end
    if active and active.update then
        return active.update()
    end
    return "showcase-ok scene=" .. tostring(current)
end

function M.on_click(name)
    if current == "index" then
        if name == "BtnClassic2D" then
            scene.load("main")
        elseif name == "BtnProductionMenu" then
            scene.load("menu")
        elseif name == "BtnProduction" then
            scene.load("production")
        elseif name == "Btn3DMesh" then
            scene.load("demo3d")
        elseif name == "Btn3DGameplay" then
            scene.load("game3d")
        elseif name == "BtnUiEvents" then
            scene.load("ui_test")
        elseif name == "BtnUiLayout" then
            scene.load("ui_layout")
        elseif name == "BtnUiDynamic" then
            scene.load("ui_dynamic")
        elseif name == "BtnTilemap" then
            scene.load("tilemap")
        elseif name == "BtnEffects" then
            scene.load("effects")
        elseif name == "BtnGameplayApi" then
            scene.load("gameplay_api")
        elseif name == "BtnData" then
            scene.load("data_inventory")
        elseif name == "BtnPersistence" then
            scene.load("persistence")
        elseif name == "BtnResources" then
            scene.load("resources")
        elseif name == "BtnStress" then
            scene.load("stress")
        elseif name == "BtnPhysics3D" then
            scene.load("physics3d")
        end
        return
    end
    if active and active.on_click then
        active.on_click(name)
    end
end

function M.on_drag(from_id, from_index, to_id, to_index)
    if active and active.on_drag then
        active.on_drag(from_id, from_index, to_id, to_index)
    end
end

function M.on_day_start()
    if active and active.on_day_start then
        active.on_day_start()
    end
end

function M.on_day_end()
    if active and active.on_day_end then
        active.on_day_end()
    end
end

function M.on_migrate(from, to)
    log.info("save migrated: " .. tostring(from) .. " -> " .. tostring(to))
end

return M
