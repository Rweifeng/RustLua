-- unified：**一份场景同时表达全部渲染类别**时的 3D 侧 Lua API 演示。
--
-- 场景 `scenes/unified.scene` 里每一类内容分别落在哪个实体上（脚本按名字找它们）：
--   CameraOrtho   正交相机（projection=orthographic）⇒ 运行时的**初始 2D 相机**（camera2d 那侧）
--   CameraPersp   透视相机（projection=perspective）⇒ `camera.*` 那侧唯一的 Camera3D，
--                 因此也是 `camera.get` / `camera.set` / `camera.set_active` 唯一的目标
--   SunLight      太阳方向光（`direction` 的 y 为负 = 光往下照，写成正的只剩环境光）
--   FillLight     第二盏灯 ⇒ `light.set_active` 在"太阳 / 补光"之间切换
--   GroundPlane   mesh=__ground_plane__（内置地面哨兵）
--   TriMesh       mesh=assets/models/m8-test-triangle.glb（真实 glTF）——运行期换网格的目标
--   HeroSprite    2D 精灵，但写在**世界坐标**上（`RenderMode mode=world` 请求世界通道，
--                 于是它和 3D 网格共用同一份深度缓冲：2D 与 3D 是同一个世界）
--   ShadowQuad    一等矩形（不写 texture ⇒ `color` 就是最终颜色）
--   WorldLabel    世界文字      Sparks  连续粒子（rate=24 > 0）    ScreenTitle/ScreenNote 屏幕 UI
--
-- 本模块演示的 API：camera.set / camera.get / camera.set_active / camera.get_active、
-- mesh.set / mesh.get / mesh.clear、light.set / light.get / light.set_active。
--
-- 按键与"看到什么"：
--   SPACE  `camera.set_active(CameraPersp)` 选中 3D 相机；之后 `camera.set` 让透视相机**绕场景
--          转圈**（每 2 秒换一个位置），状态行里的 `camera.get` 是**读回**的 x,y,z / tx,ty,tz /
--          fov,near,far——读回值就是渲染真正用的那台相机的值（不是脚本的自述）。
--          正交相机**不是** `camera.set_active` 的目标：它是 Camera2D，没有 Camera3D 组件，
--          选中它会报 "entity has no camera3d"——这正是脚本必须先把两种相机分清楚的原因。
--   1/2/3  运行期给 TriMesh 换网格：三角 glTF → 内置地面 → 不挂 MeshRenderer（实体从 3D 视图
--          消失）。`mesh.get` 读回的路径直接打进状态行；`mesh.clear` 之后它返回 **nil**（不是空串）。
--   L      在 SunLight / FillLight 之间 `light.set_active`：画面上明暗与色调立刻换一套。
--   K      换太阳/补光的颜色（`light.set`）。
--   U/D    手动把透视相机推近/拉远：先 `camera.get` 读回当前姿态，再 `camera.set` 写回 raduis。
local api = require("api_demo")

local M = {}

-- 演示常量（顶层只放常量与函数，引擎 API 一律留到 on_create/update 调用，README 硬规则 1）。
local CAM_ORTHO = "CameraOrtho"
local CAM_PERSP = "CameraPersp"
local SUN = "SunLight"
local FILL = "FillLight"
local TRI = "TriMesh"
local ORBIT_RADIUS = 9.6 -- 绕场景转圈的水平半径（世界单位）
local ORBIT_HEIGHT = 3.4
local TARGET_X = 0.0
local TARGET_Y = 1.0
local TARGET_Z = 0.0
local ORBIT_STEP = 2.0 -- 秒：每 2 秒把相机挪到轨道上的下一个位置（否则位置每帧都在变，读回值看不清）
local MAX_STEP = 0.1 -- 单帧最多推进多少秒：掉帧时不至于让角度一次跳一大步
local DOLLY_MIN = 4.0
local DOLLY_MAX = 18.0

-- mesh.set 的两条合法路径：`.glb/.gltf` 或内置地面哨兵（别的后缀会被 `mesh.set` 直接拒掉）。
local TRIANGLE_MESH = "assets/models/m8-test-triangle.glb"
local GROUND_MESH = "__ground_plane__"

-- `light.set` 每次换一套颜色，画面上立刻能看出区别（暖白 / 夕照 / 冷光）。
local LIGHT_TINTS = {
    { 1.00, 0.96, 0.88 },
    { 1.00, 0.72, 0.50 },
    { 0.62, 0.78, 1.00 },
}

-- 运行期状态。
local elapsed = 0
local angle = 0.0
local last_step = -1
local orbit = true
local radius = ORBIT_RADIUS
local tint = 0
local tint_palette = 1
local mesh_state = 0 -- 0=三角 glTF，1=内置地面，2=没有 MeshRenderer
local light_switch = 0 -- 0=太阳被选中，1=补光被选中
local frames = 0
local last_error = "-"

--- `entity.find` + 存活校验：缺实体时返回 nil，调用方据此跳过。
--- （`camera.*` / `mesh.*` / `light.*` 在实体缺组件时是**报 Lua 错误**，不是返回 nil，
--- 所以每个调用点都先过这一关。）
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

--- 按键：切镜头（`camera.set_active` → `camera.get_active` 读回确认）。
local function activate_perspective()
    local cam = live(CAM_PERSP)
    if not cam then
        last_error = "no " .. CAM_PERSP .. " entity"
        return
    end
    local ok, err = api.try(camera.set_active, cam)
    if not ok then
        last_error = tostring(err)
        return
    end
    last_error = "-"
    -- 读回：`camera.get_active()` 返回被选中的实体 id（没有选中时是 nil）。
    log.info(
        string.format("unified: camera.set_active(%d) -> get_active=%s", cam, tostring(camera.get_active()))
    )
end

--- 按键：运行期换网格（`mesh.set` / `mesh.get` / `mesh.clear`）。
local function set_mesh_state(state)
    local tri = live(TRI)
    if not tri then
        last_error = "no " .. TRI .. " entity"
        return
    end
    local ok, err
    if state == 0 then
        -- clear 之后 `mesh.get` 返回 nil，因此这一步是"重新挂上 MeshRenderer"。
        ok, err = api.try(mesh.set, tri, TRIANGLE_MESH)
    elseif state == 1 then
        ok, err = api.try(mesh.set, tri, GROUND_MESH)
    else
        ok, err = api.try(mesh.clear, tri)
    end
    if not ok then
        last_error = tostring(err)
        return
    end
    mesh_state = state
    last_error = "-"
    log.info(string.format("unified: mesh_state=%d mesh.get=%s", mesh_state, tostring(mesh.get(tri))))
end

--- 按键：在两盏灯之间切"被选中的那盏"（`light.set_active`）。
local function switch_active_light()
    local target
    if light_switch == 0 then
        target = live(FILL)
    else
        target = live(SUN)
    end
    if not target then
        last_error = "no light entity"
        return
    end
    local ok, err = api.try(light.set_active, target)
    if not ok then
        last_error = tostring(err)
        return
    end
    light_switch = 1 - light_switch
    last_error = "-"
    log.info("unified: light.set_active -> " .. tostring(target))
end

--- 每帧：太阳方向缓慢旋转 + 调色板换色（`light.set` 写、`light.get` 读）。
local function step_lights(seconds)
    local sun = live(SUN)
    if sun then
        local a = seconds * 0.25
        local dx = 0.40 + math.cos(a) * 0.35
        local dz = -0.45 + math.sin(a) * 0.35
        local c = LIGHT_TINTS[tint + 1]
        -- light.set(id, dx, dy, dz, r, g, b, intensity)：dy 恒为负 ⇒ 光一直往下照。
        api.try(light.set, sun, dx, -0.85, dz, c[1], c[2], c[3], 1.2)
    end
    local fill = live(FILL)
    if fill then
        local f = LIGHT_TINTS[tint_palette + 1]
        api.try(light.set, fill, -0.60, -0.70, 0.50, f[1], f[2], f[3], 0.5)
    end
end

--- 状态行用：`camera.get` 读回透视相机的姿态与投影参数（不依赖渲染帧，headless 也能读）。
local function camera_text()
    local cam = live(CAM_PERSP)
    if not cam then
        return "camera.get=(no " .. CAM_PERSP .. ")"
    end
    local c, err = api.try(camera.get, cam)
    if not c then
        return "camera.get failed: " .. tostring(err)
    end
    return string.format(
        "id=%d pos=(%.1f,%.1f,%.1f) target=(%.1f,%.1f,%.1f) fov=%.0f near=%.2f far=%.0f",
        cam,
        c.x,
        c.y,
        c.z,
        c.tx,
        c.ty,
        c.tz,
        c.fov,
        c.near,
        c.far
    )
end

--- 状态行用：`mesh.get` 读回网格路径（clear 之后是 nil，正是要看见的那个事实）。
local function mesh_text()
    local tri = live(TRI)
    if not tri then
        return "mesh.get=(no " .. TRI .. ")"
    end
    local path, err = api.try(mesh.get, tri)
    if path == nil then
        if err then
            return "mesh.get failed: " .. tostring(err)
        end
        return "nil (mesh.clear 移除了 MeshRenderer)"
    end
    return tostring(path)
end

--- 状态行用：`light.get` 读回被选中那盏灯的方向/颜色/强度。
local function light_text()
    local id
    if light_switch == 0 then
        id = live(SUN)
    else
        id = live(FILL)
    end
    if not id then
        return "light.get=(no light entity)"
    end
    local l, err = api.try(light.get, id)
    if not l then
        return "light.get failed: " .. tostring(err)
    end
    return string.format(
        "%s dir=(%.2f,%.2f,%.2f) rgb=(%.2f,%.2f,%.2f) i=%.2f",
        light_switch == 0 and SUN or FILL,
        l.dx,
        l.dy,
        l.dz,
        l.r,
        l.g,
        l.b,
        l.intensity
    )
end

--- 进入场景：只做一次的事（找实体、打印一条开局事实）。
function M.on_create()
    api.title("unified - 2D + 3D + UI in one scene: camera.set/get/set_active, mesh.*, light.*")
    api.hint(
        "SPACE camera.set_active orbit  1/2/3 mesh.set/get/clear  L light.set_active  "
            .. "K light color  U/D camera dolly  " .. api.KEYS_BACK
    )
    -- 正交相机只在这里点一次名：它是运行时的初始 2D 相机（Camera2D），
    -- 不是 `camera.*` 的目标——这条边界写进日志，免得有人拿它去 `camera.set_active`。
    log.info(
        string.format(
            "unified: 2D camera entity=%s (Camera2D), 3D camera entity=%s (Camera3D)",
            tostring(entity.find(CAM_ORTHO)),
            tostring(entity.find(CAM_PERSP))
        )
    )
end

--- 每个固定步一次：按键 → 改状态；读回 → 进状态行。
function M.update()
    frames = frames + 1
    local delta = time.delta()
    if type(delta) ~= "number" then
        delta = 0
    end
    local dt = math.min(delta, MAX_STEP)
    elapsed = elapsed + dt
    angle = angle + dt * 0.35

    -- ── `camera.set_active` / `camera.get_active`（按键触发；headless 下没有输入，因此不进这些分支）。
    if input.just_pressed("space") then
        orbit = not orbit
        activate_perspective()
    end
    if input.just_pressed("1") then
        set_mesh_state(0)
    elseif input.just_pressed("2") then
        set_mesh_state(1)
    elseif input.just_pressed("3") then
        set_mesh_state(2)
    end
    if input.just_pressed("l") then
        switch_active_light()
    end
    if input.just_pressed("k") then
        tint = (tint + 1) % #LIGHT_TINTS
        tint_palette = (tint_palette + 1) % #LIGHT_TINTS
        log.info("unified: light.set color palette " .. tostring(tint + 1))
    end

    -- ── `camera.set` / `camera.get`：绕场景转圈，并把**读回**的姿态打进状态行。
    local cam = live(CAM_PERSP)
    if cam then
        if input.is_down("u") then
            radius = math.max(radius - 6.0 * dt, DOLLY_MIN)
            last_step = -1 -- 强制本帧写一次位置（否则要等下一个 ORBIT_STEP 才生效）
        elseif input.is_down("d") then
            radius = math.min(radius + 6.0 * dt, DOLLY_MAX)
            last_step = -1
        end
        local step = math.floor(elapsed / ORBIT_STEP)
        if orbit and step ~= last_step then
            last_step = step
            local x = TARGET_X + math.cos(angle) * radius
            local z = TARGET_Z + math.sin(angle) * radius
            -- 只给位置：target 与 fov 省略 ⇒ 保持场景声明的值（`camera.set` 的后三个参数可选）。
            api.try(camera.set, cam, x, ORBIT_HEIGHT, z)
        end
    end

    -- ── `light.set` / `light.get`：太阳方向随时间缓慢旋转，颜色随词盘 K 变化。
    step_lights(elapsed)

    api.status(string.format("frame %d | camera.get %s | mesh.get %s", frames, camera_text(), mesh_text()))
    api.hint(string.format("light.get %s | last error %s", light_text(), last_error))
    -- 返回值进引擎日志（`*_ok` 前缀与其它演示一致，便于 grep）。
    return string.format("unified_ok frame=%d cam=%s mesh=%s", frames, tostring(cam), mesh_text())
end

--- 离开场景：动态 UI（api.title/status/hint）由 `api.clear` 负责删掉。
function M.on_destroy()
    api.clear()
    log.info("unified: demo left")
end

return M
