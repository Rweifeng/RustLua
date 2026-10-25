-- api_3d：3D 渲染（PBR / IBL / 骨骼动画）与 3D 物理的分步 Lab。
--
-- 这个场景想证明的事，每一条都能在舞台里读出来：
--   ① 场景由 `StudioEnvironment`（IBL 环境贴图）+ 方向光 + PBR 网格组成，环境本身就是一份实体；
--   ② `transform3d.get` 是**局部值**、`transform3d.world` 是**世界值**：`OrbitTriangle` 是
--      `Ground` 的子实体，父移动时子的局部值一字不变，世界值跟着父走（组织关系下父链组合
--      由 `world` 现算，所以开场两者相等、父一动就分叉）；
--   ③ 骨骼剪辑名是运行期事实：`animation3d.list` 读清单，`animation3d.set` 切换（切换即重播），
--      `set_paused` / `paused` / `time` 是三个独立读数；
--   ④ 3D 物理：速度是唯一入口（`set_velocity` 后 `get_velocity` 立刻读回），CCD 开关存在
--      刚体上，`raycast` / `overlap` / `events` 是三种不同粒度的查询；
--   ⑤ `entity.spawn` 造出来的裸实体没有变换，`transform3d.attach` 给它补一份之后才可读写。
--
-- 交互键（框架保留的 SPACE / 方向键 / R / F / F5 / 数字键一律不碰）：
--   A / D 逐条切骨骼剪辑   P 暂停/继续   S 轻推子实体
--   V 把 CCD 探针射向地面   C 开关 CCD   B 开关 3D 碰撞体线框
--
-- 自检约定：`enter` 会把场地（父/子/球/探针/线框）全部复位，因此读者先乱按再按 F5 仍全绿。
-- 物理步进由宿主每帧模拟，自检里只读回 `enter` 写入的值（或带容差的形状），不去赌模拟结果。
local lab = require("lab")

local M = {}

-- 实体名（与 `scenes/api_3d.scene` 一致）。
local GROUND = "Ground"
local CHILD = "OrbitTriangle"
local WARRIOR = "SkeletonWarrior"
local BALL = "BounceSphere"
local CAPSULE = "CapsuleSensor"
local PROBE = "CcdProbe"
local ENV = "StudioEnvironment"
local SUN = "Sun"
local ATTACH_NAME = "AttachProbe3d"

-- 演示常量。
local BALL_HOME = { x = 0.0, y = 2.2, z = 0.0 }
local PROBE_HOME = { x = 0.0, y = 1.4, z = 0.0 }
local CHILD_HOME = { x = 1.6, y = 0.05, z = 0.0 }
local PROBE_SHOT = 22.0 -- V 键：高速撞地面，用来对照 CCD 开/关

-- 运行期状态（自检与交互共用；`enter` 负责把它们复位）。
local elapsed = 0
local clips = {}
local clip_index = 1
local probe_entity = nil
local probe_attached = false
local debug_on = false
local env_id = nil
local sun_id = nil
local warrior_id = nil

--- `entity.find` + 存活校验：缺实体时返回 nil，调用方据此跳过而不是让 API 报错。
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

--- 运行期实体只在模块活着时存在；重复进入步骤时复用同一个，避免每次 enter 都造一个。
local function ensure_probe()
    if probe_entity and entity.is_alive(probe_entity) then
        return probe_entity
    end
    probe_entity = entity.spawn(ATTACH_NAME)
    return probe_entity
end

--- 剪辑清单来自运行期（`animation3d.list`），不靠写死的名字：
--- 场景组件里的 `clip=Idle` 只负责"开局播哪一条"。
local function load_clips()
    clips = {}
    clip_index = 1
    local warrior = live(WARRIOR)
    if not warrior then
        return
    end
    local listed = lab.try(animation3d.list, warrior)
    if type(listed) == "table" then
        clips = listed
    end
end

--- 场地复位：把本演示会改的 3D 状态全部写回确定值。
--- 自检对每一步都先走 `enter`，因此这里必须覆盖"读者刚才按过的所有键"。
local function reset_world()
    elapsed = 0
    debug_on = false

    local ground = live(GROUND)
    if ground then
        -- 父实体回到单位变换：子的局部值此时恰好等于世界值，便于对照。
        transform3d.set(ground, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1)
    end
    local child = live(CHILD)
    if child then
        transform3d.set(child, CHILD_HOME.x, CHILD_HOME.y, CHILD_HOME.z, 0, 0, 0, 1, 1, 1, 1)
    end
    local ball = live(BALL)
    if ball then
        physics3d.set_position(ball, BALL_HOME.x, BALL_HOME.y, BALL_HOME.z)
        physics3d.set_velocity(ball, 0, 0, 0)
    end
    local probe = live(PROBE)
    if probe then
        physics3d.set_position(probe, PROBE_HOME.x, PROBE_HOME.y, PROBE_HOME.z)
        physics3d.set_velocity(probe, 0, 0, 0)
        physics3d.set_ccd(probe, false)
    end
    -- 调试线框是"开一次即持续生效"的登记请求：复位时显式关掉，B 键才有确定起点。
    lab.try(physics3d.debug_draw, false)
end

local steps = {
    {
        id = "environment_ibl",
        title = "PBR + IBL",
        claim = "StudioEnvironment 的 IBL、太阳方向光与 PBR 网格共同构成 3D 视图；A/D 切骨骼剪辑",
        api = { "entity.find", "entity.is_alive" },
        code = "entity.find('StudioEnvironment'); entity.is_alive(warrior)",
        enter = function()
            reset_world()
            env_id = live(ENV)
            sun_id = live(SUN)
            warrior_id = live(WARRIOR)
        end,
        run = function()
            lab.note("StudioEnvironment / Sun / SkeletonWarrior 都在场景里，PBR 网格共用同一份光照")
        end,
        read = function()
            return {
                environment = env_id or "-",
                sun = sun_id or "-",
                warrior = warrior_id or "-",
                clips = #clips,
            }
        end,
        check = function(t)
            t:ok(env_id ~= nil and entity.is_alive(env_id), "StudioEnvironment exists")
            t:ok(sun_id ~= nil and entity.is_alive(sun_id), "Sun light exists")
            t:ok(warrior_id ~= nil and entity.is_alive(warrior_id), "skinned warrior exists")
        end,
    },
    {
        id = "local_vs_world",
        title = "Local != world",
        claim = "transform3d.get 是局部值、world 是世界值：父实体滑动时子的局部值不变、世界值跟着父走",
        api = { "transform3d.get", "transform3d.world", "transform3d.set" },
        code = "transform3d.get(child); transform3d.world(child); transform3d.set(ground, ...)",
        enter = function()
            reset_world()
        end,
        run = function(dt)
            elapsed = elapsed + dt
            local ground = live(GROUND)
            local child = live(CHILD)
            if ground then
                -- 整份写入位置（旋转/缩放保持），父实体左右缓慢滑动。
                transform3d.set(ground, math.sin(elapsed * 0.8) * 0.8, 0, 0, 0, 0, 0, 1, 1, 1, 1)
            end
            if child then
                local local_t = transform3d.get(child)
                local world_t = transform3d.world(child)
                lab.note(
                    string.format(
                        "child local=(%.2f,%.2f,%.2f) world=(%.2f,%.2f,%.2f)",
                        local_t.x,
                        local_t.y,
                        local_t.z,
                        world_t.x,
                        world_t.y,
                        world_t.z
                    )
                )
            end
        end,
        read = function()
            local child = live(CHILD)
            local ground = live(GROUND)
            if not child then
                return { child = "missing" }
            end
            local local_t = transform3d.get(child)
            local world_t = transform3d.world(child)
            local ground_t = ground and transform3d.get(ground) or { x = 0, y = 0, z = 0 }
            return {
                local_x = string.format("%.2f", local_t.x),
                world_x = string.format("%.2f", world_t.x),
                parent_x = string.format("%.2f", ground_t.x),
            }
        end,
        check = function(t)
            local child = live(CHILD)
            local ground = live(GROUND)
            if not child or not ground then
                t:ok(false, "OrbitTriangle and Ground are required")
                return
            end
            local local_t = transform3d.get(child)
            local world_t = transform3d.world(child)
            local ground_t = transform3d.get(ground)
            t:near(local_t.x, CHILD_HOME.x, 1e-3, "child local x is untouched")
            t:near(world_t.x - local_t.x, ground_t.x, 1e-3, "world x = local x + parent x")
            t:near(world_t.y - local_t.y, ground_t.y, 1e-3, "world y = local y + parent y")
        end,
    },
    {
        id = "attach_spawn",
        title = "Spawn + attach",
        claim = "entity.spawn 只给 id；transform3d.attach 补上 Transform3D 之后 set/move 立刻可读回",
        api = {
            "entity.spawn",
            "entity.find",
            "entity.is_alive",
            "transform3d.attach",
            "transform3d.set",
            "transform3d.move",
            "transform3d.get",
        },
        code = "e = entity.spawn('AttachProbe3d'); transform3d.attach(e); transform3d.set(e, ...)",
        enter = function()
            reset_world()
            local probe = ensure_probe()
            if probe and entity.is_alive(probe) then
                -- attach 会无条件插入一份默认 Transform3D（对本模块自己的探针来说就是复位）。
                probe_attached = transform3d.attach(probe)
                transform3d.set(probe, 0, 1.2, -1.2, 0, 0, 0, 1, 1, 1, 1)
                -- 增量移动：只在 set 的基础上抬 0.1，读回值因此是可预期的 1.3。
                transform3d.move(probe, 0, 0.1, 0)
            end
        end,
        run = function()
            lab.note("AttachProbe3d: spawn -> attach -> set -> move，全部是运行期实体")
        end,
        read = function()
            if not probe_entity or not entity.is_alive(probe_entity) then
                return { probe = "missing" }
            end
            local value = transform3d.get(probe_entity)
            return {
                probe = probe_entity,
                attached = tostring(probe_attached),
                x = string.format("%.2f", value.x),
                y = string.format("%.2f", value.y),
                z = string.format("%.2f", value.z),
            }
        end,
        check = function(t)
            t:ok(probe_entity ~= nil and entity.is_alive(probe_entity), "spawned probe is alive")
            t:eq(entity.find(ATTACH_NAME), probe_entity, "spawned entity is findable by name")
            t:ok(probe_attached == true, "transform3d.attach returned true")
            if not probe_entity or not entity.is_alive(probe_entity) then
                return
            end
            local value = transform3d.get(probe_entity)
            t:near(value.y, 1.3, 1e-4, "set + move are readable back")
        end,
    },
    {
        id = "skeleton_clips",
        title = "Clip list",
        claim = "骨骼剪辑清单是运行期事实：list 读清单、set 切换即重播、get/time 读当前状态",
        api = { "animation3d.list", "animation3d.get", "animation3d.set", "animation3d.time" },
        code = "clips = animation3d.list(w); animation3d.set(w, clips[1]); animation3d.get(w)",
        enter = function()
            reset_world()
            load_clips()
            local warrior = live(WARRIOR)
            if not warrior then
                return
            end
            animation3d.set_paused(warrior, false)
            -- 先切到另一条再切回来：即使当前就是 clips[1]，时间也会归零。
            if #clips > 1 then
                animation3d.set(warrior, clips[2])
            end
            if #clips > 0 then
                animation3d.set(warrior, clips[1])
            end
        end,
        run = function()
            local warrior = live(WARRIOR)
            if warrior then
                lab.note(
                    string.format(
                        "clip=%s (%d/%d) t=%.2fs",
                        tostring(animation3d.get(warrior)),
                        clip_index,
                        #clips,
                        animation3d.time(warrior)
                    )
                )
            end
        end,
        read = function()
            local warrior = live(WARRIOR)
            if not warrior then
                return { warrior = "missing" }
            end
            return {
                clips = #clips,
                clip = tostring(animation3d.get(warrior)),
                time = string.format("%.2f", animation3d.time(warrior)),
                first = #clips > 0 and clips[1] or "-",
            }
        end,
        check = function(t)
            local warrior = live(WARRIOR)
            if not warrior then
                t:ok(false, "SkeletonWarrior is required")
                return
            end
            t:ok(#clips > 0, "animation3d.list returned clips")
            t:eq(animation3d.get(warrior), clips[1], "current clip is the first listed clip")
            if #clips > 1 then
                t:near(animation3d.time(warrior), 0, 0.05, "switching a clip restarts its clock")
            else
                t:ok(animation3d.time(warrior) >= 0, "clip time is readable")
            end
        end,
    },
    {
        id = "clip_pause",
        title = "Pause clip",
        claim = "set_paused 冻结骨骼时间，paused() 直接读回开关；P 键切换后 time 不再前进",
        api = { "animation3d.paused", "animation3d.set_paused", "animation3d.time", "animation3d.set" },
        code = "animation3d.set_paused(w, true); animation3d.paused(w); animation3d.time(w)",
        enter = function()
            reset_world()
            load_clips()
            local warrior = live(WARRIOR)
            if not warrior then
                return
            end
            if #clips > 1 then
                animation3d.set(warrior, clips[2])
            end
            if #clips > 0 then
                animation3d.set(warrior, clips[1])
            end
            animation3d.set_paused(warrior, true)
        end,
        run = function()
            local warrior = live(WARRIOR)
            if warrior then
                lab.note(
                    string.format(
                        "paused=%s time=%.2fs",
                        tostring(animation3d.paused(warrior)),
                        animation3d.time(warrior)
                    )
                )
            end
        end,
        read = function()
            local warrior = live(WARRIOR)
            if not warrior then
                return { warrior = "missing" }
            end
            return {
                clip = tostring(animation3d.get(warrior)),
                paused = tostring(animation3d.paused(warrior)),
                time = string.format("%.2f", animation3d.time(warrior)),
            }
        end,
        check = function(t)
            local warrior = live(WARRIOR)
            if not warrior then
                t:ok(false, "SkeletonWarrior is required")
                return
            end
            t:eq(animation3d.paused(warrior), true, "paused() reads back the flag")
            t:ok(animation3d.time(warrior) >= 0, "time is readable while paused")
        end,
    },
    {
        id = "ball_velocity",
        title = "Ball velocity",
        claim = "速度是动态刚体的唯一入口：set_velocity 之后 get_velocity 立刻读回同一组数字",
        api = { "physics3d.get_velocity", "physics3d.set_velocity", "physics3d.set_position", "transform3d.get" },
        code = "physics3d.set_position(ball, 0, 2.2, 0); physics3d.set_velocity(ball, 0, 4, 0)",
        enter = function()
            reset_world()
            local ball = live(BALL)
            if ball then
                -- 位置由 set_position 同步到 ECS；速度交给物理每帧积分（自检只读回写入值）。
                physics3d.set_velocity(ball, 0, 4.0, 0)
            end
        end,
        run = function()
            local ball = live(BALL)
            if ball then
                local velocity = lab.try(physics3d.get_velocity, ball)
                local position = lab.try(transform3d.get, ball)
                if velocity and position then
                    lab.note(
                        string.format(
                            "ball pos=(%.2f,%.2f,%.2f) v=(%.2f,%.2f,%.2f)",
                            position.x,
                            position.y,
                            position.z,
                            velocity.x,
                            velocity.y,
                            velocity.z
                        )
                    )
                end
            end
        end,
        read = function()
            local ball = live(BALL)
            if not ball then
                return { ball = "missing" }
            end
            local velocity = physics3d.get_velocity(ball)
            local position = transform3d.get(ball)
            return {
                x = string.format("%.2f", position.x),
                y = string.format("%.2f", position.y),
                vy = string.format("%.2f", velocity.y),
            }
        end,
        check = function(t)
            local ball = live(BALL)
            if not ball then
                t:ok(false, "BounceSphere is required")
                return
            end
            local velocity = physics3d.get_velocity(ball)
            local position = transform3d.get(ball)
            t:near(velocity.y, 4.0, 0.05, "get_velocity reads the value just written")
            t:near(position.y, BALL_HOME.y, 0.01, "set_position also syncs the ECS transform")
        end,
    },
    {
        id = "probe_ccd",
        title = "CCD flag",
        claim = "CCD 存在刚体上：set_ccd 之后 get_ccd 读回真实开关；C 键对照，V 键高速撞地",
        api = { "physics3d.get_ccd", "physics3d.set_ccd", "physics3d.set_position", "physics3d.set_velocity" },
        code = "physics3d.set_ccd(probe, false); physics3d.get_ccd(probe)",
        enter = function()
            reset_world()
            local probe = live(PROBE)
            if probe then
                physics3d.set_position(probe, PROBE_HOME.x, PROBE_HOME.y, PROBE_HOME.z)
                physics3d.set_velocity(probe, 0, 0, 0)
                physics3d.set_ccd(probe, false)
            end
        end,
        run = function()
            local probe = live(PROBE)
            if probe then
                lab.note("CcdProbe ccd=" .. tostring(physics3d.get_ccd(probe)))
            end
        end,
        read = function()
            local probe = live(PROBE)
            if not probe then
                return { probe = "missing" }
            end
            return {
                ccd = tostring(physics3d.get_ccd(probe)),
                y = string.format("%.2f", transform3d.get(probe).y),
            }
        end,
        check = function(t)
            local probe = live(PROBE)
            if not probe then
                t:ok(false, "CcdProbe is required")
                return
            end
            t:eq(physics3d.get_ccd(probe), false, "get_ccd reads back false after set_ccd(false)")
        end,
    },
    {
        id = "physics_queries",
        title = "Ray / overlap",
        claim = "raycast 命中最近的碰撞体；overlap 问附近有谁；events 取走即清空；B 开关线框",
        api = { "physics3d.raycast", "physics3d.overlap", "physics3d.events", "physics3d.debug_draw" },
        code = "physics3d.raycast(x+1,y,z,0,-1,0,12); physics3d.overlap(x,y,z,3.2)",
        enter = function()
            reset_world()
            local ball = live(BALL)
            if ball then
                -- 球心在高处：向下的射线必定先撞到地面。
                physics3d.set_position(ball, BALL_HOME.x, BALL_HOME.y, BALL_HOME.z)
                physics3d.set_velocity(ball, 0, 0, 0)
            end
        end,
        run = function()
            local ball = live(BALL)
            local hit = nil
            local near = 0
            if ball then
                local p = transform3d.get(ball)
                -- 射线起点偏 +1（球半径 0.4）：起点落在球自己的碰撞体之外，否则会先命中球本身；
                -- 同一 x 上的 CCD 探针也在半径之外，垂直向下只会打到 Ground。
                hit = lab.try(physics3d.raycast, p.x + 1.0, p.y, p.z, 0, -1, 0, 12)
                lab.try(physics3d.get_velocity, ball)
                local capsule = live(CAPSULE)
                if capsule then
                    local c = transform3d.get(capsule)
                    local ids = lab.try(physics3d.overlap, c.x, c.y, c.z, 3.2)
                    near = type(ids) == "table" and #ids or 0
                end
            end
            -- events 与 debug_draw 是宿主帧依赖/取走即清的接口：用 lab.try 只判形状。
            local events = lab.try(physics3d.events)
            lab.try(physics3d.debug_draw, debug_on)
            lab.note(
                string.format(
                    "ray=%s overlap=%d events=%d wireframe=%s",
                    hit and "hit" or "none",
                    near,
                    type(events) == "table" and #events or 0,
                    tostring(debug_on)
                )
            )
        end,
        read = function()
            local ball = live(BALL)
            if not ball then
                return { ball = "missing" }
            end
            local p = transform3d.get(ball)
            local hit = lab.try(physics3d.raycast, p.x + 1.0, p.y, p.z, 0, -1, 0, 12)
            local capsule = live(CAPSULE)
            local ids = capsule and lab.try(physics3d.overlap, p.x, p.y, p.z, 3.2) or {}
            return {
                ray_hit = hit or "none",
                ground = live(GROUND) or "-",
                overlap = type(ids) == "table" and #ids or 0,
                wireframe = tostring(debug_on),
            }
        end,
        check = function(t)
            local ball = live(BALL)
            local ground = live(GROUND)
            if not ball or not ground then
                t:ok(false, "BounceSphere and Ground are required")
                return
            end
            local p = transform3d.get(ball)
            local hit = lab.try(physics3d.raycast, p.x + 1.0, p.y, p.z, 0, -1, 0, 12)
            t:eq(hit, ground, "downward raycast hits Ground")
            local events = lab.try(physics3d.events)
            t:ok(type(events) == "table", "events() returns a table (drained per frame)")
            local capsule = live(CAPSULE)
            local ids = capsule and lab.try(physics3d.overlap, p.x, p.y, p.z, 3.2) or nil
            t:ok(type(ids) == "table" and #ids >= 1, "overlap finds bodies around the ball")
            t:eq(lab.try(physics3d.debug_draw, false), false, "debug_draw returns the flag it registered")
        end,
    },
}

function M.on_create()
    load_clips()
    lab.start({
        scene = "api_3d",
        title = "3D Rendering",
        category = "3D",
        steps = steps,
        hide_scene_text = {},
    })
    log.info(string.format("demo_3d: animation3d.list returned %d clips", #clips))
end

function M.update()
    -- 宿主每帧推进 `animation3d_advance`；这里只读回时钟，顺便演示"球落地反弹"这类
    -- 速度控制（没有施加冲量的 API，速度就是唯一入口）。
    local warrior = live(WARRIOR)
    if warrior and #clips > 0 then
        lab.try(animation3d.time, warrior)
    end
    local ball = live(BALL)
    if ball then
        local velocity = lab.try(physics3d.get_velocity, ball)
        local position = lab.try(transform3d.get, ball)
        if velocity and position and position.y <= 0.45 and velocity.y < 0 then
            physics3d.set_velocity(ball, velocity.x, 3.2, velocity.z)
        end
    end

    -- 字母键交互：框架保留键（SPACE/方向键/R/F/F5/数字）不占用。
    if warrior and #clips > 0 then
        if input.just_pressed("d") then
            clip_index = clip_index % #clips + 1
            animation3d.set(warrior, clips[clip_index])
        elseif input.just_pressed("a") then
            clip_index = ((clip_index - 2) % #clips) + 1
            animation3d.set(warrior, clips[clip_index])
        end
        if input.just_pressed("p") then
            animation3d.set_paused(warrior, not animation3d.paused(warrior))
        end
    end
    if input.just_pressed("s") then
        local child = live(CHILD)
        if child then
            transform3d.move(child, 0.2, 0, 0)
        end
    end
    if input.just_pressed("c") then
        local probe = live(PROBE)
        if probe then
            physics3d.set_ccd(probe, not physics3d.get_ccd(probe))
        end
    end
    if input.just_pressed("v") then
        local probe = live(PROBE)
        if probe then
            physics3d.set_position(probe, PROBE_HOME.x, PROBE_HOME.y, PROBE_HOME.z)
            physics3d.set_velocity(probe, 0, -PROBE_SHOT, 0)
        end
    end
    if input.just_pressed("b") then
        debug_on = not debug_on
        lab.try(physics3d.debug_draw, debug_on)
    end

    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 本模块自己创建的实体与开关要收干净：运行期 spawn 的探针、3D 线框、场地改动。
    if probe_entity and entity.is_alive(probe_entity) then
        entity.destroy(probe_entity)
        probe_entity = nil
    end
    reset_world()
    lab.stop()
end

return M
