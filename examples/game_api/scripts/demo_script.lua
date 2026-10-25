-- api_script：每实体 `Script` 钩子 + 实体生命周期 API 的分步 Lab。
--
-- 场景里 `PatrolA` / `PatrolB` / `PatrolC` 三个实体挂的是**同一个**
-- `scripts/entity/patrol.lua`：`on_spawn(id)` / `on_update(id, dt)` 由引擎每帧驱动，
-- 位置与头顶 `pt dir=.. x=.. t=..` 文字都不是本文件摆出来的。本文件只观察它们
-- （`entity.find` / `entity.is_alive` / `transform.get`），原有每帧驱动逻辑一行没动；
-- 三只实体各有一条 `Script`，但共享一份 Lua 文件状态，位置相位互不相同。
--
-- 另一半是实体生命周期 API：`entity.spawn` 造一只只带 `Transform` 的探针实体
-- （`entity.spawn` 只插 Transform，脚本组件得由场景/预制体给），本演示用 `run()`
-- 每帧 `transform.set` 驱动它——这正是每实体 `on_update` 会做的事。随后按
-- 生成 → 名字查 → 稳定 ID 反查 → 读回稳定 ID → 判活 → 销毁 → 再判活的链路走一遍。
--
-- 看什么：
--   * 步骤 01：三只巡逻实体都在动，x 落在 patrol.lua 的 [420, 860] 轨道里，y 不变。
--   * 步骤 02：金色探针在舞台里左右摆动，位置由 run() 的 transform.set 逐帧写入。
--   * 步骤 03：check() 会把探针销毁、换名字重生，稳定 ID 仍指回新句柄。
--   * 步骤 04：destroy 之后 `entity.is_alive` / `entity.find` / `entity.find_id` 同时失效。
--   * 步骤 05：名字、句柄、稳定 ID 三条查询路径互相指回同一个实体。
-- 交互键：`g` 重建探针，`h` 销毁探针，`j` 把探针挪到舞台右侧。R / F5 等由 Lab 保留。
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run()` 在 headless 下只做内存读写；
-- 每个 `enter` 先清掉上一代探针再重建，读者乱按之后 F5 仍能全绿。
local lab = require("lab")

local M = {}

-- 场景里三只巡逻实体的名字与初始 y（patrol.lua 只改 x，y 是场景给的常数）。
local PATROL_NAMES = { "PatrolA", "PatrolB", "PatrolC" }
local PATROL_Y = { 240.0, 380.0, 520.0 }
local PATROL_MIN_X = 420.0
local PATROL_MAX_X = 860.0

-- 探针实体：名字 / 备用名字（演示"稳定 ID 不变、名字可以换"）/ 稳定 ID。
local PROBE_NAME = "LabProbe"
local PROBE_ALT = "LabProbeNext"
local PROBE_PERSIST = "demo.lab_probe"
local PROBE_TEXTURE = "assets/textures/coin.png"
local PROBE_Y = 300.0
local PROBE_X = 560.0

-- 模块状态（普通 Lua 局部变量，不是引擎状态）。
local patrols = {}
local probe = nil
local phase = 0.0

local function dash(value)
    if value == nil then
        return "-"
    end
    return tostring(value)
end

--- 按场景名重新查三只巡逻实体；名字是真源，不缓存数字 id 以外的任何东西。
local function find_patrols()
    patrols = {}
    for index, name in ipairs(PATROL_NAMES) do
        local id = entity.find(name)
        if id ~= nil and entity.is_alive(id) then
            patrols[#patrols + 1] = { name = name, id = id, y = PATROL_Y[index] }
        end
    end
    return #patrols
end

--- 把上一代探针收干净：句柄、稳定 ID、两个演示名字都查一遍。
--- check 里会用备用名字重生，所以 enter 不能只认 `probe` 这一个句柄。
local function drop_probe()
    if probe ~= nil and entity.is_alive(probe) then
        entity.destroy(probe)
    end
    probe = nil
    local by_persist = entity.find_id(PROBE_PERSIST)
    if by_persist ~= nil and entity.is_alive(by_persist) then
        entity.destroy(by_persist)
    end
    for _, name in ipairs({ PROBE_NAME, PROBE_ALT }) do
        local id = entity.find(name)
        if id ~= nil and entity.is_alive(id) then
            entity.destroy(id)
        end
    end
end

--- 给探针补一个可见的小精灵（`entity.spawn` 只给 Transform）。
local function paint_probe()
    if probe == nil or not entity.is_alive(probe) then
        return
    end
    sprite.set(
        probe,
        string.format(
            "texture=%s;size=22x22;pivot=0.500,0.500;tint=1.000,0.850,0.350,1.000;sorting_layer=default;order=6",
            PROBE_TEXTURE
        )
    )
end

--- 标准探针：固定名字 + 固定稳定 ID，出生在舞台中上部。
local function ensure_probe()
    drop_probe()
    probe = entity.spawn(PROBE_NAME, PROBE_PERSIST)
    transform.set(probe, PROBE_X, PROBE_Y)
    paint_probe()
    return probe
end

local steps = {
    {
        id = "script_entities",
        title = "Script hooks",
        claim = "三个实体共用同一份 patrol.lua：x 每帧由各自的 on_update 推进，y 保持不变",
        api = { "entity.find", "entity.is_alive", "transform.get" },
        code = "id = entity.find('PatrolA'); entity.is_alive(id); transform.get(id)",
        enter = function()
            find_patrols()
            lab.note("three entities, one script file: scripts/entity/patrol.lua")
        end,
        run = function()
            find_patrols()
            local parts = {}
            for _, patrol in ipairs(patrols) do
                local position = transform.get(patrol.id)
                parts[#parts + 1] = string.format("%s=%.0f", patrol.name, position.x)
            end
            lab.note(table.concat(parts, "  "))
        end,
        read = function()
            local rows = { { label = "alive", value = tostring(#patrols) } }
            for _, patrol in ipairs(patrols) do
                local position = transform.get(patrol.id)
                rows[#rows + 1] = { label = patrol.name, value = string.format("%.1f", position.x) }
            end
            rows[#rows + 1] = { label = "script", value = "entity/patrol.lua" }
            return rows
        end,
        check = function(t)
            t:eq(#patrols, 3, "three scene entities declare the same Script file")
            for index, patrol in ipairs(patrols) do
                t:ok(entity.is_alive(patrol.id), patrol.name .. " is alive")
                t:eq(entity.find(patrol.name), patrol.id, patrol.name .. " is found by name")
                local position = transform.get(patrol.id)
                t:ok(
                    position.x >= PATROL_MIN_X and position.x <= PATROL_MAX_X,
                    patrol.name .. " stays on its patrol track"
                )
                t:near(position.y, PATROL_Y[index], 0.01, patrol.name .. " keeps its scene y")
            end
        end,
    },
    {
        id = "spawn_drive",
        title = "Spawn & drive",
        claim = "entity.spawn 只给 Transform；探针每帧的 transform.set 就是 on_update 会做的事",
        api = { "entity.spawn", "transform.set", "transform.get", "entity.is_alive" },
        code = "p = entity.spawn('LabProbe','demo.lab_probe'); transform.set(p, x, y)",
        enter = function()
            ensure_probe()
            phase = 0.0
            lab.note("g respawns the probe / h destroys it / j moves it right")
        end,
        run = function(dt)
            if probe == nil or not entity.is_alive(probe) then
                return
            end
            phase = phase + dt * 60.0
            local x = PROBE_X + 40.0 * math.sin(phase * 0.3)
            transform.set(probe, x, PROBE_Y)
            lab.note(string.format("probe x=%.1f y=%.0f (frame-driven by run)", x, PROBE_Y))
        end,
        read = function()
            local alive = probe ~= nil and entity.is_alive(probe)
            local x = "-"
            local persist = nil
            if alive then
                x = string.format("%.1f", transform.get(probe).x)
                persist = entity.persist_id(probe)
            end
            return {
                { label = "handle", value = dash(probe) },
                { label = "x", value = x },
                { label = "y", value = string.format("%.0f", PROBE_Y) },
                { label = "alive", value = tostring(alive) },
                { label = "persist", value = dash(persist) },
            }
        end,
        check = function(t)
            if probe == nil or not entity.is_alive(probe) then
                t:ok(false, "probe entity is missing")
                return
            end
            local position = transform.get(probe)
            t:near(position.y, PROBE_Y, 0.01, "probe y is fixed by the demo")
            t:near(
                position.x,
                PROBE_X + 40.0 * math.sin(phase * 0.3),
                0.01,
                "run() moved the probe with transform.set"
            )
            t:ok(phase > 1.0, "three frames advanced the drive phase")
            t:eq(entity.persist_id(probe), PROBE_PERSIST, "probe keeps its stable id")
        end,
    },
    {
        id = "stable_id",
        title = "Stable ID",
        claim = "persist_id 是稳定键：换名字重生之后，find_id 仍然指回新句柄",
        api = { "entity.persist_id", "entity.find_id", "entity.find", "entity.spawn", "entity.destroy" },
        code = "entity.destroy(p); p2 = entity.spawn('LabProbeNext','demo.lab_probe')",
        enter = function()
            ensure_probe()
            lab.note("check() destroys the probe and respawns it under a second name")
        end,
        run = function()
            if probe == nil then
                return
            end
            local by_id = entity.find_id(PROBE_PERSIST)
            local back = entity.is_alive(probe) and entity.persist_id(probe) or nil
            lab.note(string.format("find_id=%s persist_id=%s", dash(by_id), dash(back)))
        end,
        read = function()
            local alive = probe ~= nil and entity.is_alive(probe)
            return {
                { label = "handle", value = dash(probe) },
                { label = "persist", value = dash(alive and entity.persist_id(probe) or nil) },
                { label = "find_id", value = dash(entity.find_id(PROBE_PERSIST)) },
                { label = "find_name", value = dash(entity.find(PROBE_NAME)) },
                { label = "alive", value = tostring(alive) },
            }
        end,
        check = function(t)
            local first = probe
            t:ok(first ~= nil and entity.is_alive(first), "probe exists before the round trip")
            if first == nil or not entity.is_alive(first) then
                return
            end
            t:eq(entity.persist_id(first), PROBE_PERSIST, "persist_id reads the registered stable id")
            t:eq(entity.find(PROBE_NAME), first, "find resolves the current name")
            t:eq(entity.find_id(PROBE_PERSIST), first, "find_id resolves the stable id")
            entity.destroy(first)
            t:ok(not entity.is_alive(first), "destroyed handle is no longer alive")
            t:eq(entity.find(PROBE_NAME), nil, "destroy unregisters the name")
            t:eq(entity.find_id(PROBE_PERSIST), nil, "destroy unregisters the stable id")
            probe = entity.spawn(PROBE_ALT, PROBE_PERSIST)
            transform.set(probe, 600.0, PROBE_Y)
            paint_probe()
            t:ok(probe ~= first, "respawn with the same stable id gets a new handle")
            t:eq(entity.find(PROBE_ALT), probe, "the new name maps to the new handle")
            t:eq(entity.find_id(PROBE_PERSIST), probe, "the stable id follows the new handle")
            t:eq(entity.persist_id(probe), PROBE_PERSIST, "the stable id round-trips on the new handle")
        end,
    },
    {
        id = "destroy_alive",
        title = "Destroy",
        claim = "destroy 是判活之后的安全操作：句柄立刻失效，名字与稳定 ID 一起注销",
        api = { "entity.is_alive", "entity.destroy", "entity.spawn", "entity.find" },
        code = "entity.destroy(p); entity.is_alive(p) -> false",
        enter = function()
            ensure_probe()
            lab.note("h destroys the probe, g respawns it")
        end,
        run = function()
            local alive = probe ~= nil and entity.is_alive(probe)
            lab.note(string.format("probe alive=%s", tostring(alive)))
        end,
        read = function()
            local alive = probe ~= nil and entity.is_alive(probe)
            return {
                { label = "handle", value = dash(probe) },
                { label = "alive", value = tostring(alive) },
                { label = "find_name", value = dash(entity.find(PROBE_NAME)) },
                { label = "find_id", value = dash(entity.find_id(PROBE_PERSIST)) },
            }
        end,
        check = function(t)
            local id = probe
            t:ok(id ~= nil and entity.is_alive(id), "probe is alive before destroy")
            if id == nil or not entity.is_alive(id) then
                return
            end
            t:eq(entity.find(PROBE_NAME), id, "name points at the probe")
            entity.destroy(id)
            t:ok(not entity.is_alive(id), "is_alive is false right after destroy")
            t:eq(entity.find(PROBE_NAME), nil, "name lookup returns nil after destroy")
            t:eq(entity.find_id(PROBE_PERSIST), nil, "stable id lookup returns nil after destroy")
            probe = entity.spawn(PROBE_NAME, PROBE_PERSIST)
            transform.set(probe, 580.0, PROBE_Y)
            paint_probe()
            t:ok(entity.is_alive(probe), "respawn succeeds on the released name")
        end,
    },
    {
        id = "find_roundtrip",
        title = "Find round trip",
        claim = "名字、句柄、稳定 ID 三条查询路径指向同一个实体，transform 也读得到",
        api = { "entity.find", "entity.find_id", "entity.persist_id", "transform.get", "transform.set" },
        code = "entity.find('LabProbe'); entity.find_id('demo.lab_probe'); entity.persist_id(id)",
        enter = function()
            ensure_probe()
            transform.set(probe, 620.0, PROBE_Y)
            lab.note("j moves the probe to x=640")
        end,
        run = function()
            local by_name = entity.find(PROBE_NAME)
            local by_id = entity.find_id(PROBE_PERSIST)
            lab.note(string.format("find=%s find_id=%s", dash(by_name), dash(by_id)))
        end,
        read = function()
            local alive = probe ~= nil and entity.is_alive(probe)
            local x, y = nil, nil
            if alive then
                local position = transform.get(probe)
                x = string.format("%.1f", position.x)
                y = string.format("%.1f", position.y)
            end
            return {
                { label = "find", value = dash(entity.find(PROBE_NAME)) },
                { label = "find_id", value = dash(entity.find_id(PROBE_PERSIST)) },
                { label = "persist", value = dash(alive and entity.persist_id(probe) or nil) },
                { label = "x", value = dash(x) },
                { label = "y", value = dash(y) },
            }
        end,
        check = function(t)
            if probe == nil or not entity.is_alive(probe) then
                t:ok(false, "probe entity is missing")
                return
            end
            t:eq(entity.find(PROBE_NAME), probe, "find and the handle agree")
            t:eq(entity.find_id(PROBE_PERSIST), probe, "find_id and the handle agree")
            t:eq(entity.persist_id(probe), PROBE_PERSIST, "persist_id and find_id are inverse queries")
            local position = transform.get(probe)
            t:near(position.x, 620.0, 0.01, "transform.set placed the probe")
            t:near(position.y, PROBE_Y, 0.01, "transform.get reads the same position back")
        end,
    },
}

function M.on_create()
    find_patrols()
    lab.start({
        scene = "api_script",
        title = "Entity Scripts",
        category = "Script",
        steps = steps,
        -- ScriptHint 在 (12, 8)：会压住 Lab 的页眉，隐藏它（stop 时恢复）。
        hide_scene_text = { "ScriptHint" },
    })
    log.info(string.format("api_script lab: %d steps, %d patrols", #steps, #patrols))
end

function M.update()
    local step = lab.step()
    local id = step and step.id or ""
    if id == "script_entities" then
        if input.just_pressed("g") then
            find_patrols()
        end
    elseif id == "spawn_drive" or id == "stable_id" or id == "destroy_alive" or id == "find_roundtrip" then
        if input.just_pressed("g") then
            ensure_probe()
        elseif input.just_pressed("h") then
            drop_probe()
        elseif input.just_pressed("j") then
            if probe == nil or not entity.is_alive(probe) then
                ensure_probe()
            end
            transform.set(probe, 640.0, PROBE_Y)
            phase = 0.0
        end
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 探针是本模块创建的实体：离开场景前销毁，别把句柄留给下一个场景。
    drop_probe()
    lab.stop()
end

return M
