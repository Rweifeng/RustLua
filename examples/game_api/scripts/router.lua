-- 场景路由：把工程级回调分发给当前场景对应的演示模块（`scripts/demos.lua` 是清单真源）。
--
-- 为什么用"工程级一份路由"而不是每个场景一个 `Script` 组件：演示模块之间共享
-- 入口菜单、ESC 返回、状态行这些横切行为，路由是它们的唯一落点；每实体 `Script`
-- 的用法由 `api_script` 场景单独演示（`scripts/entity/*.lua`）。
local api = require("api_demo")

local M = {}

-- 场景名 → 模块名。演示清单（`scripts/demos.lua`）是唯一真源，但**入口菜单自己不是演示**：
-- 它不列进清单（不进菜单卡片、不计入覆盖率清单），因此单独挂在 `ROUTES` 里。
local ROUTES = { index = "demo_index" }
local LABELS = {}
for _, entry in ipairs(require("demos")) do
    ROUTES[entry.scene] = entry.module
    LABELS[entry.scene] = entry.label
end

local current = "index"
local active = nil

local function module_for(scene_name)
    local name = ROUTES[scene_name]
    if not name then
        return nil
    end
    return require(name)
end

function M.scene_name()
    return current
end

function M.label(scene_name)
    return LABELS[scene_name]
end

function M.on_create()
    -- `scene.reload()` 返回**当前场景名**（不是重新加载），它同时是宿主真源：
    -- 切场景后脚本 VM 会重建，这里读到的就是新场景。
    current = scene.reload() or "index"
    active = module_for(current)
    if active and active.on_create then
        active.on_create()
    end
    log.info(
        string.format(
            "api showcase: scene=%s demo=%s",
            current,
            ROUTES[current] or "(none)"
        )
    )
end

function M.update()
    -- 命名动作 `cancel`（键盘 ESC / 手柄 B）回到入口菜单：这是"动作名"而不是物理键，
    -- 因此键位在手柄、键盘之间共用一条绑定（`input.action_pressed`）。
    if current ~= "index" and input.action_pressed("cancel") then
        scene.load("index")
        return "api_showcase_back_to_index"
    end
    local result = string.format("api_showcase scene=%s", current)
    if active and active.update then
        result = active.update() or result
    end
    api.render()
    return result
end

function M.on_destroy()
    if active and active.on_destroy then
        active.on_destroy()
    end
    api.clear()
end

function M.on_click(name)
    if active and active.on_click then
        return active.on_click(name)
    end
    return false
end

function M.on_drag(from_id, from_index, to_id, to_index)
    if active and active.on_drag then
        return active.on_drag(from_id, from_index, to_id, to_index)
    end
    return false
end

function M.on_day_start()
    if active and active.on_day_start then
        return active.on_day_start()
    end
    return false
end

function M.on_day_end()
    if active and active.on_day_end then
        return active.on_day_end()
    end
    return false
end

return M
