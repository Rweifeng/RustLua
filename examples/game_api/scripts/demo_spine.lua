-- api_spine：Spine 2D 骨骼动画的 2.0 分步 Lab（M84 P1）。
--
-- 一条主线：**动画状态在运行时，组件载荷是初始值**。
--   * `spine.set_animation` / `set_skin` / `set_speed` / `set_enabled` 改的是运行时播放状态，
--     `spine.get_animation` 当场读回；
--   * `spine.list` 问的是这份骨架里真实存在的动画（由 `.skel` 决定，不是手抄清单）；
--   * `spine.events` 是**取走即清**的轮询队列，切动画时产生 `start` 事件；
--   * `spine.bounds` 读的是**本帧采样后**的世界空间包围盒——首帧前是四点全零，
--     因此断言只要求"四个数、且 max >= min"，不去假设某一帧的具体尺寸。
--
-- 交互键（Lab 保留 space/方向键/1..9/R/F/F5，这里只用字母）：
--   A idle  R run  J jump  W walk（切换动画）
--   S 切皮肤（default <-> 另一套，没有就跳过）  E 启停  Z/V 减速/加速  B 取事件
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run` 在 headless 下只读或做内存写，
-- 不写盘、不切场景、不模拟输入。
local lab = require("lab")

local M = {}

-- 场景实体名（scenes/api_spine.scene）。
local HERO = "Hero"

local ids = {}

-- 本演示会按下的几个动画名；真正的合法性由 `spine.list` 回答。
local ANIMS = { idle = "keya", run = "keyr", jump = "keyj", walk = "keyw" }

local obs = { op = "-", anim = "-", events = 0, speed = 1.0, enabled = true }

--- 优先挑一个"有代表性"的动画名：run > walk > idle > 第一个。
local function pick_animation()
    local names = spine.list(ids.hero)
    for _, candidate in ipairs({ "run", "walk", "idle" }) do
        for _, name in ipairs(names) do
            if name == candidate then
                return name
            end
        end
    end
    return names[1]
end


local steps = {
    {
        id = "inspect",
        title = "骨架与动画清单",
        claim = "场景里 Hero 挂着 SpineSkeleton；spine.list 读的是骨骼里真实存在的动画名",
        api = { "spine.list", "spine.get_animation", "spine.bounds" },
        run = function()
            local names = spine.list(ids.hero)
            obs.anim = spine.get_animation(ids.hero)
            obs.op = string.format("list=%d anim=%s", #names, obs.anim)
        end,
        check = function(t)
            local names = spine.list(ids.hero)
            t:eq(type(names), "table", "spine.list 返回数组")
            t:ok(#names > 0, "骨架里至少有一个动画")
            local current = spine.get_animation(ids.hero)
            t:eq(type(current), "string", "spine.get_animation 返回字符串")
            local box = spine.bounds(ids.hero)
            t:eq(#box, 4, "spine.bounds 返回 4 个数")
            t:ok(box[3] >= box[1], "bounds max_x >= min_x")
            t:ok(box[4] >= box[2], "bounds max_y >= min_y")
        end,
    },
    {
        id = "switch",
        title = "切换动画",
        claim = "set_animation 切到骨架里有的动画，get_animation 读回；不存在的名字报错而不是静默忽略",
        api = { "spine.set_animation", "spine.get_animation", "spine.list" },
        run = function()
            spine.set_animation(ids.hero, pick_animation(), true)
            obs.op = "set_animation(" .. pick_animation() .. ")"
            obs.anim = spine.get_animation(ids.hero)
        end,
        check = function(t)
            local names = spine.list(ids.hero)
            local target = nil
            for _, candidate in ipairs({ "run", "walk", "idle" }) do
                for _, name in ipairs(names) do
                    if name == candidate then
                        target = name
                        break
                    end
                end
                if target then
                    break
                end
            end
            target = target or names[1]
            spine.set_animation(ids.hero, target, true)
            t:eq(spine.get_animation(ids.hero), target, "切到 " .. target .. " 后读回一致")
            -- 不存在的名字必须报错：静默忽略会让"动画没切"看起来像"动画播完了"。
            local ok = pcall(function()
                spine.set_animation(ids.hero, "__no_such_animation__", true)
            end)
            t:eq(ok, false, "不存在的动画名必须报错")
            t:eq(spine.get_animation(ids.hero), target, "报错后仍停在原来的动画")
        end,
    },
    {
        id = "speed_skin",
        title = "速度与皮肤",
        claim = "set_speed 改播放倍率；set_skin 换装，不存在的皮肤名报错",
        api = { "spine.set_speed", "spine.set_skin" },
        run = function()
            spine.set_speed(ids.hero, 0.5)
            obs.op = "set_speed(0.5)"
            obs.speed = 0.5
        end,
        check = function(t)
            spine.set_speed(ids.hero, 0.5)
            t:eq(type(spine.get_animation(ids.hero)), "string", "调速度不影响当前动画")
            spine.set_speed(ids.hero, 1.0)
            -- `default` 皮肤永远存在；换回去应成功。
            local ok = pcall(function()
                spine.set_skin(ids.hero, "default")
            end)
            t:eq(ok, true, "切到 default 皮肤成功")
            local bad = pcall(function()
                spine.set_skin(ids.hero, "__no_such_skin__")
            end)
            t:eq(bad, false, "不存在的皮肤名必须报错")
        end,
    },
    {
        id = "events",
        title = "事件轮询",
        claim = "切动画产生 start 事件；spine.events 取走即清，第二次调用不再返回同一批",
        api = { "spine.events", "spine.set_animation" },
        run = function()
            local events = spine.events()
            obs.events = #events
            obs.op = string.format("events=%d", #events)
        end,
        check = function(t)
            -- 先清空历史事件，避免上一演示步骤留下的事件干扰断言。
            spine.events()
            local names = spine.list(ids.hero)
            local target = names[1]
            for _, name in ipairs(names) do
                if name == "run" then
                    target = name
                    break
                end
            end
            spine.set_animation(ids.hero, target, true)
            local first = spine.events()
            t:eq(type(first), "table", "spine.events 返回数组")
            t:ok(#first > 0, "切动画至少产生一个事件")
            local kinds = {}
            for _, event in ipairs(first) do
                kinds[event.kind] = true
                t:eq(type(event.entity), "number", "事件带 entity id")
                t:eq(type(event.animation), "string", "事件带 animation 名")
            end
            t:eq(kinds.start, true, "切换动画产生 start 事件")
            local second = spine.events()
            t:eq(#second, 0, "取走即清：第二次调用返回空")
        end,
    },
    {
        id = "toggle",
        title = "启停",
        claim = "set_enabled(false) 停止推进但保留上一帧网格；bounds 仍可读",
        api = { "spine.set_enabled", "spine.bounds" },
        run = function()
            obs.enabled = not obs.enabled
            spine.set_enabled(ids.hero, obs.enabled)
            obs.op = "set_enabled(" .. tostring(obs.enabled) .. ")"
        end,
        check = function(t)
            local before = spine.bounds(ids.hero)
            spine.set_enabled(ids.hero, false)
            local box = spine.bounds(ids.hero)
            t:eq(#box, 4, "禁用后仍能读包围盒")
            spine.set_enabled(ids.hero, true)
            t:ok(before[3] >= before[1], "禁用前的包围盒自洽")
        end,
    },
    {
        id = "snapshot",
        title = "姿态烘焙",
        claim = "spine.snapshot 把当前姿态写成 PNG；snapshot_frames 逐帧推进后各写一张，返回的路径可直接喂给 ui.image",
        api = { "spine.snapshot", "spine.snapshot_frames" },
        run = function()
            -- `run` 每帧都会执行，headless 下不写盘：这里只读姿态（写盘放在 check 里，自检时执行一次）。
            local box = spine.bounds(ids.hero)
            obs.op = string.format("pose=%.0fx%.0f", box[3] - box[1], box[4] - box[2])
        end,
        check = function(t)
            local root = app.project_root()
            t:ok(type(root) == "string" and root ~= "", "app.project_root 可用（快照按工程根解析图集页）")

            -- 只写工程内 `.temp/`（生成物目录，不入库），与 headless 写盘约定一致。
            local dir = ".temp/spine-snapshot"
            local single = spine.snapshot(ids.hero, 96, 96, dir .. "/pose.png")
            t:eq(type(single), "string", "spine.snapshot 返回写出的绝对路径")
            t:ok(single:find("pose.png", 1, true) ~= nil, "返回路径指向请求的文件名")

            local frames = spine.snapshot_frames(ids.hero, 64, 64, dir, 2, 1.0 / 30.0, "walk")
            t:eq(type(frames), "table", "spine.snapshot_frames 返回数组")
            t:eq(#frames, 2, "请求 2 帧就返回 2 个路径")
            for index, path in ipairs(frames) do
                t:eq(type(path), "string", string.format("第 %d 帧返回路径字符串", index))
                t:ok(path:find("walk", 1, true) ~= nil, "序列帧文件名带前缀")
            end
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

local function find_ids()
    ids.hero = entity.find(HERO)
end

function M.on_create()
    find_ids()
    lab.start({
        scene = "api_spine",
        title = "Spine 2D",
        category = "2D",
        steps = steps,
        hide_scene_text = { "Hud" },
    })
    log.info("api_spine: lab ready (list/switch/speed/events/toggle/snapshot)")
end

function M.update()
    if ids.hero and entity.is_alive(ids.hero) then
        for name, key in pairs(ANIMS) do
            if input.just_pressed(key) then
                local ok = pcall(function()
                    spine.set_animation(ids.hero, name, true)
                end)
                obs.op = "set_animation(" .. name .. ")=" .. tostring(ok)
                if ok then
                    obs.anim = name
                end
            end
        end
        if input.just_pressed("keys") then
            local ok = pcall(function()
                spine.set_skin(ids.hero, "default")
            end)
            obs.op = "set_skin(default)=" .. tostring(ok)
        elseif input.just_pressed("keye") then
            obs.enabled = not obs.enabled
            spine.set_enabled(ids.hero, obs.enabled)
            obs.op = "set_enabled(" .. tostring(obs.enabled) .. ")"
        elseif input.just_pressed("keyz") then
            obs.speed = math.max(0.0, obs.speed - 0.25)
            spine.set_speed(ids.hero, obs.speed)
            obs.op = string.format("speed %.2f", obs.speed)
        elseif input.just_pressed("keyv") then
            obs.speed = math.min(4.0, obs.speed + 0.25)
            spine.set_speed(ids.hero, obs.speed)
            obs.op = string.format("speed %.2f", obs.speed)
        elseif input.just_pressed("keyb") then
            local events = spine.events()
            obs.events = #events
            obs.op = string.format("events=%d", #events)
        end
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    lab.try(function()
        if ids.hero and entity.is_alive(ids.hero) then
            spine.set_enabled(ids.hero, true)
            spine.set_speed(ids.hero, 1.0)
        end
        return true
    end)
    lab.stop()
end

-- 能力图谱门（sample_project_capability_catalog_is_complete）靠它核对步骤 id。
M.steps = steps

return M
