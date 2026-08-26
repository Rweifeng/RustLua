local state = require("state")

local M = {}

function M.save(player_id)
    if not player_id or not entity.is_alive(player_id) then
        return false
    end
    save.write("hp", tostring(health.get(player_id)))
    save.write("scene", state.scene_name)
    save.write("score", tostring(state.score))
    save.write("coins_total", tostring(state.coin_total))
    if save.commit() then
        log.info(
            "save committed: scene="
                .. state.scene_name
                .. " hp="
                .. tostring(health.get(player_id))
                .. " score="
                .. tostring(state.score)
        )
        return true
    end
    return false
end

function M.load(player_id)
    if not save.load() then
        return false
    end
    local saved_scene = save.read("scene")
    if saved_scene and saved_scene ~= state.scene_name then
        scene.load(saved_scene)
        return true
    end
    local saved_hp = save.read("hp")
    if saved_hp and player_id and entity.is_alive(player_id) then
        health.set(player_id, tonumber(saved_hp))
    end
    log.info("save loaded: scene=" .. tostring(saved_scene or state.scene_name))
    return true
end

function M.load_saved_scene(default_scene)
    if not save.load() then
        scene.load(default_scene)
        return true
    end
    local saved_scene = save.read("scene") or default_scene
    log.info("loading saved scene: " .. tostring(saved_scene))
    return scene.load(saved_scene)
end

return M
