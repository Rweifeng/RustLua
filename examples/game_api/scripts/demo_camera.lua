-- api_camera：相机三组 API 的可运行演示 —— `camera2d`（2D 相机控制器）、`camera3d`（3D 屏↔
-- 世界投影链）与统一 `Camera` 组件的选择（`camera` 表）。
--
-- 这个演示想证明四件事（每一条都对应下面的一段代码）：
--   ① `camera2d.set` / `camera2d.set_zoom` 是**登记请求、宿主下一帧消费**：脚本"刚请求的值"
--      与 `camera2d.get` / `get_zoom` 读回的"引擎实际在用的值"在同一帧里可以不同（状态行里的
--      request 列与 host 列并排显示）。原因是宿主的 2D 相机控制器是**帧级系统**：它在本帧的
--      Lua update 之后才取走请求、算出本帧相机、再回填真实值，因此回读总比请求晚一帧；
--   ② `camera2d.follow` 生效期间相机中心由控制器决定，此时 `camera2d.set` 会被**覆盖**；
--      `camera2d.unfollow` 之后 `set` 才重新说了算（F / G / V 三个键可以现场对比两列读数）；
--   ③ `screen_to_world` / `world_to_screen` / `mouse_world` 与整个 `camera3d.*` 组用的是宿主
--      **每帧渲染时回填**的相机快照：首帧渲染之前调用会**明确报错**，引擎刻意不拿一台默认
--      相机去猜（猜错会让"点不中 / 点错位置"变成查不出原因的现象）。本演示用 `api.try` 把它
--      包住，并把报错原文打上屏幕 —— 这是引擎刻意的边界，不是演示的缺陷；
--   ④ `camera.set_active(id)` 是**唯一**的选择链（批 9 起不再分 2D/3D 两条）：选中的实体带
--      `Camera` 就行，由它的 `projection` 决定服务哪个 pass —— 透视相机决定 3D pass 取景
--      （按 1 / 2 切换机组看得见画面变化），**正交相机则是 2D 相机的来源**（按 B 把 2D 取景切回
--      场景声明的正交相机：宿主采用它的 `center`/`zoom`/`bounds`）。而 `camera.set` /
--      `camera.get` 读写的是**实体上的 `Camera` 组件**本身（透视 payload）：写入立即生效、
--      立即读回，与 `camera2d` 的"请求 / 消费"模式正好对照。
--
-- 场景（scenes/api_camera.scene）里的东西各自负责什么：
--   Camera2dOrtho  `Camera projection=orthographic` —— 场景声明的正交相机就是运行时**初始
--                  2D 相机**（第一台启用的正交相机），`camera2d.*` 控制的就是它；
--                  按 B 用 `camera.set_active` 显式选中它 ⇒ 2D 取景复位回它声明的值；
--   Camera3dMain / Camera3dTop  两台透视相机，供 `camera.set_active` 切换（决定 3D pass 取景）；
--   Sun + Ground + Prop3d       3D 观察对象：方向光 + `__ground_plane__` 地面 + 一个三角形；
--   Runner（Sprite）            2D 相机的跟随目标：沿椭圆轨迹匀速走，跟随 / 死区 / 平滑才看得见；
--   Marker（Quad）              跟着 `camera2d.mouse_world()` 跑：鼠标指到哪、方块就画在哪
--                               （两者用的是同一份投影实现，因此"点到哪"与"看到哪"一致）；
--   Hud（Text + UiLayout）      屏幕空间 HUD：2D 相机怎么动、切哪台 3D 相机，它都钉在右上角。
--
-- Runner 与 Marker 都显式声明了 `RenderMode mode=overlay`：这句话的意思是"我属于 2D 相机
-- 的屏幕 / overlay 空间"，于是即使宿主把世界精灵通道打开（`world_sprite_depth`）它们也不会
-- 跑到 3D 相机里去 —— 本演示里"2D 相机的世界坐标 = `camera2d.*` 的坐标空间"这条对应关系
-- 因此始终成立。
local api = require("api_demo")

local M = {}

-- ---- 常量（顶层只定义函数与常量，绝不调用引擎 API）--------------------------------

-- Runner 的椭圆轨迹：跟随的目标必须真的在动，"死区 / 平滑"才有可观察的含义。
local RUN_CENTER_X, RUN_CENTER_Y = 640.0, 360.0
local RUN_RADIUS_X, RUN_RADIUS_Y = 170.0, 120.0
local RUN_SPEED = 0.5

-- `camera2d.follow(id, opts)` 的 opts：向上偏移 32px、死区 220x140 像素、每秒收敛率 3。
-- 死区非零 ⇒ Runner 在死区内移动时相机**不动**（这就是"跟随不是把目标硬贴在屏幕中心"证据）。
local FOLLOW_OPTS = {
    offset_x = 0.0,
    offset_y = -32.0,
    dead_zone_x = 110.0,
    dead_zone_y = 70.0,
    smoothing = 3.0,
}

-- 两档 zoom：`camera2d.set_zoom` 只接受 > 0 的有限值。
local ZOOM_BASE, ZOOM_IN = 1.0, 1.8

-- `camera.set` 环绕主相机用的参数（每帧写一次姿态，画面当帧就动）。
local ORBIT_RADIUS, ORBIT_HEIGHT, ORBIT_SPEED, MAIN_FOV = 7.0, 5.0, 0.6, 60.0
-- 主相机在场景里声明的姿态：按 C 用 **7 参数**形式写回（第 8 参 fov 省略 ⇒ fov 不变）。
local MAIN_POS = { 0.0, 6.0, 8.0 }

-- 演示用的固定屏幕探针点：屏幕中心 (640,360)（1280x720 工程的设计分辨率中心）。
local PROBE_SX, PROBE_SY = 640.0, 360.0
-- 按 V 时 `camera2d.set` 的目标中心：不跟随时画面立刻平移过去；跟随中会被控制器覆盖。
local SET_X, SET_Y = 900.0, 430.0

-- ---- 状态 -------------------------------------------------------------------------

local id_camera3d_main, id_camera3d_top = nil, nil
local id_camera2d_ortho = nil
local id_runner, id_marker = nil, nil

local bootstrapped = false      -- 首帧把"登记型 / 写入型"接口各走过一遍
local elapsed = 0.0            -- 累计时间（Runner 轨迹、相机环绕都用它）
local requested_center = nil   -- `camera2d.set` 的请求值（与宿主回填值同屏对照）
local requested_zoom = nil     -- `camera2d.set_zoom` 的请求值
local following = false        -- 当前是否登记了跟随
local orbit = false            -- 主相机环绕（每帧 `camera.set`）
local zoom_level = ZOOM_BASE
local initial_active = nil     -- 首次读到的 `camera.get_active()`（nil = 尚未显式选择）
local active_read_done = false

-- ---- 小工具 -----------------------------------------------------------------------

local function live(id)
    return id ~= nil and entity.is_alive(id)
end

local function bound()
    return live(id_camera3d_main) and live(id_camera3d_top) and live(id_runner) and live(id_marker)
end

--- 2D 换算接口返回的是**数组** `{x, y}`（与 `input.cursor_position()` 同形），
--- 与 `camera2d.get` 的具名字段表 `{x=, y=}` 不是同一种表：这里统一成可读文本。
local function arr_text(v)
    if v == nil then
        return "(nil)"
    end
    return string.format("%.1f,%.1f", v[1], v[2])
end

--- `camera.get(id)` 读回的实体 `Camera` 组件姿态（fov 是**度**）。
local function pose_text(p)
    if p == nil then
        return "(nil)"
    end
    return string.format("p=%.1f,%.1f,%.1f t=%.1f,%.1f,%.1f fov=%.0f", p.x, p.y, p.z, p.tx, p.ty, p.tz, p.fov)
end

--- `camera3d.get()` 读回的是宿主回填的**渲染快照**（字段名是 px/py/pz、带视口尺寸）。
local function snapshot_text(s)
    if s == nil then
        return "(nil)"
    end
    return string.format(
        "p=%.1f,%.1f,%.1f t=%.1f,%.1f,%.1f fov=%.0f near=%.2f far=%.0f view=%.0fx%.0f",
        s.px, s.py, s.pz, s.tx, s.ty, s.tz, s.fov, s.near, s.far, s.width, s.height
    )
end

--- `camera3d.ray` / `mouse_ray` 返回 `{ox,oy,oz,dx,dy,dz}`（这就是能直接喂给
--- `physics3d.raycast(ox,oy,oz,dx,dy,dz,max)` 的六个分量，本场景不重复演示物理那边）。
local function ray_text(r)
    if r == nil then
        return "(nil)"
    end
    return string.format("o=%.1f,%.1f,%.1f d=%.2f,%.2f,%.2f", r.ox, r.oy, r.oz, r.dx, r.dy, r.dz)
end

local function point_text(p)
    if p == nil then
        return "(nil)"
    end
    return string.format("%.2f,%.2f,%.2f", p.x, p.y, p.z)
end

--- `camera3d.world_to_screen` 多一个 `visible`：相机背后的点在 NDC 里也有坐标，
--- 引擎明确用 `visible=false` 标出来（否则 HUD 标记会在背后画出幽灵）。
local function screen_text(s)
    if s == nil then
        return "(nil)"
    end
    return string.format("%.0f,%.0f visible=%s", s.x, s.y, tostring(s.visible))
end

--- `camera.get_active()` 返回的是**实体 id**（nil = 还没显式选择，宿主回退到排序序第一台）。
local function active_text(id)
    if id == nil then
        return "nil(first)"
    end
    if id == id_camera3d_main then
        return "Camera3dMain"
    end
    if id == id_camera3d_top then
        return "Camera3dTop"
    end
    return "entity " .. tostring(id)
end

-- ---- 首帧的一次性演示 -------------------------------------------------------------

--- 首帧把"登记型 / 写入型"接口各调用一次：headless 步进没有键盘输入，按键分支不会被执行，
--- 这里保证每条 API 都真的跑过一遍。这些调用全部是**请求**或与场景声明等值的写入，
--- 因此画面不会因为这次 bootstrap 跳一下。
local function bootstrap()
    -- 登记"回到设计分辨率中心"：宿主下一帧取走并消费（跟随中它会被控制器覆盖，见按 V）。
    camera2d.set(640.0, 360.0)
    requested_center = { 640.0, 360.0 }
    -- 登记 zoom = 1（与场景里正交相机的声明一致 ⇒ 这一帧不跳）。请求值与 host 读回值
    -- 在**本帧**会并排显示，下一帧起通常就一致了 —— 这正是"登记 / 消费"的可见证据。
    camera2d.set_zoom(ZOOM_BASE)
    requested_zoom = ZOOM_BASE
    -- 默认状态就是"跟着 Runner"：带偏移 / 死区 / 平滑，于是跟随看起来像一条跟随镜头，
    -- 而不是"Runner 永远钉在屏幕正中"。
    camera2d.follow(id_runner, FOLLOW_OPTS)
    following = true
    -- 抖动也是一次**请求**（幅度 10px、0.6 秒内线性衰减）：立刻登记，效果由控制器推进。
    camera2d.shake(10.0, 0.6)
    -- 用 7 参数形式把主相机姿态写成与场景**相同**的一组值（fov 省略 ⇒ 保持不变）。
    camera.set(id_camera3d_main, MAIN_POS[1], MAIN_POS[2], MAIN_POS[3], 0.0, 0.0, 0.0)
    -- 显式选中主相机：3D pass 每帧读的就是它（没选择时宿主回退到排序序第一台，
    -- 那个"初始事实"在 update 里第一次读 `camera.get_active()` 时就记下来了）。
    camera.set_active(id_camera3d_main)
end

-- ---- 按键 -------------------------------------------------------------------------

local function handle_keys()
    -- 1 / 2：切换 3D pass 用的透视相机；B：把 2D 相机切回场景声明的正交相机。
    -- `camera.set_active` 是**唯一**的选择链（批 9 起不再区分 2D/3D）：选中的实体带 `Camera`
    -- 就行，由它的 `projection` 决定服务哪个 pass —— 透视进 3D world，正交进 2D overlay。
    if input.just_pressed("1") then
        camera.set_active(id_camera3d_main)
        orbit = false
    elseif input.just_pressed("2") then
        camera.set_active(id_camera3d_top)
        orbit = false
    elseif input.just_pressed("b") then
        -- 选中正交相机 ⇒ 宿主采用它的**载荷**（center/zoom/bounds）作为 2D 取景基准。
        -- 跟随中这一步会被下一帧的跟随再拉走，与 V 键同一性质：状态行的 host 列先跳到场景值、
        -- 随后被跟随带走（先按 G 取消跟随，再按 B，就能看到它停在场景声明的取景上）。
        camera.set_active(id_camera2d_ortho)
    end

    -- F / G：跟随与取消跟随是一对。
    if input.just_pressed("f") then
        camera2d.follow(id_runner, FOLLOW_OPTS)
        following = true
    elseif input.just_pressed("g") then
        camera2d.unfollow()
        following = false
    end

    -- V：直接设置 2D 相机中心。**跟随中它会输**：状态行里 request 列变成 900,430，
    -- 而 host 列仍旧跟着 Runner —— 先按 G 取消跟随，再按 V，host 列才会跟着变。
    if input.just_pressed("v") then
        camera2d.set(SET_X, SET_Y)
        requested_center = { SET_X, SET_Y }
    end

    -- Z：在两档 zoom 之间切换（同样是登记请求，回读晚一帧）。
    if input.just_pressed("z") then
        zoom_level = (zoom_level == ZOOM_BASE) and ZOOM_IN or ZOOM_BASE
        camera2d.set_zoom(zoom_level)
        requested_zoom = zoom_level
    end

    -- X：抖动（幅度 26px、0.6 秒）。
    -- 实测到的引擎行为（写在这里免得读者把状态行的两列读数误读成"set 没生效"）：抖动偏移是
    -- **逐帧叠加在"当前"中心上**的，衰减结束后相机停在被推开的位置，而不是回到抖动前的中心
    -- （跟随开启时会被控制器拉回来，不跟随时就留在那里）。因此按过 X 之后 request 与 host
    -- 会差十来个像素 —— 那是抖动的残留位移。
    if input.just_pressed("x") then
        camera2d.shake(26.0, 0.6)
    end

    -- C：把主相机姿态复位成场景声明的值（7 参数 = 不改 fov），顺带停掉环绕。
    if input.just_pressed("c") then
        orbit = false
        camera.set(id_camera3d_main, MAIN_POS[1], MAIN_POS[2], MAIN_POS[3], 0.0, 0.0, 0.0)
    end

    -- T：环绕主相机（每帧 `camera.set`，这一条用 **8 参数**形式带 fov）。
    if input.just_pressed("t") then
        orbit = not orbit
        if orbit then
            -- 环绕写的是主相机：先把它选为活动相机，否则它在 3D pass 里根本没被取景。
            camera.set_active(id_camera3d_main)
        end
    end

    if orbit then
        local t = elapsed * ORBIT_SPEED
        camera.set(
            id_camera3d_main,
            ORBIT_RADIUS * math.cos(t),
            ORBIT_HEIGHT,
            ORBIT_RADIUS * math.sin(t),
            0.0,
            0.0,
            0.0,
            MAIN_FOV
        )
    end
end

-- ---- 生命周期 ---------------------------------------------------------------------

function M.on_create()
    api.title("api_camera - camera2d controller / camera3d screen<->world / Camera selection")
    api.hint("1/2 pick 3D cam  F/G follow  V set2d  Z zoom  X shake  C reset  T orbit  - " .. api.KEYS_BACK)
    id_camera3d_main = entity.find("Camera3dMain")
    id_camera3d_top = entity.find("Camera3dTop")
    id_camera2d_ortho = entity.find("Camera2dOrtho")
    id_runner = entity.find("Runner")
    id_marker = entity.find("Marker")
    if not bound() then
        api.status("api_camera: scene is missing Camera3dMain / Camera3dTop / Runner / Marker")
    end
    log.info("api_camera: demo ready (2D camera controller + 3D projection chain + Camera selection)")
end

function M.update()
    if not bound() then
        -- 场景坏了（实体改名 / 被删）：不抛 Lua 错误，只把事实说清楚。
        api.status("api_camera: scene entity missing (Camera3dMain / Camera3dTop / Runner / Marker)")
        return "api_camera_missing_entity"
    end

    -- 第一次读 `camera.get_active()`：nil 说明 Lua 还没显式选择过，此时宿主用"排序序第一台
    -- 启用的 Camera3D"。这个初始事实先记下来，再在 bootstrap 里显式选择。
    if not active_read_done then
        initial_active = api.try(function()
            return camera.get_active()
        end)
        active_read_done = true
    end
    if not bootstrapped then
        bootstrap()
        bootstrapped = true
    end

    elapsed = elapsed + time.delta()

    -- ① 跟随目标沿椭圆匀速走：只用 2D 的 `transform.set`（2D 实体 = 世界像素坐标）。
    local angle = elapsed * RUN_SPEED
    transform.set(
        id_runner,
        RUN_CENTER_X + RUN_RADIUS_X * math.cos(angle),
        RUN_CENTER_Y + RUN_RADIUS_Y * math.sin(angle)
    )

    handle_keys()

    -- ② `camera2d` 的**读数**：`get` / `get_zoom` 读的是宿主回填的真实值（控制器每个帧级
    --    系统都会 report，因此即使没有渲染帧也读得到；但它们的数与下面的换算接口同源，
    --    所以一并包在 `api.try` 里，两边的失败处理就只有一个地方）。
    local host_center = api.try(function()
        return camera2d.get()
    end)
    local host_zoom = api.try(function()
        return camera2d.get_zoom()
    end)

    -- ③ 2D 屏↔世界换算：**宿主每帧渲染时回填**的快照，首帧渲染之前会明确报错。
    --    用它当"快照就绪"的探针：探针失败就把报错原文打上屏幕（而不是悄悄显示 0）。
    local probe, probe_err = api.try(function()
        return camera2d.screen_to_world(PROBE_SX, PROBE_SY)
    end)
    local snapshot_ready = (probe ~= nil)

    -- 屏幕点 → 世界 → 屏幕：两次换算用的是**同一份**投影实现（`math2d` 里只有这一份），
    -- 因此往返误差应当 ≈ 0 —— 这就是"点到哪"与"看到哪"不会错位的那条链。
    local round_world = api.try(function()
        return camera2d.screen_to_world(PROBE_SX, PROBE_SY)
    end)
    local round_screen = nil
    if round_world then
        -- 注意参数来自上一步的世界坐标，而不是再算一遍屏幕点：这样才真的是一次往返。
        local wx, wy = round_world[1], round_world[2]
        round_screen = api.try(function()
            return camera2d.world_to_screen(wx, wy)
        end)
    end
    local round_err = "(n/a)"
    if round_screen then
        round_err = string.format("%.3f,%.3f", round_screen[1] - PROBE_SX, round_screen[2] - PROBE_SY)
    end

    -- Runner 的世界坐标 → 屏幕像素：跟随 / 死区生效时它会偏离屏幕中心一点点。
    local runner_world = api.try(function()
        return transform.get(id_runner)
    end)
    local runner_screen = nil
    if runner_world then
        local rx, ry = runner_world.x, runner_world.y
        runner_screen = api.try(function()
            return camera2d.world_to_screen(rx, ry)
        end)
    end

    -- 鼠标位置的世界坐标：把 Marker 挪过去 —— "鼠标指到哪，方块就画在哪"。
    local cursor = input.cursor_position()
    local cursor_x, cursor_y = cursor[1], cursor[2]
    local mouse_world, mouse_err = api.try(function()
        return camera2d.mouse_world()
    end)
    if mouse_world then
        transform.set(id_marker, mouse_world[1], mouse_world[2])
    end

    -- ④ 3D 链：宿主每帧把**本帧 3D pass 实际用的那台相机**（`camera.set_active` 选中的，
    --    否则排序序第一台）与视口回填进来，所以"点中哪个"与"画了哪个"用的是同一台相机。
    local snap, snap_err = api.try(function()
        return camera3d.get()
    end)
    local center_ray = api.try(function()
        return camera3d.ray(PROBE_SX, PROBE_SY)
    end)
    local mouse_ray = api.try(function()
        return camera3d.mouse_ray()
    end)
    -- 鼠标射线 ∩ 地面 y = 0：地面点选（放置 / 寻路 / 点击地面）最常见的一步。
    -- 没命中（射线与平面平行或背向）时返回 nil 而不是一个假点。
    local ground = api.try(function()
        return camera3d.pick_plane(cursor_x, cursor_y, 0.0)
    end)
    -- 把地面命中点再投回屏幕：应当 ≈ 鼠标位置（"点中哪、看到哪"在 3D 侧也成立）。
    local ground_screen = nil
    if ground then
        local gx, gy, gz = ground.x, ground.y, ground.z
        ground_screen = api.try(function()
            return camera3d.world_to_screen(gx, gy, gz)
        end)
    end
    local pick_delta = "(n/a)"
    if ground_screen then
        pick_delta = string.format("%.1f,%.1f", ground_screen.x - cursor_x, ground_screen.y - cursor_y)
    end

    -- `camera.get(id)` 读的是**实体上的** `Camera` 组件（写进去立刻读得回来），
    -- 而 `camera3d.get()` 读的是宿主回填的**渲染快照**（上一帧渲染用的那台）。
    -- 环绕（T）时两者会差一帧，正好把这两种"真源"的区别摆在屏幕上。
    local main_pose = api.try(function()
        return camera.get(id_camera3d_main)
    end)
    local top_pose = api.try(function()
        return camera.get(id_camera3d_top)
    end)
    local active_now = api.try(function()
        return camera.get_active()
    end)

    -- ---- 屏幕上的三行事实 ---------------------------------------------------------

    -- 第一行：2D 相机（请求 vs 宿主回填 vs 跟随状态）。
    local req_center = requested_center
            and string.format("%.0f,%.0f", requested_center[1], requested_center[2])
        or "-"
    local req_zoom = requested_zoom and string.format("%.2f", requested_zoom) or "-"
    local host_center_text = host_center and string.format("%.1f,%.1f", host_center.x, host_center.y)
        or "(n/a)"
    local host_zoom_text = host_zoom and string.format("%.2f", host_zoom) or "(n/a)"
    ui.text_at(
        "api_camera_cam2d",
        24,
        568,
        string.format(
            "camera2d  request(set/set_zoom): center=%s zoom=%s  |  host(get/get_zoom): center=%s zoom=%s  |  follow=%s  orbit=%s  snapshot=%s",
            req_center,
            req_zoom,
            host_center_text,
            host_zoom_text,
            tostring(following),
            tostring(orbit),
            tostring(snapshot_ready)
        ),
        1.0
    )

    -- 第二行：统一 `Camera` 组件的选择与读写（camera.get 是实体组件，立即生效）。
    ui.text_at(
        "api_camera_cam3d",
        24,
        592,
        string.format(
            "camera  active=%s (first read=%s)  |  camera.get(main): %s  |  camera.get(top): %s  |  camera3d.get: %s",
            active_text(active_now),
            active_text(initial_active),
            pose_text(main_pose),
            pose_text(top_pose),
            snapshot_text(snap)
        ),
        1.0
    )

    -- 第三行（状态行）：2D 屏↔世界链的读数；快照没回填时显示引擎给的报错原文。
    if snapshot_ready then
        api.status(string.format(
            "2D screen(%.0f,%.0f) -> world %s -> screen %s (round-trip err=%s) | mouse_world %s -> Marker | Runner world=%s screen=%s",
            PROBE_SX,
            PROBE_SY,
            arr_text(round_world),
            arr_text(round_screen),
            round_err,
            arr_text(mouse_world),
            runner_world and string.format("%.1f,%.1f", runner_world.x, runner_world.y) or "(n/a)",
            arr_text(runner_screen)
        ))
    else
        api.status(string.format(
            "2D screen<->world needs one rendered frame first (the engine refuses to guess a camera): %s | mouse_world: %s | camera2d.get() still works: %s",
            tostring(probe_err),
            tostring(mouse_err),
            host_center_text
        ))
    end

    -- 第四行：3D 屏↔世界链（ray / mouse_ray / pick_plane / world_to_screen）。
    if snap == nil then
        ui.text_at(
            "api_camera_pick",
            24,
            684,
            string.format(
                "camera3d ray/mouse_ray/pick_plane need one rendered frame first: %s",
                tostring(snap_err)
            ),
            1.0
        )
    else
        ui.text_at(
            "api_camera_pick",
            24,
            684,
            string.format(
                "camera3d  ray(%.0f,%.0f) %s  |  mouse_ray %s  |  pick_plane(mouse, y=0)=%s -> screen %s (mouse delta=%s)",
                PROBE_SX,
                PROBE_SY,
                ray_text(center_ray),
                ray_text(mouse_ray),
                point_text(ground),
                screen_text(ground_screen),
                pick_delta
            ),
            1.0
        )
    end

    -- 返回值进引擎日志：headless 也能看到"选了哪台相机 / 宿主相机中心 / 跟随状态"。
    return string.format(
        "api_camera_ok active=%s first=%s host_center=%s host_zoom=%s follow=%s orbit=%s snapshot=%s",
        active_text(active_now),
        active_text(initial_active),
        host_center_text,
        host_zoom_text,
        tostring(following),
        tostring(orbit),
        tostring(snapshot_ready)
    )
end

function M.on_destroy()
    -- 本模块自己的动态文本由本模块清（路由只清 api_title / api_status / api_hint）。
    ui.clear("api_camera_cam2d")
    ui.clear("api_camera_cam3d")
    ui.clear("api_camera_pick")
    id_camera3d_main, id_camera3d_top = nil, nil
    id_runner, id_marker = nil, nil
    bootstrapped = false
    active_read_done = false
    following = false
    orbit = false
    requested_center, requested_zoom = nil, nil
    initial_active = nil
end

return M
