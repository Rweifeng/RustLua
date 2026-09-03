-- 存档职责：启动自动恢复、F1 存、F2 读。
local state = require("state")

local M = {}

--- 首次进入时读取持久化存档（启动自动读档），必要时恢复存档场景。
--- 只在首次加载存档（启动）时触发场景恢复；此后 on_create 因场景切换
--- 再次运行时不重复切换（否则 F4 切到 arena 会被存档场景拉回 main）。
--- 返回 true 表示已请求场景切换，on_create 将以目标场景再次运行。
function M.auto_restore()
    if not state.save_loaded then
        save.load()
        state.save_loaded = true
        local saved_scene = save.read("scene")
        if saved_scene and saved_scene ~= state.scene_name and scene.load(saved_scene) then
            return true
        end
    end
    return false
end

function M.save(player)
    save.write("hp", tostring(health.get(player)))
    save.write("scene", state.scene_name)
    if save.commit() then
        log.info("save committed: hp=" .. health.get(player) .. " scene=" .. state.scene_name)
    end
end

function M.load(player)
    if save.load() then
        local saved_hp = save.read("hp")
        if saved_hp then
            health.set(player, tonumber(saved_hp))
        end
        local saved_scene = save.read("scene")
        if saved_scene and saved_scene ~= state.scene_name then
            scene.load(saved_scene)
        end
        log.info("save loaded: hp=" .. health.get(player))
    end
end

return M
