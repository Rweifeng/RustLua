-- unified：同一个世界里同时摆着 2D 精灵、一等矩形、glTF 网格、粒子与两投影相机。
--
-- 场景 `scenes/unified.scene` 里的每一类内容都按名字找：
--   CameraOrtho   正交相机（Camera2D 那侧的来源）    CameraPersp  透视相机（camera.* 的目标）
--   SunLight / FillLight  两盏灯（light.set_active 在它们之间选一盏）
--   GroundPlane / TriMesh  内置地面哨兵与真实 glTF（mesh.set/get/clear 的目标）
--   HeroSprite（Sprite）/ ShadowQuad（Quad）/ WorldLabel（世界文字）/ Sparks（连续粒子）
--
-- 本 Lab 想证明的是"同世界、两投影"这件事本身：
--   ① 2D 精灵、一等矩形与 3D 网格共享同一份世界（都在场景里活着，按名字能取到）；
--   ② `mesh.get/set/clear` 换的是渲染器的网格，读回直接给路径，clear 之后是 nil；
--   ③ `light.set` 写方向/颜色/强度，`light.get` 读回同一组数字；
--   ④ `camera.set_active` 是一条选择链：选透视相机服务 3D pass，选正交相机服务 2D pass；
--      `camera.get_active` 读回的就是刚选中的那个实体 id。
--
-- 交互键（不碰框架保留键）：C 切换两投影相机  G 循环网格  L 切换被选中的灯  K 换灯色
--
-- 自检约定：每一步的 `enter` 先 `reset_world()`（网格/灯光/相机/选中项全部回到确定值），
-- 因此读者切完相机再按 F5 也全绿；相机读写都是 ECS 里的组件值，不依赖渲染帧。
local lab = require("lab")

local M = {}

-- 实体名（与 `scenes/unified.scene` 一致）。
local CAM_ORTHO = "CameraOrtho"
local CAM_PERSP = "CameraPersp"
local SUN = "SunLight"
local FILL = "FillLight"
local TRI = "TriMesh"
local HERO = "HeroSprite"
local SHADOW = "ShadowQuad"
local SPARKS = "Sparks"

-- mesh.set 的两条合法路径：`.glb/.gltf` 或内置地面哨兵。
local TRIANGLE_MESH = "assets/models/m8-test-triangle.glb"
local GROUND_MESH = "__ground_plane__"

-- 透视相机的"归位"姿态：位置、目标与视场角都写全（camera.set 的 target 是必填参数）。
local CAM_HOME = { x = 5.5, y = 3.5, z = 8.0, tx = 0.0, ty = 1.0, tz = 0.0, fov = 55.0 }

-- 运行期状态（`enter` 负责复位）。
local mesh_state = 0 -- 0=三角 glTF，1=内置地面，2=没有 MeshRenderer
local light_side = "sun"
local palette = 1
local cleared_nil = false
local camera_ok = false

--- `entity.find` + 存活校验：缺实体时返回 nil，调用方据此跳过而不是让 API 报错。
local function live(name)
    local id = entity.find(name)
    if id and entity.is_alive(id) then
        return id
    end
    return nil
end

local function apply_mesh(state)
    local tri = live(TRI)
    if not tri then
        return
    end
    if state == 1 then
        mesh.set(tri, GROUND_MESH)
    elseif state == 2 then
        mesh.clear(tri)
    else
        mesh.set(tri, TRIANGLE_MESH)
    end
    mesh_state = state
end

local function apply_sun(tint)
    local sun = live(SUN)
    if sun then
        -- light.set(id, dx, dy, dz, r, g, b, intensity)；dy 恒为负 ⇒ 光一直往下照。
        light.set(sun, 0.40, -0.85, -0.45, tint[1], tint[2], tint[3], 1.2)
    end
end

local function reset_world()
    mesh_state = 0
    light_side = "sun"
    palette = 1
    cleared_nil = false
    camera_ok = false

    apply_mesh(0)
    apply_sun({ 1.00, 0.96, 0.88 })
    local fill = live(FILL)
    if fill then
        light.set(fill, -0.60, -0.70, 0.50, 0.45, 0.60, 0.95, 0.5)
    end
    local sun = live(SUN)
    if sun then
        light.set_active(sun)
    end

    local persp = live(CAM_PERSP)
    if persp then
        camera.set(persp, CAM_HOME.x, CAM_HOME.y, CAM_HOME.z, CAM_HOME.tx, CAM_HOME.ty, CAM_HOME.tz, CAM_HOME.fov)
        camera_ok = camera.set_active(persp)
    end
end

local steps = {
    {
        id = "one_world",
        title = "One world",
        claim = "同一份场景里 2D 精灵、一等矩形、glTF 网格与粒子都在；它们共享一份世界，不是两层画面",
        api = { "entity.find", "entity.is_alive", "mesh.get", "sprite.get", "quad.get" },
        code = "entity.find('HeroSprite'); mesh.get(tri); sprite.get(hero); quad.get(shadow)",
        enter = function()
            reset_world()
        end,
        run = function()
            lab.note("2D sprite + quad、3D mesh、world text 与 particles 同时在场景里")
        end,
        read = function()
            local tri = live(TRI)
            local hero = live(HERO)
            local shadow = live(SHADOW)
            return {
                mesh = tri and tostring(mesh.get(tri)) or "missing",
                sprite = hero and tostring(sprite.get(hero).texture) or "missing",
                quad_w = shadow and string.format("%.2f", quad.get(shadow).size_x) or "-",
                sparks = tostring(live(SPARKS) ~= nil),
            }
        end,
        check = function(t)
            local hero = live(HERO)
            local shadow = live(SHADOW)
            local tri = live(TRI)
            t:ok(hero ~= nil and entity.is_alive(hero), "HeroSprite lives in the same world")
            t:ok(shadow ~= nil and entity.is_alive(shadow), "ShadowQuad lives in the same world")
            t:ok(tri ~= nil and entity.is_alive(tri), "TriMesh lives in the same world")
            t:eq(mesh.get(tri), TRIANGLE_MESH, "mesh.get returns the glTF path")
            t:ok(
                hero ~= nil and tostring(sprite.get(hero).texture):find("player.png") ~= nil,
                "sprite.get returns the sprite texture"
            )
            t:near(shadow and quad.get(shadow).size_x or 0, 1.8, 0.01, "quad.get returns its width")
        end,
    },
    {
        id = "mesh_swap",
        title = "Mesh swap",
        claim = "mesh.set 在三角 glTF / 内置地面之间换，mesh.clear 之后 mesh.get 返回 nil（不是空串）",
        api = { "mesh.set", "mesh.get", "mesh.clear", "entity.find", "entity.is_alive" },
        code = "mesh.set(tri, ground); mesh.clear(tri); mesh.get(tri) == nil; mesh.set(tri, gltf)",
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            mesh.set(tri, GROUND_MESH)
            local removed = mesh.clear(tri)
            cleared_nil = (removed == true) and (mesh.get(tri) == nil)
            mesh.set(tri, TRIANGLE_MESH)
        end,
        run = function()
            local tri = live(TRI)
            lab.note("mesh.get = " .. (tri and tostring(mesh.get(tri)) or "missing"))
        end,
        read = function()
            local tri = live(TRI)
            return {
                mesh = tri and tostring(mesh.get(tri)) or "missing",
                cleared_nil = tostring(cleared_nil),
                state = mesh_state,
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            t:eq(mesh.get(tri), TRIANGLE_MESH, "mesh.set restored the glTF path")
            t:eq(cleared_nil, true, "mesh.clear made mesh.get return nil")
        end,
    },
    {
        id = "sun_and_fill",
        title = "Sun / fill",
        claim = "light.set 写方向/颜色/强度、light.get 读回同一组数字；L 在两盏灯间切 light.set_active",
        api = { "light.set", "light.get", "light.set_active", "entity.find", "entity.is_alive" },
        code = "light.set(sun, 0.4, -0.85, -0.45, 1, 0.96, 0.88, 1.2); light.get(sun)",
        enter = function()
            reset_world()
            local sun = live(SUN)
            if sun then
                light.set(sun, 0.40, -0.85, -0.45, 1.00, 0.96, 0.88, 1.2)
            end
        end,
        run = function()
            local sun = live(SUN)
            if sun then
                local value = light.get(sun)
                lab.note(
                    string.format(
                        "sun dir=(%.2f,%.2f,%.2f) rgb=(%.2f,%.2f,%.2f) i=%.2f",
                        value.dx,
                        value.dy,
                        value.dz,
                        value.r,
                        value.g,
                        value.b,
                        value.intensity
                    )
                )
            end
        end,
        read = function()
            local sun = live(SUN)
            local fill = live(FILL)
            return {
                sun = sun and light.get(sun).intensity or "-",
                fill = fill and light.get(fill).intensity or "-",
                active = light_side,
            }
        end,
        check = function(t)
            local sun = live(SUN)
            if not sun then
                t:ok(false, "SunLight is required")
                return
            end
            local value = light.get(sun)
            t:near(value.intensity, 1.2, 1e-3, "intensity reads back")
            t:near(value.r, 1.0, 1e-3, "red channel reads back")
            t:near(value.dy, -0.85, 1e-3, "direction reads back")
        end,
    },
    {
        id = "perspective_camera",
        title = "Perspective",
        claim = "透视相机是 camera.get/set 的目标：写入位置/目标/视场角后读回同一组数字",
        api = { "camera.set", "camera.get", "camera.set_active", "camera.get_active", "entity.find", "entity.is_alive" },
        code = "camera.set(persp, 5.5, 3.5, 8, 0, 1, 0, 55); camera.get(persp)",
        enter = function()
            reset_world()
            local persp = live(CAM_PERSP)
            if persp then
                camera.set(persp, CAM_HOME.x, CAM_HOME.y, CAM_HOME.z, CAM_HOME.tx, CAM_HOME.ty, CAM_HOME.tz, CAM_HOME.fov)
                camera_ok = camera.set_active(persp)
            end
        end,
        run = function()
            local persp = live(CAM_PERSP)
            if persp then
                local value = camera.get(persp)
                lab.note(
                    string.format(
                        "persp pos=(%.1f,%.1f,%.1f) fov=%.0f active=%s",
                        value.x,
                        value.y,
                        value.z,
                        value.fov,
                        tostring(camera.get_active())
                    )
                )
            end
        end,
        read = function()
            local persp = live(CAM_PERSP)
            local value = persp and camera.get(persp) or nil
            return {
                active = tostring(camera.get_active()),
                x = value and string.format("%.1f", value.x) or "-",
                fov = value and string.format("%.0f", value.fov) or "-",
            }
        end,
        check = function(t)
            local persp = live(CAM_PERSP)
            if not persp then
                t:ok(false, "CameraPersp is required")
                return
            end
            t:eq(camera.get_active(), persp, "get_active reads the selected perspective camera")
            local value = camera.get(persp)
            t:near(value.x, CAM_HOME.x, 1e-3, "camera position reads back")
            t:near(value.fov, CAM_HOME.fov, 1e-3, "fov reads back")
            t:eq(camera_ok, true, "camera.set_active returned true")
        end,
    },
    {
        id = "ortho_camera",
        title = "Ortho pass",
        claim = "同一条选择链也能选正交相机（它服务 2D pass）；camera.get 只认透视，对正交会报错",
        api = { "camera.set_active", "camera.get_active", "camera.get", "entity.find", "entity.is_alive" },
        code = "camera.set_active(ortho); camera.get_active(); camera.get(ortho) -- error",
        enter = function()
            reset_world()
            local ortho = live(CAM_ORTHO)
            local persp = live(CAM_PERSP)
            if ortho then
                camera_ok = camera.set_active(ortho)
            end
            if persp then
                -- 场景里两台相机都在：正交只负责 2D 取景，3D 视图仍由透视相机渲染。
                camera.set(persp, CAM_HOME.x, CAM_HOME.y, CAM_HOME.z, CAM_HOME.tx, CAM_HOME.ty, CAM_HOME.tz, CAM_HOME.fov)
            end
        end,
        run = function()
            local ortho = live(CAM_ORTHO)
            local value, err = lab.try(camera.get, ortho)
            lab.note(
                string.format(
                    "active=%s camera.get(ortho)=%s",
                    tostring(camera.get_active()),
                    value and "value" or tostring(err)
                )
            )
        end,
        read = function()
            local ortho = live(CAM_ORTHO)
            local value, err = lab.try(camera.get, ortho)
            return {
                active = tostring(camera.get_active()),
                ortho = tostring(ortho),
                get_ok = tostring(value ~= nil),
                error = value == nil and tostring(err) or "-",
            }
        end,
        check = function(t)
            local ortho = live(CAM_ORTHO)
            if not ortho then
                t:ok(false, "CameraOrtho is required")
                return
            end
            t:eq(camera.get_active(), ortho, "get_active reads the selected orthographic camera")
            t:eq(camera_ok, true, "camera.set_active accepted the orthographic camera")
            local value, err = lab.try(camera.get, ortho)
            t:ok(value == nil and err ~= nil, "camera.get rejects an orthographic camera")
            t:ok(live(HERO) ~= nil, "2D sprite is still in the same world")
        end,
    },
    {
        id = "two_projections",
        title = "Both projections",
        claim = "切回透视：两投影相机各服务一条 pass，2D/3D 内容都在同一世界里活着",
        api = {
            "camera.set_active",
            "camera.get_active",
            "camera.get",
            "entity.find",
            "entity.is_alive",
            "mesh.get",
            "light.set",
        },
        code = "camera.set_active(persp); camera.get(persp); mesh.get(tri)",
        enter = function()
            reset_world()
            local persp = live(CAM_PERSP)
            if persp then
                camera.set(persp, 6.2, 3.8, 7.4, CAM_HOME.tx, CAM_HOME.ty, CAM_HOME.tz, CAM_HOME.fov)
                camera_ok = camera.set_active(persp)
            end
            apply_mesh(0)
            apply_sun({ 0.90, 0.85, 1.00 })
        end,
        run = function()
            lab.note("active=" .. tostring(camera.get_active()) .. " mesh=" .. tostring(live(TRI) and mesh.get(live(TRI)) or "-"))
        end,
        read = function()
            local persp = live(CAM_PERSP)
            local value = persp and camera.get(persp) or nil
            return {
                active = tostring(camera.get_active()),
                x = value and string.format("%.1f", value.x) or "-",
                hero = tostring(live(HERO) ~= nil),
                sparks = tostring(live(SPARKS) ~= nil),
            }
        end,
        check = function(t)
            local persp = live(CAM_PERSP)
            local tri = live(TRI)
            if not persp or not tri then
                t:ok(false, "CameraPersp and TriMesh are required")
                return
            end
            t:eq(camera.get_active(), persp, "perspective camera is selected again")
            t:near(camera.get(persp).x, 6.2, 1e-3, "camera.set wrote the orbit position")
            t:eq(mesh.get(tri), TRIANGLE_MESH, "3D mesh is back to the glTF")
            t:ok(live(HERO) ~= nil, "2D sprite is still alive")
            t:ok(live(SPARKS) ~= nil, "continuous particle emitter is still alive")
        end,
    },
}

function M.on_create()
    lab.start({
        scene = "unified",
        title = "Unified",
        category = "3D",
        steps = steps,
        -- 屏幕 Title/Note 是场景 Text，画在动态 UI 之上，会压住页眉，交给 shell 让位。
        hide_scene_text = { "ScreenTitle", "ScreenNote" },
    })
end

function M.update()
    -- 字母键交互：C 在两投影相机间切，G 循环网格，L 切灯，K 换灯色。
    if input.just_pressed("c") then
        local persp = live(CAM_PERSP)
        local ortho = live(CAM_ORTHO)
        local active = camera.get_active()
        if active == persp and ortho then
            camera.set_active(ortho)
        elseif persp then
            camera.set_active(persp)
        end
    end
    if input.just_pressed("g") then
        apply_mesh((mesh_state + 1) % 3)
    end
    if input.just_pressed("l") then
        local target = light_side == "sun" and live(FILL) or live(SUN)
        if target then
            light.set_active(target)
            light_side = light_side == "sun" and "fill" or "sun"
        end
    end
    if input.just_pressed("k") then
        local tints = {
            { 1.00, 0.96, 0.88 },
            { 1.00, 0.72, 0.50 },
            { 0.62, 0.78, 1.00 },
        }
        palette = palette % #tints + 1
        apply_sun(tints[palette])
    end
    return lab.tick()
end

function M.self_test()
    return lab.self_test()
end

function M.on_destroy()
    -- 恢复本模块改过的网格/灯光/相机选择，再拆 shell。
    apply_mesh(0)
    local sun = live(SUN)
    if sun then
        light.set_active(sun)
    end
    local persp = live(CAM_PERSP)
    if persp then
        camera.set_active(persp)
    end
    lab.stop()
end

return M
