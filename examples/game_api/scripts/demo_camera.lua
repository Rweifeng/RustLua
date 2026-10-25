-- api_camera：camera / camera2d / camera3d 的 2.0 分步 Lab。
--
-- 一条主线：**相机组件是数据，选中项是会话态，2D 控制器的写请求由宿主下一帧消费**。
--   * `camera.set` / `camera.get` 直接读写实体的透视载荷，**当场**可读回；
--   * `camera2d.set` / `set_zoom` / `shake` 只登记请求，`get` / `get_zoom` 读宿主回填的真值；
--   * `camera2d.screen_to_world` / `camera3d.*` 用的是宿主每帧烘焙的渲染快照 ——
--     没有渲染帧时它们**明确报错**，不猜默认相机。
-- 因此自检只断言"请求被接受"与"读回值形状正确"，不去假设请求在同一帧已经生效。
--
-- 交互键（Lab 保留 space/方向键/1..9/R/F/F5，这里只用字母）：
--   G follow  H unfollow  Z/V zoom -/+  X shake  C recenter
--   C 3D main  T 3D top  Y 2D camera  Z/V fov -/+（camera_selection 步）
--   B cast one more downward ray（raycast 步）
--
-- 硬规则（README §2）：顶层只定义常量与函数；`run` 在 headless 下只读或做内存写，
-- 不写盘、不切场景、不模拟输入。
local lab = require("lab")

local M = {}

-- 场景实体名（scenes/api_camera.scene）。
local CAM_2D = "Camera2dOrtho"
local CAM_MAIN = "Camera3dMain"
local CAM_TOP = "Camera3dTop"
local PROP = "Prop3d"
local RUNNER = "Runner"
local MARKER = "Marker"

-- 舞台是 x 296..936、y 76..644；runner 绕 (640,360) 转，所有投影点再夹进框里。
local STAGE = { x0 = 296, y0 = 76, x1 = 936, y1 = 644 }
local HOME = { x = 640, y = 360 }
local ORBIT_RX = 110
local ORBIT_RY = 70

-- 主 3D 相机的作者初始值（enter 每次都推回这一组，保证 F5 与用户操作无关）。
local MAIN = { x = 0, y = 6, z = 8, tx = 0, ty = 0, tz = 0, fov = 60 }

-- 动态 UI 的 id 清单：离开时必须逐个清（`lab.stop` 只清 shell）。
local UI_IDS = {
    "cam_head", "cam_info_1", "cam_info_2", "cam_info_3", "cam_info_4",
    "cam_cross_h", "cam_cross_v", "cam_focus_t", "cam_focus_b", "cam_focus_l", "cam_focus_r",
    "cam_mouse_dot", "cam_hit_dot",
}

local ids = {}
local phase = 0

local obs = {
    follow = true,
    center = { x = HOME.x, y = HOME.y },
    zoom = 1.0,
    runner = { x = HOME.x, y = HOME.y },
    round = nil,
    mouse = nil,
    view3d = nil,
    ray = nil,
    plane = nil,
    screen = nil,
    active = nil,
    hit = nil,
    prop = nil,
    op = "ready",
}

-- ---------------------------------------------------------------------------
-- 纯值工具
-- ---------------------------------------------------------------------------

local function clamp(value, low, high)
    if value < low then
        return low
    end
    if value > high then
        return high
    end
    return value
end

local function short(value)
    if type(value) ~= "number" then
        return "-"
    end
    if math.abs(value - math.floor(value + 0.5)) < 0.001 then
        return string.format("%d", math.floor(value + 0.5))
    end
    return string.format("%.2f", value)
end

local function point_text(x, y)
    return string.format("%s,%s", short(x), short(y))
end

-- ---------------------------------------------------------------------------
-- 场景 / 相机状态工具
-- ---------------------------------------------------------------------------

--- 按名字取实体；场景被改名/删实体时返回 nil，调用点先判空（不硬崩）。
local function find_ids()
    ids.cam2d = entity.find(CAM_2D)
    ids.main = entity.find(CAM_MAIN)
    ids.top = entity.find(CAM_TOP)
    ids.prop = entity.find(PROP)
    ids.runner = entity.find(RUNNER)
    ids.marker = entity.find(MARKER)
end

--- 清掉本模块的动态 UI（切步骤 / 销毁时调用）。
local function clear_ui()
    for _, id in ipairs(UI_IDS) do
        ui.clear(id)
    end
end

--- 把 2D 控制器、runner、3D 主相机都推回作者初始值。
--- 这是"F5 一定全绿"的前提：读者可能跟随/缩放/抖动/切过相机。
local function reset_camera_state()
    if ids.runner then
        transform.set(ids.runner, HOME.x, HOME.y)
    end
    if ids.marker then
        transform.set(ids.marker, HOME.x, HOME.y)
    end
    if ids.main then
        camera.set(ids.main, MAIN.x, MAIN.y, MAIN.z, MAIN.tx, MAIN.ty, MAIN.tz, MAIN.fov)
    end
    camera2d.unfollow()
    camera2d.set(HOME.x, HOME.y)
    camera2d.set_zoom(1.0)
    camera2d.shake(0, 0.001)
    phase = 0
    obs.follow = false
end

local function select_camera(id)
    if id then
        camera.set_active(id)
    end
end

--- 投影/鼠标点可能落在舞台外：先夹进来再交给 Transform（世界内容不越框）。
--- 边距取 34：跟随框 60 宽，夹到 34 后四条边也仍在舞台内。
local function stage_point(x, y)
    return clamp(x, STAGE.x0 + 34, STAGE.x1 - 34), clamp(y, STAGE.y0 + 34, STAGE.y1 - 34)
end

local function draw_head(text)
    ui.text_at("cam_head", 320, 96, text, 1.0, 14)
end

local function draw_info(lines)
    for index = 1, 4 do
        local text = lines[index]
        if text then
            ui.text_at("cam_info_" .. index, 320, 468 + (index - 1) * 26, text, 1.0, 12)
        else
            ui.clear("cam_info_" .. index)
        end
    end
end

local function draw_crosshair(visible)
    if not visible then
        ui.clear("cam_cross_h")
        ui.clear("cam_cross_v")
        return
    end
    ui.image("cam_cross_h", 614, 359, 52, 2, "__white__", 0.36, 0.62, 1.00)
    ui.image("cam_cross_v", 639, 334, 2, 52, "__white__", 0.36, 0.62, 1.00)
end

local function active_name()
    local active = obs.active
    if active == nil then
        return "none"
    end
    if active == ids.main then
        return "3d main"
    end
    if active == ids.top then
        return "3d top"
    end
    if active == ids.cam2d then
        return "2d ortho"
    end
    return tostring(active)
end

-- ---------------------------------------------------------------------------
-- 步骤
-- ---------------------------------------------------------------------------

local steps = {
    {
        id = "follow_zoom",
        title = "Follow & zoom",
        claim = "camera2d 的写接口只登记请求：follow/unfollow/set/set_zoom/shake 当场返回 true",
        api = { "camera2d.follow", "camera2d.unfollow", "camera2d.set", "camera2d.set_zoom", "camera2d.get", "camera2d.get_zoom", "camera2d.shake", "transform.get", "transform.set" },
        code = 'camera2d.follow(id, {dead_zone_x=40}); camera2d.set(640, 360); camera2d.set_zoom(1.5)',
        enter = function()
            find_ids()
            clear_ui()
            reset_camera_state()
            select_camera(ids.cam2d)
            if ids.runner then
                camera2d.follow(ids.runner, { dead_zone_x = 40, dead_zone_y = 30, smoothing = 6 })
                obs.follow = true
            end
            obs.op = "2d controller reset, following Runner"
        end,
        run = function(dt)
            phase = phase + dt * 1.2
            local x = HOME.x + math.cos(phase) * ORBIT_RX
            local y = HOME.y + math.sin(phase) * ORBIT_RY
            if ids.runner then
                transform.set(ids.runner, x, y)
                local value = transform.get(ids.runner)
                obs.runner.x = value.x
                obs.runner.y = value.y
            end
            local center = camera2d.get()
            obs.center.x = center.x
            obs.center.y = center.y
            obs.zoom = camera2d.get_zoom()

            -- 跟随生效时 Runner 应落在中心；十字准星标的是相机中心（屏幕空间固定）。
            local screen = lab.try(function()
                local value = camera2d.world_to_screen(x, y)
                return { x = value[1], y = value[2] }
            end)
            if screen then
                local sx, sy = stage_point(screen.x, screen.y)
                -- 细边框而不是实心块：动态 UI 永远盖在世界之上，实心图会把 Runner 遮住。
                ui.image("cam_focus_t", sx - 30, sy - 30, 60, 2, "__white__", 0.34, 0.80, 0.50)
                ui.image("cam_focus_b", sx - 30, sy + 28, 60, 2, "__white__", 0.34, 0.80, 0.50)
                ui.image("cam_focus_l", sx - 30, sy - 28, 2, 56, "__white__", 0.34, 0.80, 0.50)
                ui.image("cam_focus_r", sx + 28, sy - 28, 2, 56, "__white__", 0.34, 0.80, 0.50)
            else
                ui.clear("cam_focus_t")
                ui.clear("cam_focus_b")
                ui.clear("cam_focus_l")
                ui.clear("cam_focus_r")
            end
            draw_crosshair(true)

            draw_head("CAMERA2D   follow / dead zone / zoom / shake")
            draw_info({
                string.format("follow=%s   zoom=%s   center=%s", tostring(obs.follow), short(obs.zoom), point_text(obs.center.x, obs.center.y)),
                string.format("runner world=%s   readback space=%s", point_text(obs.runner.x, obs.runner.y), ids.runner and transform.get(ids.runner).space or "-"),
                "G follow   H unfollow   Z/V zoom -/+   X shake   C recenter",
                obs.op,
            })
            lab.note(string.format("zoom=%s center=%s follow=%s", short(obs.zoom), point_text(obs.center.x, obs.center.y), tostring(obs.follow)))
        end,
        read = function()
            return {
                { label = "zoom", value = short(obs.zoom) },
                { label = "center", value = point_text(obs.center.x, obs.center.y) },
                { label = "follow", value = tostring(obs.follow) },
                { label = "runner", value = point_text(obs.runner.x, obs.runner.y) },
                { label = "last op", value = obs.op },
            }
        end,
        check = function(t)
            t:ok(ids.runner ~= nil, "Runner exists in the scene")
            t:eq(entity.is_alive(ids.runner), true, "Runner is alive")
            local value = transform.get(ids.runner)
            t:eq(value.space, "2d", "Runner has a 2d transform")

            t:eq(camera2d.set(HOME.x, HOME.y), true, "camera2d.set accepts a center")
            t:eq(camera2d.set_zoom(1.0), true, "camera2d.set_zoom accepts a positive zoom")
            local zoom = camera2d.get_zoom()
            t:ok(type(zoom) == "number" and zoom > 0, "get_zoom reads a positive host value")
            local center = camera2d.get()
            t:eq(type(center.x), "number", "get returns numeric x")

            t:eq(camera2d.follow(ids.runner, { dead_zone_x = 40, dead_zone_y = 30, smoothing = 6 }), true, "follow accepts a live entity")
            t:eq(camera2d.unfollow(), true, "unfollow clears the target")
            t:eq(camera2d.shake(4, 0.2), true, "shake accepts amplitude and duration")

            local bad, reason = lab.try(function()
                return camera2d.set_zoom(0)
            end)
            t:ok(bad == nil and type(reason) == "string", "a non-positive zoom is a Lua error")

            camera2d.set(HOME.x, HOME.y)
            camera2d.set_zoom(1.0)
            camera2d.shake(0, 0.001)
        end,
    },
    {
        id = "screen_world",
        title = "Screen <-> world",
        claim = "screen_to_world 与 world_to_screen 互逆，mouse_world 只是同一换算的鼠标入口",
        api = { "camera2d.screen_to_world", "camera2d.world_to_screen", "camera2d.mouse_world", "camera2d.get" },
        code = 'local p = camera2d.screen_to_world(640, 360); camera2d.world_to_screen(p[1], p[2])',
        enter = function()
            find_ids()
            clear_ui()
            reset_camera_state()
            select_camera(ids.cam2d)
            obs.round = nil
            obs.mouse = nil
            obs.op = "conversion needs the host's baked 2d view"
        end,
        run = function()
            local round, round_error = lab.try(function()
                local world = camera2d.screen_to_world(HOME.x, HOME.y)
                local screen = camera2d.world_to_screen(world[1], world[2])
                return { wx = world[1], wy = world[2], sx = screen[1], sy = screen[2] }
            end)
            obs.round = round
            local mouse, mouse_error = lab.try(function()
                local world = camera2d.mouse_world()
                return { x = world[1], y = world[2] }
            end)
            obs.mouse = mouse

            local center = camera2d.get()
            obs.center.x = center.x
            obs.center.y = center.y
            obs.zoom = camera2d.get_zoom()

            -- Marker 落在鼠标的世界坐标上；十字准星画的是它投影回屏幕的位置。
            local mx = mouse and mouse.x or HOME.x
            local my = mouse and mouse.y or HOME.y
            if ids.marker then
                transform.set(ids.marker, stage_point(mx, my))
            end
            local pointer = lab.try(function()
                local screen = camera2d.world_to_screen(mx, my)
                return { x = screen[1], y = screen[2] }
            end)
            if pointer then
                local px, py = stage_point(pointer.x, pointer.y)
                ui.image("cam_mouse_dot", px - 7, py - 7, 14, 14, "__white__", 1.00, 0.62, 0.28)
            else
                ui.clear("cam_mouse_dot")
            end

            draw_head("CAMERA2D   screen <-> world (shared math2d projection)")
            draw_info({
                round and string.format("screen 640,360 -> world %s -> screen %s", point_text(round.wx, round.wy), point_text(round.sx, round.sy))
                    or ("no baked view: " .. tostring(round_error)),
                mouse and ("mouse world " .. point_text(mouse.x, mouse.y)) or ("mouse_world: " .. tostring(mouse_error)),
                string.format("camera center %s   zoom %s", point_text(obs.center.x, obs.center.y), short(obs.zoom)),
                "the marker follows mouse_world; the cross marks its projection",
            })
            lab.note("round-trip 640,360 -> world -> screen stays at the viewport center")
        end,
        read = function()
            local round = obs.round
            local mouse = obs.mouse
            return {
                { label = "world", value = round and point_text(round.wx, round.wy) or "no view" },
                { label = "back", value = round and point_text(round.sx, round.sy) or "no view" },
                { label = "mouse", value = mouse and point_text(mouse.x, mouse.y) or "no view" },
                { label = "center", value = point_text(obs.center.x, obs.center.y) },
                { label = "zoom", value = short(obs.zoom) },
            }
        end,
        check = function(t)
            local round, round_error = lab.try(function()
                local world = camera2d.screen_to_world(HOME.x, HOME.y)
                local screen = camera2d.world_to_screen(world[1], world[2])
                return { wx = world[1], wy = world[2], sx = screen[1], sy = screen[2] }
            end)
            t:ok(round ~= nil or type(round_error) == "string", "screen_to_world returns a point or a controlled error")
            if round then
                t:near(round.sx, HOME.x, 1.5, "world_to_screen inverts screen_to_world (x)")
                t:near(round.sy, HOME.y, 1.5, "world_to_screen inverts screen_to_world (y)")
            else
                t:ok(true, "round-trip x skipped without a baked 2d view")
                t:ok(true, "round-trip y skipped without a baked 2d view")
            end

            local mouse, mouse_error = lab.try(function()
                local world = camera2d.mouse_world()
                return { x = world[1], y = world[2] }
            end)
            t:ok(mouse ~= nil or type(mouse_error) == "string", "mouse_world returns a point or a controlled error")
            t:eq(type(camera2d.get().x), "number", "camera2d.get still reports the host center")
        end,
    },
    {
        id = "camera3d_chain",
        title = "3D projection",
        claim = "camera3d 的 get/ray/mouse_ray/pick_plane/world_to_screen 共用一份已烘焙的 3D 快照",
        api = { "camera3d.get", "camera3d.ray", "camera3d.mouse_ray", "camera3d.pick_plane", "camera3d.world_to_screen" },
        code = 'camera3d.ray(640, 360); camera3d.pick_plane(640, 360, 0); camera3d.world_to_screen(0, 0, 0)',
        enter = function()
            find_ids()
            clear_ui()
            reset_camera_state()
            select_camera(ids.main)
            obs.view3d = nil
            obs.ray = nil
            obs.plane = nil
            obs.screen = nil
            obs.op = "waiting for the host's 3d view"
        end,
        run = function()
            local view, view_error = lab.try(function()
                return camera3d.get()
            end)
            local ray, ray_error = lab.try(function()
                return camera3d.ray(HOME.x, HOME.y)
            end)
            local mouse, mouse_error = lab.try(function()
                return camera3d.mouse_ray()
            end)
            local plane, plane_error = lab.try(function()
                return camera3d.pick_plane(HOME.x, HOME.y, 0)
            end)
            local screen, screen_error = lab.try(function()
                return camera3d.world_to_screen(0, 0, 0)
            end)
            obs.view3d = view
            obs.ray = ray
            obs.plane = plane
            obs.screen = screen

            -- 世界原点投影到屏幕：Marker 跟着它走（夹在舞台里）。
            if screen then
                transform.set(ids.marker, stage_point(screen.x, screen.y))
            else
                transform.set(ids.marker, HOME.x, HOME.y)
            end

            draw_head("CAMERA3D   ray / ground pick / world -> screen")
            draw_info({
                view and string.format("camera3d.get pos %s,%s,%s fov %s viewport %sx%s",
                    short(view.px), short(view.py), short(view.pz), short(view.fov), short(view.width), short(view.height))
                    or ("no 3d view: " .. tostring(view_error)),
                ray and string.format("ray(640,360) origin %s,%s,%s dir %s,%s,%s",
                    short(ray.ox), short(ray.oy), short(ray.oz), short(ray.dx), short(ray.dy), short(ray.dz))
                    or ("ray: " .. tostring(ray_error)),
                mouse and "mouse_ray returns the same six components" or ("mouse_ray: " .. tostring(mouse_error)),
                plane and string.format("pick_plane(640,360,0) -> %s,%s,%s", short(plane.x), short(plane.y), short(plane.z))
                    or ("pick_plane: " .. tostring(plane_error or "miss")),
            })
            lab.note("all five calls read the same snapshot the renderer baked last frame")
        end,
        read = function()
            local view = obs.view3d
            local ray = obs.ray
            local screen = obs.screen
            return {
                { label = "camera3d pos", value = view and string.format("%s,%s,%s", short(view.px), short(view.py), short(view.pz)) or "no view" },
                { label = "fov", value = view and short(view.fov) or "no view" },
                { label = "ray dir", value = ray and string.format("%s,%s,%s", short(ray.dx), short(ray.dy), short(ray.dz)) or "no view" },
                { label = "origin screen", value = screen and point_text(screen.x, screen.y) or "behind" },
                { label = "visible", value = screen and tostring(screen.visible) or "-" },
            }
        end,
        check = function(t)
            local view, view_error = lab.try(function()
                return camera3d.get()
            end)
            t:ok(view ~= nil or type(view_error) == "string", "camera3d.get returns a view or a controlled error")
            t:ok(view == nil or (type(view.px) == "number" and type(view.fov) == "number"), "the view carries numeric position and fov")

            local ray, ray_error = lab.try(function()
                return camera3d.ray(HOME.x, HOME.y)
            end)
            t:ok(ray ~= nil or type(ray_error) == "string", "camera3d.ray returns a ray or a controlled error")
            if ray then
                t:eq(type(ray.dy), "number", "the ray carries a numeric dy")
                t:ok(ray.dy < 0, "the central ray points down toward the target")
            else
                t:eq(type(ray_error), "string", "the ray error is a string")
                t:ok(true, "central-ray direction skipped without a 3d view")
            end

            local mouse, mouse_error = lab.try(function()
                return camera3d.mouse_ray()
            end)
            t:ok(mouse ~= nil or type(mouse_error) == "string", "camera3d.mouse_ray returns a ray or a controlled error")

            local plane, plane_error = lab.try(function()
                return camera3d.pick_plane(HOME.x, HOME.y, 0)
            end)
            t:ok(plane == nil or type(plane.x) == "number" or type(plane_error) == "string", "pick_plane returns a point, nil, or an error")
            local screen, screen_error = lab.try(function()
                return camera3d.world_to_screen(0, 0, 0)
            end)
            t:ok(screen == nil or type(screen.x) == "number" or type(screen_error) == "string", "world_to_screen returns a point, nil, or an error")
        end,
    },
    {
        id = "camera_selection",
        title = "Camera select",
        claim = "camera.set/get 直接读写透视载荷；set_active/get_active 是一条立即生效的选择链",
        api = { "camera.set", "camera.get", "camera.set_active", "camera.get_active" },
        code = 'camera.set(main, 1, 7, 9, 0, 0, 0, 55); camera.set_active(main); camera.get_active()',
        enter = function()
            find_ids()
            clear_ui()
            reset_camera_state()
            select_camera(ids.main)
            obs.active = camera.get_active()
            obs.op = "main camera selected"
        end,
        run = function(dt)
            phase = phase + dt * 0.8
            if ids.main then
                camera.set(ids.main, math.sin(phase) * 3, MAIN.y, MAIN.z, MAIN.tx, MAIN.ty, MAIN.tz, MAIN.fov)
            end
            obs.active = camera.get_active()
            local view = ids.main and camera.get(ids.main) or nil
            draw_head("CAMERA   one selection chain, two projections")
            draw_info({
                string.format("active = %s", active_name()),
                view and string.format("main payload pos %s,%s,%s target %s,%s,%s fov %s",
                    short(view.x), short(view.y), short(view.z), short(view.tx), short(view.ty), short(view.tz), short(view.fov))
                    or "main camera missing",
                "C main   T top   Y 2d   Z/V fov -/+",
                obs.op,
            })
            lab.note(string.format("active=%s (selection takes effect immediately)", active_name()))
        end,
        read = function()
            local view = ids.main and camera.get(ids.main) or nil
            return {
                { label = "active", value = active_name() },
                { label = "get_active", value = obs.active or "nil" },
                { label = "main pos", value = view and string.format("%s,%s,%s", short(view.x), short(view.y), short(view.z)) or "-" },
                { label = "fov", value = view and short(view.fov) or "-" },
                { label = "last op", value = obs.op },
            }
        end,
        check = function(t)
            t:ok(ids.main ~= nil and ids.top ~= nil, "both 3D cameras exist")

            t:eq(camera.set(ids.main, 1, 7, 9, 0, 0, 0, 55), true, "camera.set accepts a perspective payload")
            local view = camera.get(ids.main)
            t:near(view.x, 1, 0.001, "camera.get reads back x")
            t:near(view.y, 7, 0.001, "camera.get reads back y")
            t:near(view.z, 9, 0.001, "camera.get reads back z")
            t:near(view.fov, 55, 0.001, "camera.get reads back fov")

            t:eq(camera.set_active(ids.main), true, "set_active accepts a camera entity")
            t:eq(camera.get_active(), ids.main, "get_active reports the main camera")
            t:eq(camera.set_active(ids.top), true, "set_active can switch to the top camera")
            t:eq(camera.get_active(), ids.top, "get_active follows the switch")

            local dead, reason = lab.try(function()
                return camera.get(999999)
            end)
            t:ok(dead == nil and type(reason) == "string", "a dead entity id is a Lua error")

            camera.set_active(ids.main)
            camera.set(ids.main, MAIN.x, MAIN.y, MAIN.z, MAIN.tx, MAIN.ty, MAIN.tz, MAIN.fov)
            obs.active = camera.get_active()
        end,
    },
    {
        id = "raycast",
        title = "3D raycast",
        claim = "physics3d.raycast 命中碰撞体时返回实体 id，未命中返回 nil，负距离当场报错",
        api = { "physics3d.raycast", "entity.find", "entity.is_alive", "transform.get" },
        code = 'physics3d.raycast(0, 5, 0, 0, -1, 0, 20); entity.is_alive(hit)',
        enter = function()
            find_ids()
            clear_ui()
            reset_camera_state()
            select_camera(ids.main)
            obs.hit = nil
            obs.prop = nil
            obs.op = "cast pending"
        end,
        run = function()
            local hit, hit_error = lab.try(function()
                return physics3d.raycast(0, 5, 0, 0, -1, 0, 20)
            end)
            obs.hit = hit
            if ids.prop then
                obs.prop = transform.get(ids.prop)
            end
            local alive = hit ~= nil and entity.is_alive(hit) or false
            if hit then
                ui.image("cam_hit_dot", 700, 250, 26, 26, "__white__", 0.34, 0.86, 0.48)
            else
                ui.clear("cam_hit_dot")
            end

            draw_head("PHYSICS3D   raycast down onto the ground / prop")
            draw_info({
                hit and string.format("hit entity %s   alive=%s", tostring(hit), tostring(alive))
                    or ("no hit (" .. tostring(hit_error or "nil") .. "); a cast may miss"),
                obs.prop and string.format("Prop3d transform %s: %s,%s,%s", obs.prop.space, short(obs.prop.x), short(obs.prop.y), short(obs.prop.z))
                    or "Prop3d missing",
                "B casts one more ray from (0,5,0) down to y=-1",
                obs.op,
            })
            lab.note(string.format("hit=%s alive=%s", hit and tostring(hit) or "nil", tostring(alive)))
        end,
        read = function()
            local prop = obs.prop
            return {
                { label = "hit", value = obs.hit and tostring(obs.hit) or "nil" },
                { label = "prop space", value = prop and prop.space or "-" },
                { label = "prop pos", value = prop and string.format("%s,%s,%s", short(prop.x), short(prop.y), short(prop.z)) or "-" },
                { label = "last op", value = obs.op },
            }
        end,
        check = function(t)
            local negative, reason = lab.try(function()
                return physics3d.raycast(0, 5, 0, 0, -1, 0, -1)
            end)
            t:ok(negative == nil and type(reason) == "string", "a negative distance is rejected")
            t:ok(reason ~= nil and reason:find("non%-negative") ~= nil, "the rejection names the non-negative rule")

            local hit = lab.try(function()
                return physics3d.raycast(0, 8, 0, 0, -1, 0, 20)
            end)
            t:ok(hit == nil or entity.is_alive(hit), "the cast returns nil or a live entity")

            t:ok(ids.prop ~= nil, "Prop3d exists in the scene")
            t:eq(entity.is_alive(ids.prop), true, "Prop3d is alive")
            local prop = transform.get(ids.prop)
            t:eq(prop.space, "3d", "Prop3d keeps its 3d transform")
            t:near(prop.y, 0.6, 0.01, "Prop3d sits on the ground plane")
        end,
    },
}

-- ---------------------------------------------------------------------------
-- 生命周期
-- ---------------------------------------------------------------------------

function M.on_create()
    find_ids()
    lab.start({
        scene = "api_camera",
        title = "Camera API",
        category = "2D",
        steps = steps,
        -- Hud 是屏幕空间 Text（右上角），会压住 shell 的标题/计数，让它让位。
        hide_scene_text = { "Hud" },
    })
    log.info("api_camera: lab ready (follow / screen-world / 3d chain / selection / raycast)")
end

-- 按步骤分支的字母交互（Lab 保留 space/方向键/1..9/R/F/F5，这里不碰）。
function M.update()
    local step = lab.step()
    if step then
        if step.id == "follow_zoom" then
            if input.just_pressed("keyg") then
                local ok = ids.runner and camera2d.follow(ids.runner, { dead_zone_x = 40, dead_zone_y = 30, smoothing = 6 })
                obs.follow = true
                obs.op = "follow(Runner)=" .. tostring(ok)
            elseif input.just_pressed("keyh") then
                camera2d.unfollow()
                obs.follow = false
                obs.op = "unfollow()"
            elseif input.just_pressed("keyz") then
                local zoom = math.max(0.25, camera2d.get_zoom() - 0.25)
                camera2d.set_zoom(zoom)
                obs.op = string.format("set_zoom(%.2f)", zoom)
            elseif input.just_pressed("keyv") then
                local zoom = math.min(4.0, camera2d.get_zoom() + 0.25)
                camera2d.set_zoom(zoom)
                obs.op = string.format("set_zoom(%.2f)", zoom)
            elseif input.just_pressed("keyx") then
                camera2d.shake(6, 0.3)
                obs.op = "shake(6, 0.3)"
            elseif input.just_pressed("keyc") then
                camera2d.set(HOME.x, HOME.y)
                obs.op = "set(640, 360)"
            end
        elseif step.id == "camera_selection" then
            if input.just_pressed("keyc") then
                obs.op = "set_active(main)=" .. tostring(camera.set_active(ids.main))
            elseif input.just_pressed("keyt") then
                obs.op = "set_active(top)=" .. tostring(camera.set_active(ids.top))
            elseif input.just_pressed("keyy") then
                obs.op = "set_active(2d)=" .. tostring(camera.set_active(ids.cam2d))
            elseif input.just_pressed("keyz") or input.just_pressed("keyv") then
                local delta = input.just_pressed("keyz") and -5 or 5
                local view = camera.get(ids.main)
                camera.set(ids.main, view.x, view.y, view.z, view.tx, view.ty, view.tz, clamp(view.fov + delta, 20, 100))
                obs.op = string.format("fov %.0f", clamp(view.fov + delta, 20, 100))
            end
            obs.active = camera.get_active()
        elseif step.id == "raycast" then
            if input.just_pressed("keyb") then
                local hit = lab.try(function()
                    return physics3d.raycast(0, 5, 0, 0, -1, 0, 20)
                end)
                obs.op = "cast -> " .. tostring(hit)
            end
        end
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 动态 UI 是自己画的：先清图；再把相机选择/控制器交回默认状态，最后交回 shell。
    -- 复位走 `lab.try`：场景切换时实体可能已经先一步失效，清理不应该反过来卡住 lab.stop。
    clear_ui()
    lab.try(function()
        if ids.main then
            camera.set(ids.main, MAIN.x, MAIN.y, MAIN.z, MAIN.tx, MAIN.ty, MAIN.tz, MAIN.fov)
            camera.set_active(ids.main)
        end
        camera2d.unfollow()
        camera2d.set(HOME.x, HOME.y)
        camera2d.set_zoom(1.0)
        camera2d.shake(0, 0.001)
        return true
    end)
    lab.stop()
end

return M
