-- api_prefab：**预制体（Prefab）是编辑器侧的特性**——`assets/prefabs/*.prefab` 是一份可复用的
-- 实体子树，编辑器把它的实例写进场景，并在实例实体上留下 `prefab.root` /
-- `prefab.source` / `prefab.source_id` 三条**实体元数据**（本场景的 4 只史莱姆就是两个源
-- 各实例化两次，见 `scenes/api_prefab.scene` 的 `entity_meta` 行）。
--
-- 运行期**没有** prefab API：Lua 看到的只是普通实体，`entity.find` / `sprite` / `animation` /
-- `transform` 照常生效。这个演示想说明的正是这条边界——"预制体在编辑期复用，运行期没有特殊身份"。
local api = require("api_demo")

local M = {}

local instances = {}
local elapsed = 0
local action = "ready"

local function tint(id, r, g, b)
    -- `sprite.set` 用与场景**同一套组件串语法**：改色 = 重写 tint 段。
    local current = sprite.get(id)
    sprite.set(
        id,
        string.format(
            "texture=%s;size=%dx%d;pivot=%.2f,%.2f;tint=%.3f,%.3f,%.3f,%.3f;"
                .. "sorting_layer=actors;order=%d",
            current.texture,
            current.size_x,
            current.size_y,
            current.pivot_x,
            current.pivot_y,
            r,
            g,
            b,
            current.tint_a,
            current.order
        )
    )
end

function M.on_create()
    api.title("api_prefab - prefab instances are plain entities at runtime")
    api.hint("1 tint every green instance - 2 tint every red instance - 3 restart the squish clip - " .. api.KEYS_BACK)
    for _, name in ipairs({ "SlimeGreen1", "SlimeGreen2", "SlimeRed1", "SlimeRed2" }) do
        local id = entity.find(name)
        if id then
            instances[#instances + 1] = { name = name, id = id }
        end
    end
    log.info(string.format("api_prefab: %d prefab instances found", #instances))
end

function M.update()
    elapsed = elapsed + time.delta()

    if input.just_pressed("1") then
        for _, instance in ipairs(instances) do
            if instance.name:find("Green") then
                tint(instance.id, 0.400, 1.000, 0.400)
            end
        end
        action = "greens tinted"
    elseif input.just_pressed("2") then
        for _, instance in ipairs(instances) do
            if instance.name:find("Red") then
                tint(instance.id, 1.000, 0.400, 0.400)
            end
        end
        action = "reds tinted"
    elseif input.just_pressed("3") then
        for _, instance in ipairs(instances) do
            -- `animation.set` 切到状态机里的另一个（这里是同一个）片段并**重播**，
            -- 于是四只史莱姆的挤压动画重新对齐——这正是"实例各自独立"的可见证据。
            animation.set(instance.id, "squish")
        end
        action = "squish restarted"
    end

    -- 实例之间互不影响：各自按自己的相位上下浮动（`transform.move` 是增量移动）。
    local first_id = instances[1] and instances[1].id
    local first_x, first_y = 0.0, 0.0
    for index, instance in ipairs(instances) do
        local phase = elapsed + index * 0.6
        transform.move(instance.id, 0, math.sin(phase) * 0.05)
        if index == 1 then
            local t = transform.get(instance.id)
            first_x, first_y = t.x, t.y
        end
    end

    api.status(
        string.format(
            "%d instances from 2 prefab sources - last action: %s - %s at (%.0f, %.0f) - sprite %s",
            #instances,
            action,
            instances[1] and instances[1].name or "?",
            first_x,
            first_y,
            first_id and sprite.get(first_id).texture or "(none)"
        )
    )
    return string.format("api_prefab_ok instances=%d action=%s", #instances, action)
end

return M
