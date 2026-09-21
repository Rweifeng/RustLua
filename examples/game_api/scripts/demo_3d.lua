-- api_3d：演示 3D 变换（transform3d）、骨骼动画（animation3d）与 3D 物理（physics3d）。
--
-- 这个场景想证明的三件事，每件都能在画面上看见：
--
-- ① **局部 ≠ 世界**（`transform3d.get` vs `transform3d.world`）。
--    场景里的 `OrbitTriangle`（实体 4）是 `Ground`（实体 3）的**子实体**，它的 `Transform`
--    是**相对父的局部值**。父一开始是单位变换（位置 0,0,0、四元数 0,0,0,1、缩放 1,1,1），
--    所以开局局部值恰好等于世界值；一旦按住 ↑/↓ 移动父，子的局部值一个字都不变，而它的
--    **世界**位置立刻跟着父走。状态行把两个值并排打出来，这就是 `transform3d.world` 存在的
--    理由（docs/54 §8-⑦）。
--    ⚠️ 为什么父的初始变换必须是单位变换：本仓库有三条自动化门会把
--    `examples/game_api/scenes/*.scene` 里**每个**带 Transform 的实体按 v2 语义（局部=世界）
--    预先算一次世界位置并逐实体比对
--    （`spatial_transform::migration_preserves_world_positions_on_project_scenes`、
--    `spatial_transform::unified_matrix_matches_legacy_model_matrix_on_project_scenes`、
--    `typed_scene::compiles_every_project_scene_and_matches_serialized_extraction`）。
--    给父写任何非零偏移，都会让子实体的世界位置在"派生的父子解释"下对不上而判红**别人的**门，
--    所以这里把"父有偏移"这件事放到**运行期**用 `transform3d.move` 制造：演示效果一样，门照过。
--    写 `meta spatial local` 开空间继承也不行——那会改变子实体的初始世界位置（从局部值变成
--    父链组合值），与上面同一条门冲突。因此本场景按 v2 默认（组织关系）解释 parent，
--    运行期的世界值由 `transform3d.world` 现算（`hierarchy::world_transform_3d_lazy`：
--    没有 GlobalTransform 时沿父链现推一次，不依赖"宿主本帧有没有传播过"）。
--
-- ② **骨骼动画的剪辑是运行期事实**（`animation3d.list`）。
--    剪辑名不靠猜：`on_create` 里 `animation3d.list(warrior)` 把全部剪辑读出来，状态行给出条数，
--    A/D 逐条 `animation3d.set` 切换（**切换即重播**，时间归零），P 键 `animation3d.set_paused`
--    暂停/继续，`animation3d.get` / `animation3d.paused` / `animation3d.time` 三项读数都在状态行。
--    场景组件写的是 `clip=Idle`——那个名字是用 GLB 容器的 JSON 块（`animations[].name`）核对过的：
--    `assets/models/kaykit-warrior.glb` 里有 95 个剪辑（`Idle` / `Idle_Combat` / `Running_A` /
--    `Walking_A` / `1H_Melee_Attack_Chop` …），而 `m8-skin-quad.glb` 只有一个剪辑（`Bend`，
--    见 `script.rs::lua_3d_animation_queries_and_switches_clips`），不适合做"逐条切换"演示。
--
-- ③ **3D 物理查询与刚体控制**（`physics3d.*`）。
--    BounceSphere 是动态刚体：自由落体、撞地反弹（`physics3d.get_velocity` 读到负速度 →
--    `physics3d.set_velocity` 反向并衰减）、R 用 `physics3d.set_position` 把它传送回高处、
--    F 把它打出去。右下角的胶囊声明了 `sensor=true`，`physics3d.overlap` 用它做"附近有哪些
--    东西"的经典检测；`physics3d.raycast` 从球心往下打一条射线做**地面检测**，状态行显示
--    **命中实体的名字**（不是裸 id）。C 键用 `physics3d.set_ccd` / `physics3d.get_ccd` 开关
--    CcdProbe 的连续碰撞检测：它是高速小球，**不打开 CCD 时一步就能整段跨过薄地面（隧穿过去），
--    打开后会被挡在表面**；场景串里写了 `;ccd=true` 让开局就是开着的，C 键用来对照两边。
--    V 键把它以 22 m/s 射向地面。`physics3d.events()` 取走**本帧**的碰撞事件
--    （`{a,b,started}`，取走即清空），取到就累计，并把最近一对实体写进状态行。
--
-- 按键分工见下方 api.hint。所有 3D 调用都先 `entity.find("<名字>")` 确认实体存在再调用：
-- `animation3d.*` / `physics3d.*` 在实体没有对应组件时是**报 Lua 错误**（不是返回 nil），
-- headless 步进门（`sample_project_runs_every_scene_headless`）会把这类错误直接判红。
local api = require("api_demo")

local M = {}

-- 实体名（与 `scenes/api_3d.scene` 一致）。
local GROUND = "Ground"
local CHILD = "OrbitTriangle"
local WARRIOR = "SkeletonWarrior"
local BALL = "BounceSphere"
local CAPSULE = "CapsuleSensor"
local PROBE = "CcdProbe"

-- 演示常量。
local BALL_RADIUS = 0.4
local BALL_HOME = { x = 0.0, y = 2.2, z = 0.0 }
local PROBE_HOME = { x = 0.0, y = 1.4, z = 0.0 }
local BOUNCE_FLOOR = 3.2 -- 反弹时至少给这么大的向上速度，球不会"躺"在地上
local PROBE_SHOT = 22.0 -- V 键：高速撞地面，用来对照 CCD 开/关
local MAX_STEP = 0.1 -- 单帧最多推进多少秒：掉帧时不让位移看起来像瞬移

-- 运行期状态。
local elapsed = 0
local spin = true -- 子实体是否绕 Y 轴自转（旋转值每次都由 transform3d.set 整份写入）
local child_slide = 0 -- 子实体的局部 X 目标（←/→ 改它）
local shot_speed = 4.0 -- F 键打球的水平速度（每按一次反向并加速，带上限）
local clips = {} -- `animation3d.list` 的运行期结果
local clip_index = 0
local events_seen = 0 -- 累计看到的碰撞事件数
local last_event = "(none)" -- 最近一对碰撞实体
local last_hit = "(none)" -- 最近一次射线命中的实体名
local attach_demo = "attach3d: pending"
local debug_draw_on = false

--- `entity.find` + 存活校验：缺实体时返回 nil（调用方据此跳过，而不是让 API 报错）。
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

--- 物理查询返回的是 id，状态行想显示名字（id 可能已经失效，因此包一层）。
local function name_of(id)
    if not id then
        return "nil"
    end
    local ok, value = pcall(entity.name, id)
    if ok and value then
        return tostring(value)
    end
    return "#" .. tostring(id)
end

--- 把 id 数组拼成一行名字（`physics3d.overlap` 的经典用法：先知道"附近有谁"）。
local function names_of(ids)
    if not ids or #ids == 0 then
        return "(none)"
    end
    local parts = {}
    for index, id in ipairs(ids) do
        if index > 4 then
            parts[#parts + 1] = "+" .. tostring(#ids - 4)
            break
        end
        parts[#parts + 1] = name_of(id)
    end
    return table.concat(parts, ",")
end

--- 子实体与父实体的"局部 vs 世界"对照——本演示的核心读数。
--  每处调用都写成 pcall(function() ... end) 包住**直接调用**：既让"调用确实发生过"（这个工程
--  的覆盖率门按 `模块.函数(` 的文本判定，传函数引用等于没演示），又不让组件缺失时炸掉整帧。
local function hierarchy_text()
    local parent = live(GROUND)
    local child = live(CHILD)
    if not child then
        return "hierarchy: OrbitTriangle missing"
    end
    local local_t = nil
    local world_t = nil
    local ok_local, err_local = pcall(function()
        local_t = transform3d.get(child)
    end)
    local ok_world, err_world = pcall(function()
        world_t = transform3d.world(child)
    end)
    if not (ok_local and ok_world) or not local_t or not world_t then
        return "hierarchy: " .. tostring(err_local or err_world)
    end
    local parent_text = "parent=?"
    if parent then
        local parent_t = nil
        if pcall(function()
            parent_t = transform3d.get(parent)
        end) and parent_t then
            parent_text =
                string.format("parent=(%.2f,%.2f,%.2f)", parent_t.x, parent_t.y, parent_t.z)
        end
    end
    return string.format(
        "%s child_local=(%.2f,%.2f,%.2f) child_world=(%.2f,%.2f,%.2f) q=(%.2f,%.2f,%.2f,%.2f)",
        parent_text,
        local_t.x,
        local_t.y,
        local_t.z,
        world_t.x,
        world_t.y,
        world_t.z,
        local_t.rx,
        local_t.ry,
        local_t.rz,
        local_t.rw
    )
end

--- `animation3d` 的三项读回：当前剪辑（`get`）、暂停标志（`paused`）、播放时刻（`time`）。
local function animation_text()
    local warrior = live(WARRIOR)
    if not warrior then
        return "anim3d: no SkeletonWarrior"
    end
    local clip = nil
    local paused = nil
    local clock = nil
    local ok_clip, err_clip = pcall(function()
        clip = animation3d.get(warrior)
    end)
    local ok_paused = pcall(function()
        paused = animation3d.paused(warrior)
    end)
    local ok_time = pcall(function()
        clock = animation3d.time(warrior)
    end)
    if not (ok_clip and ok_paused and ok_time) then
        return "anim3d: " .. tostring(err_clip or "entity has no animation3d")
    end
    return string.format(
        "anim3d clip=%s(%d/%d) paused=%s t=%.2fs first=%s",
        tostring(clip),
        clip_index,
        #clips,
        tostring(paused),
        clock,
        #clips > 0 and clips[1] or "(empty)"
    )
end

--- `physics3d` 的查询：速度、向下的射线、胶囊周围的重叠、本帧碰撞事件、CCD 开关。
local function physics_text()
    local ball = live(BALL)
    local ball_text = "ball=(missing)"
    local position = nil
    if ball then
        local velocity = nil
        pcall(function()
            velocity = physics3d.get_velocity(ball)
        end)
        pcall(function()
            position = transform3d.get(ball)
        end)
        if velocity and position then
            ball_text = string.format(
                "ball pos=(%.2f,%.2f,%.2f) v=(%.2f,%.2f,%.2f)",
                position.x,
                position.y,
                position.z,
                velocity.x,
                velocity.y,
                velocity.z
            )
        end
    end

    -- 地面检测 / 向前打一条射线：命中实体名比裸 id 有用得多
    --（`raycast` 只给 id，`entity.name` 才把 id 变成"脚下的 Ground"）。
    local ray_text = "ray=(no origin)"
    if position then
        -- ① 向下：脚下的地面检测（场景里 Ground 的顶面在 y=0.1，球心 0.5 处必定命中）。
        -- ② 向 -Z = 相机方向"向前"打一条，并且**带分组过滤**：位域是 u8，
        --    layer/mask 越界或非整数会报 Lua 错误（这里给的是合法值）。
        --    这条通常打空（前方没有碰撞体），正好对照"命中返回 id / 打空返回 nil"。
        local down = nil
        local forward = nil
        local ok_down, err_down = pcall(function()
            down = physics3d.raycast(position.x, position.y, position.z, 0, -1, 0, 12)
        end)
        pcall(function()
            forward =
                physics3d.raycast(position.x, position.y, position.z, 0, 0, -1, 12, {
                    layer = 1,
                    mask = 0xFF,
                })
        end)
        last_hit = name_of(down)
        if not ok_down then
            last_hit = "error:" .. tostring(err_down)
        end
        ray_text = string.format("ray_down=%s ray_fwd=%s", name_of(down), name_of(forward))
    end

    -- 重叠查询：以胶囊（sensor）为中心问"附近有哪些碰撞体"。
    local capsule = live(CAPSULE)
    local near_text = "(no capsule)"
    if capsule then
        local capsule_t = nil
        pcall(function()
            capsule_t = transform3d.get(capsule)
        end)
        if capsule_t then
            local ids = nil
            pcall(function()
                ids = physics3d.overlap(capsule_t.x, capsule_t.y, capsule_t.z, 3.2)
            end)
            near_text = names_of(ids)
        end
    end

    -- 本帧碰撞事件：取走即清空（与 animation.events / physics.events 同一套轮询用法）。
    local started = 0
    for _, event in ipairs(physics3d.events()) do
        events_seen = events_seen + 1
        if event.started then
            started = started + 1
        end
        last_event = name_of(event.a) .. (event.started and "<>" or "||") .. name_of(event.b)
    end

    local ccd_text = "?"
    local probe = live(PROBE)
    if probe then
        local ccd = nil
        if
            pcall(function()
                ccd = physics3d.get_ccd(probe)
            end)
        then
            ccd_text = tostring(ccd)
        end
    end

    return string.format(
        "%s %s near=[%s] ccd=%s events+%d/%d last=%s",
        ball_text,
        ray_text,
        near_text,
        ccd_text,
        started,
        events_seen,
        last_event
    )
end

--- 进入场景：只做一次的事（演示 attach、读剪辑清单、关一次调试线框）。
function M.on_create()
    api.title("api_3d - transform3d / animation3d / physics3d")
    api.hint(
        "UP/DOWN move parent (child local stays)  L/R move child  T spin  A/D clip  P pause  "
            .. "R reset  F shoot  V speed probe  C ccd  B collider wireframe  "
            .. api.KEYS_BACK
    )

    -- `transform3d.attach(id)`：把一个**还没有 3D 变换**的实体挂上 Transform3D 组件，返回 true。
    -- 引擎的实际语义是**无条件插入一份默认 Transform3D 并返回 true**（`script.rs` 的
    -- `transform3d.attach` 不查旧组件）：所以对已经有 3D 变换的实体再调一次也会返回 true，
    -- 但会把它的变换**重置为默认值**——这段注释就是那句提醒。
    -- 这里用 `entity.spawn` 造一个全新实体（spawn 只给 id，不给任何组件）来演示：
    -- attach 之前 `transform3d.get` 报 "entity has no transform3d"，attach 之后立刻可读可写。
    -- 运行期 spawn 的实体不进场景文件，因此不会影响任何"逐场景"的自动化门。
    local fresh = entity.spawn("AttachProbe3d")
    if fresh and entity.is_alive(fresh) then
        local before = nil
        pcall(function()
            before = transform3d.get(fresh)
        end)
        local attached = transform3d.attach(fresh)
        transform3d.set(fresh, 0, 1.2, -1.2)
        local after = nil
        pcall(function()
            after = transform3d.get(fresh)
        end)
        attach_demo = string.format(
            "attach3d before=%s attach=%s after=%s",
            before and "readable" or "error",
            tostring(attached),
            after and string.format("(%.2f,%.2f,%.2f)", after.x, after.y, after.z) or "error"
        )
    end

    -- 2D 与 3D 的调试线框是**两条互不干扰**的开关（混合场景里"只想看一侧"是常态）。
    -- 它返回传入的布尔值；这里开局先显式关一次，让 B 键有确定的起点。
    physics3d.debug_draw(false)

    -- 剪辑清单来自运行期（`animation3d.list`），不靠写死的名字：
    -- 场景组件里的 `clip=Idle` 只负责"开局播哪一条"。
    local warrior = live(WARRIOR)
    if warrior then
        local listed = nil
        pcall(function()
            listed = animation3d.list(warrior)
        end)
        if listed then
            clips = listed
        end
        if #clips > 0 then
            clip_index = 1
            local ok = animation3d.set(warrior, clips[1])
            log.info("api_3d: clip -> " .. tostring(clips[1]) .. " ok=" .. tostring(ok))
        end
    end
    log.info(string.format("api_3d: animation3d.list returned %d clips", #clips))
end

--- 每个固定步一次：按键 → 改状态；读回 → 进状态行。
function M.update()
    local dt = math.min(time.delta(), MAX_STEP)
    elapsed = elapsed + dt

    local parent = live(GROUND)
    local child = live(CHILD)
    local warrior = live(WARRIOR)
    local ball = live(BALL)
    local probe = live(PROBE)

    -- ── transform3d.set：整份写入（位置 + 四元数 + 缩放）。绕 Y 轴转，并做正弦上下浮动。
    if child then
        local angle = spin and elapsed * 1.1 or 0
        transform3d.set(
            child,
            1.6 + child_slide,
            0.05 + math.sin(elapsed * 2.0) * 0.15,
            0,
            0,
            math.sin(angle * 0.5),
            0,
            math.cos(angle * 0.5),
            1,
            1,
            1
        )
        -- ── transform3d.move：**增量**移动（局部坐标里的 dx,dy,dz），与 set 的绝对写法对照。
        transform3d.move(child, 0, math.sin(elapsed * 3.0) * dt * 0.08, 0)
    end

    -- 子实体沿父的 +X 平移多少（改的是局部值，与父的位置无关）。
    if input.just_pressed("left") then
        child_slide = math.max(child_slide - 0.4, -1.4)
    elseif input.just_pressed("right") then
        child_slide = math.min(child_slide + 0.4, 1.4)
    end

    -- ── transform3d.move 移动**父**：子实体自己的局部值一个字都没变，
    --    但它的世界位置跟着父走 —— `get` 与 `world` 在这里分叉。
    if parent then
        if input.is_down("up") then
            transform3d.move(parent, 0, 1.6 * dt, 0)
        elseif input.is_down("down") then
            transform3d.move(parent, 0, -1.6 * dt, 0)
        end
    end

    if input.just_pressed("t") then
        spin = not spin
        log.info("api_3d: rotate child = " .. tostring(spin))
    end

    -- ── animation3d.list（on_create 已取）/ set / get / paused / set_paused / time。
    --    实体在场景里带着 Animation3D 组件，因此这些调用不会报 "entity has no animation3d"。
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

    -- ── physics3d：动态球的重力 + 撞地反弹（get_velocity → set_velocity）。
    if ball then
        local velocity = nil
        local position = nil
        pcall(function()
            velocity = physics3d.get_velocity(ball)
        end)
        pcall(function()
            position = transform3d.get(ball)
        end)
        if velocity and position then
            if position.y <= BALL_RADIUS - 0.05 and velocity.y < 0 then
                physics3d.set_velocity(
                    ball,
                    velocity.x,
                    math.max(-velocity.y * 0.6, BOUNCE_FLOOR),
                    velocity.z
                )
            end
        end
        if input.just_pressed("r") then
            -- set_position 是**传送**：下一帧物理从这里继续积分。
            physics3d.set_position(ball, BALL_HOME.x, BALL_HOME.y, BALL_HOME.z)
            physics3d.set_velocity(ball, 0, 0, 0)
        end
        if input.just_pressed("f") then
            -- 没有"施加冲量"的 API：速度是唯一入口，位移由物理每步积分出来。
            shot_speed = math.min(shot_speed * -1.2, 24.0)
            physics3d.set_velocity(ball, shot_speed, 4.0, 0)
        end
    end

    -- ── physics3d.get_ccd / set_ccd：高速小球防隧穿。场景串里 `ccd=true` 让开局就是开的，
    --    C 键在开/关之间对照——**不开 CCD 时这颗球一步就能整段跨过薄地面（隧穿）**。
    if probe then
        if input.just_pressed("c") then
            local current = nil
            if
                pcall(function()
                    current = physics3d.get_ccd(probe)
                end)
            then
                physics3d.set_ccd(probe, not current)
                log.info("api_3d: ccd -> " .. tostring(not current))
            end
        end
        if input.just_pressed("v") then
            physics3d.set_position(probe, PROBE_HOME.x, PROBE_HOME.y, PROBE_HOME.z)
            physics3d.set_velocity(probe, 0, -PROBE_SHOT, 0)
        end
        if input.just_pressed("r") then
            physics3d.set_position(probe, PROBE_HOME.x, PROBE_HOME.y, PROBE_HOME.z)
            physics3d.set_velocity(probe, 0, 0, 0)
        end
    end

    -- ── physics3d.debug_draw(enabled)：3D 碰撞体线框。它是**开一次即持续生效**的开关：
    --    宿主渲染期取走这次请求后画线框（动态体绿、固定体青、sensor 黄），不需要每帧调用；
    --    所以这里只在按键那一帧调用。headless（没有渲染帧）下它只是登记请求、不报错、
    --    返回传入的布尔值；宿主渲染期消费见 `RuntimeScene::take_collider_debug_request_3d`。
    if input.just_pressed("b") then
        debug_draw_on = not debug_draw_on
        physics3d.debug_draw(debug_draw_on)
    end

    -- 状态行的取景框：本帧的事实（局部 vs 世界、剪辑、速度、命中实体名、事件数、CCD）。
    api.status(
        string.format(
            "%s | %s | %s | 3d_wireframe=%s | %s",
            hierarchy_text(),
            animation_text(),
            physics_text(),
            tostring(debug_draw_on),
            attach_demo
        )
    )
    return string.format(
        "api_3d_ok spin=%s clip=%s clips=%d events=%d last_hit=%s",
        tostring(spin),
        #clips > 0 and tostring(clips[clip_index]) or "(none)",
        #clips,
        events_seen,
        last_hit
    )
end

--- 离开场景：清掉标题/状态行/提示这三处动态 UI。
function M.on_destroy()
    api.clear()
end

return M
