-- api_prefab：编辑期预制体 vs 运行期 `prefab` 表。
--
-- 两个不相干的概念要分清：
--   1. 编辑期 `.prefab` 实例：场景里的 4 只史莱姆来自两份源（`entity_meta prefab.*`），
--      运行期 Lua 看到的只是普通实体——`sprite` / `animation` / `transform` 照常生效；
--   2. 运行期 `prefab` 表：把 `.prefab` 子树实例化进活世界，用 handle 读写节点
--      （`root` / `node` / `set_text` / `set_rect` / `set_visible` / `despawn`）。
local lab = require("lab")

local M = {}

local INSTANCES = { "SlimeGreen1", "SlimeGreen2", "SlimeRed1", "SlimeRed2" }
local RUNTIME_PREFAB = "assets/prefabs/demo_ui.prefab"
local instances = {}
local handle = nil
local box_visible = true
local action = "ready"
local last_text = ""

local function find_instances()
    instances = {}
    for _, name in ipairs(INSTANCES) do
        local id = entity.find(name)
        if id then
            instances[#instances + 1] = { name = name, id = id }
        end
    end
end

local function tint(id, r, g, b)
    local current = sprite.get(id)
    sprite.set(
        id,
        string.format(
            "texture=%s;size=%dx%d;pivot=%.2f,%.2f;tint=%.3f,%.3f,%.3f,%.3f;sorting_layer=actors;order=%d",
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

local function tint_matching(fragment, r, g, b)
    for _, instance in ipairs(instances) do
        if instance.name:find(fragment) then
            tint(instance.id, r, g, b)
        end
    end
end

local function spawn_runtime()
    local value, err = lab.try(prefab.spawn, RUNTIME_PREFAB)
    if value then
        handle = value
    else
        handle = nil
        action = "spawn failed: " .. tostring(err)
    end
end

local function ensure_runtime()
    if handle == nil then
        spawn_runtime()
    end
    return handle
end

local function drop_runtime()
    local removed = false
    if handle ~= nil then
        removed = prefab.despawn(handle) or false
        handle = nil
    end
    return removed
end

local function reset_instances()
    find_instances()
    for _, instance in ipairs(instances) do
        if instance.name:find("Green") then
            tint(instance.id, 0.600, 1.000, 0.600)
        else
            tint(instance.id, 1.000, 0.550, 0.550)
        end
    end
end

local steps = {
    {
        id = "editor_instances",
        title = "Editor instances",
        claim = "编辑期预制体的 4 个实例，在运行期就是普通实体",
        api = { "entity.find", "sprite.get", "animation.get", "transform.get" },
        code = "entity.find(name) / sprite.get(id) / animation.get(id)",
        enter = function()
            reset_instances()
        end,
        run = function()
            action = "4 instances from 2 prefab sources"
            lab.note(action)
        end,
        read = function()
            local first = instances[1]
            local t = first and transform.get(first.id) or { x = 0, y = 0 }
            return {
                instances = #instances,
                texture = first and sprite.get(first.id).texture or "-",
                clip = first and animation.get(first.id) or "-",
                first_x = string.format("%.0f", t.x),
                first_y = string.format("%.0f", t.y),
            }
        end,
        check = function(t)
            t:eq(#instances, 4, "4 prefab instances should be found")
            t:ok(
                instances[1] and sprite.get(instances[1].id).texture:find("slime.png") ~= nil,
                "first instance keeps its Sprite"
            )
            t:eq(animation.get(instances[1].id), "squish", "animation state survives instantiation")
        end,
    },
    {
        id = "independent_state",
        title = "Instance state",
        claim = "同一份 prefab 的两次实例共享外观，但不共享运行期状态",
        api = { "sprite.get", "sprite.set", "transform.move", "transform.get" },
        code = "sprite.set(id, spec) / transform.move(id, dx, dy)",
        enter = function()
            find_instances()
            tint_matching("Green1", 0.400, 1.000, 0.400)
        end,
        run = function(dt)
            for index, instance in ipairs(instances) do
                transform.move(instance.id, 0, math.sin(index * 0.9) * 0.4 * dt)
            end
            action = "Green1 tinted; each instance moves on its own phase"
            lab.note(action)
        end,
        read = function()
            local first = sprite.get(instances[1].id)
            local second = sprite.get(instances[2].id)
            local a = transform.get(instances[1].id)
            local b = transform.get(instances[2].id)
            return {
                green1_r = string.format("%.2f", first.tint_r),
                green2_r = string.format("%.2f", second.tint_r),
                delta_y = string.format("%.2f", a.y - b.y),
            }
        end,
        check = function(t)
            local first = sprite.get(instances[1].id)
            local second = sprite.get(instances[2].id)
            t:near(first.tint_r, 0.4, 0.01, "first green instance was re-tinted")
            t:near(second.tint_r, 0.6, 0.01, "second instance keeps its own tint")
        end,
    },
    {
        id = "animation_per_instance",
        title = "Animation state",
        claim = "动画状态属于实例：切换只重播它自己的剪辑",
        api = { "animation.set", "animation.get" },
        code = "animation.set(id, 'squish'); animation.get(id)",
        enter = function()
            find_instances()
            for _, instance in ipairs(instances) do
                animation.set(instance.id, "squish")
            end
        end,
        run = function()
            action = "squish restarted on all four instances"
            lab.note(action)
        end,
        read = function()
            return {
                green1 = animation.get(instances[1].id),
                green2 = animation.get(instances[2].id),
                red1 = animation.get(instances[3].id),
                red2 = animation.get(instances[4].id),
            }
        end,
        check = function(t)
            for _, instance in ipairs(instances) do
                t:eq(animation.get(instance.id), "squish", instance.name .. " is on squish")
            end
        end,
    },
    {
        id = "runtime_handle",
        title = "Runtime prefab",
        claim = "运行期 prefab 句柄能取到实例的根与命名节点",
        api = { "prefab.spawn", "prefab.root", "prefab.node" },
        code = "h = prefab.spawn(path); prefab.root(h); prefab.node(h, 'DemoLabel')",
        enter = function()
            ensure_runtime()
        end,
        run = function()
            action = handle and ("handle " .. handle .. " alive") or "no handle"
            lab.note(action)
        end,
        read = function()
            local root_id = handle and prefab.root(handle) or nil
            local label_id = handle and prefab.node(handle, "DemoLabel") or nil
            return {
                handle = handle or "-",
                root = root_id or "-",
                label = label_id or "-",
                alive = tostring(root_id ~= nil and entity.is_alive(root_id)),
            }
        end,
        check = function(t)
            t:ok(handle ~= nil, "prefab.spawn returned a handle")
            if not handle then
                return
            end
            local root_id = prefab.root(handle)
            local label_id = prefab.node(handle, "DemoLabel")
            t:ok(entity.is_alive(root_id), "instance root is a live entity")
            t:ok(entity.is_alive(label_id), "named node is a live entity")
        end,
    },
    {
        id = "node_ops",
        title = "Node operations",
        claim = "set_text / set_rect / set_visible 改的是实例节点；despawn 幂等",
        api = { "prefab.set_text", "prefab.set_rect", "prefab.set_visible", "prefab.despawn" },
        code = "prefab.set_text(h,'DemoLabel',...); prefab.set_rect(...); prefab.set_visible(...)",
        enter = function()
            ensure_runtime()
            if handle == nil then
                return
            end
            last_text = "runtime text " .. tostring(math.floor(time.delta() * 1000))
            prefab.set_text(handle, "DemoLabel", last_text)
            prefab.set_rect(handle, "DemoBox", 400, 280, 880, 440)
            box_visible = true
            prefab.set_visible(handle, "DemoBox", box_visible)
        end,
        run = function()
            action = string.format(
                "text=%s rect=400,280,880,440 visible=%s",
                last_text,
                tostring(box_visible)
            )
            lab.note(action)
        end,
        read = function()
            local label_id = handle and prefab.node(handle, "DemoLabel") or nil
            return {
                label = label_id or "-",
                visible = tostring(box_visible),
                handle = handle or "-",
            }
        end,
        check = function(t)
            t:ok(handle ~= nil, "runtime prefab handle exists")
            if not handle then
                return
            end
            local label_id = prefab.node(handle, "DemoLabel")
            prefab.set_text(handle, "DemoLabel", "selftest")
            prefab.set_rect(handle, "DemoBox", 400, 280, 880, 440)
            prefab.set_visible(handle, "DemoBox", false)
            t:ok(entity.is_alive(label_id), "node operations keep the node alive")
            local removed = prefab.despawn(handle)
            local again = prefab.despawn(handle)
            handle = nil
            t:ok(removed, "first despawn returns true")
            t:ok(not again, "second despawn is idempotent (false)")
            spawn_runtime()
        end,
    },
}

function M.on_create()
    find_instances()
    lab.start({
        scene = "api_prefab",
        title = "Prefab Instances",
        category = "Script",
        steps = steps,
        hide_scene_text = { "PrefabHud" },
    })
    log.info(string.format("api_prefab: %d prefab instances found", #instances))
end

function M.update()
    local step = lab.step()
    if step and step.id == "node_ops" then
        if input.just_pressed("t") and handle then
            last_text = "set_text #" .. tostring(math.floor(time.delta() * 10000))
            prefab.set_text(handle, "DemoLabel", last_text)
            action = "set_text"
        elseif input.just_pressed("u") and handle then
            prefab.set_rect(handle, "DemoBox", 400, 280, 880, 440)
            action = "set_rect"
        elseif input.just_pressed("v") and handle then
            box_visible = not box_visible
            prefab.set_visible(handle, "DemoBox", box_visible)
            action = box_visible and "set_visible(true)" or "set_visible(false)"
        elseif input.just_pressed("d") and handle then
            local removed = drop_runtime()
            spawn_runtime()
            action = "despawn=" .. tostring(removed) .. " then respawn"
        end
    end
    if input.just_pressed("g") then
        tint_matching("Green", 0.400, 1.000, 0.400)
        action = "greens tinted"
    elseif input.just_pressed("b") then
        tint_matching("Red", 1.000, 0.400, 0.400)
        action = "reds tinted"
    elseif input.just_pressed("a") then
        for _, instance in ipairs(instances) do
            animation.set(instance.id, "squish")
        end
        action = "squish restarted"
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    drop_runtime()
    lab.stop()
end

return M
