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
local EMBER = "EmberLight"
local TRI = "TriMesh"
local HERO = "HeroSprite"
local SHADOW = "ShadowQuad"
local SPARKS = "Sparks"

-- mesh.set 的两条合法路径：`.glb/.gltf` 或内置地面哨兵。
local TRIANGLE_MESH = "assets/models/m8-test-triangle.glb"
local SPHERE_MESH = "assets/models/p3-sphere.glb"
local GROUND_MESH = "__ground_plane__"

-- 透视相机的"归位"姿态：位置、目标与视场角都写全（camera.set 的 target 是必填参数）。
local CAM_HOME = { x = 5.5, y = 3.5, z = 8.0, tx = 0.0, ty = 1.0, tz = 0.0, fov = 55.0 }

-- 运行期状态（`enter` 负责复位）。
local mesh_state = 0 -- 0=三角 glTF，1=内置地面，2=没有 MeshRenderer
local light_side = "sun"
local palette = 1
local cleared_nil = false
local camera_ok = false
local batch_count = 0
local cleared_count = 0
local motion_count = 0
local motion_pose = nil
local patch_replaced = 0
local patch_count = 0
local patch_x = nil
local patch_zero = nil
local patch_oob = nil
local lod_mesh = nil
local lod_distance = nil
local lod_far_mesh = nil
local lod_far_distance = nil
local lod_cleared = nil
local lod_bad_err = nil
local lod_same_err = nil
local lod_far_pair_err = nil
local lod_far_order_err = nil
local planar_default = nil
local planar_off = nil
local planar_live = nil
local planar_no_batch_err = nil

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
        claim = "mesh.set 在三角 glTF / 内置地面之间换，mesh.clear 之后 mesh.get 返回 nil；mesh.set_material 覆盖/清除材质",
        api = { "mesh.set", "mesh.get", "mesh.clear", "mesh.set_material", "entity.find", "entity.is_alive" },
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
            -- 逐实例材质覆盖：先换成示例工程里的 blend 材质，再清回 glTF 内嵌。
            mesh.set_material(tri, "assets/materials/p3-r02-alpha-blend-a.material")
            mesh.set_material(tri, nil)
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
        id = "crowd_batch",
        title = "Crowd batch",
        claim = "mesh.set_batch 把 N 份实例变换收进**一个**实体（渲染侧一次实例化绘制）；mesh.batch_count 读回实例数，mesh.clear_batch 摘掉批次",
        api = { "mesh.set_batch", "mesh.batch_count", "mesh.clear_batch", "entity.find", "entity.is_alive" },
        code = "mesh.set_batch(tri, ground, {{x=0,y=0,z=0},{x=2,y=0,z=0,yaw=1.57}}); mesh.batch_count(tri)",
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            -- 两份实例摆位：一份在原点、一份偏 2 米并转 90°。姿态给 yaw（弧度，绕 +Y），
            -- 缩放给 sx/sy/sz；也可以给 q={x,y,z,w} 用完整四元数（给了 q 就以 q 为准）。
            mesh.set_batch(tri, GROUND_MESH, {
                { x = 0.0, y = 0.0, z = 0.0, yaw = 0.0 },
                { x = 2.0, y = 0.0, z = 0.0, yaw = 1.5707963, sx = 0.5, sy = 0.5, sz = 0.5 },
            })
            batch_count = mesh.batch_count(tri)
            mesh.clear_batch(tri)
            cleared_count = mesh.batch_count(tri)
        end,
        run = function()
            local tri = live(TRI)
            lab.note("mesh.batch_count = " .. tostring(batch_count) .. "（clear 之后 " .. tostring(cleared_count) .. "）")
        end,
        read = function()
            return {
                batch_count = tostring(batch_count),
                cleared_count = tostring(cleared_count),
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            t:eq(batch_count, 2, "mesh.set_batch stored two instances")
            t:eq(cleared_count, 0, "mesh.clear_batch removed the batch")
            t:eq(mesh.batch_count(tri), 0, "batch_count reads back zero after clear")
        end,
    },
    {
        id = "crowd_batch_patch",
        title = "Crowd batch range patch",
        claim = "mesh.set_batch_range 只解析并替换从 start 起的一段实例：批次数量与其余实例都不动；越界段直接报错（不扩批、不静默）",
        api = { "mesh.set_batch", "mesh.set_batch_range", "mesh.batch_count", "mesh.batch_pose", "entity.find", "entity.is_alive" },
        code = "mesh.set_batch(tri, ground, {{x=0},{x=2},{x=4}}); mesh.set_batch_range(tri, 1, {{x=9}}); mesh.batch_pose(tri, 1)",
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            mesh.set_batch(tri, GROUND_MESH, {
                { x = 0.0, y = 0.0, z = 0.0 },
                { x = 2.0, y = 0.0, z = 0.0 },
                { x = 4.0, y = 0.0, z = 0.0 },
            })
            patch_replaced = mesh.set_batch_range(tri, 1, {
                { x = 9.0, y = 0.0, z = 0.0 },
            })
            patch_count = mesh.batch_count(tri)
            patch_zero = mesh.batch_pose(tri, 0)
            local pose = mesh.batch_pose(tri, 1)
            patch_x = pose and pose.x
            -- 越界段必须报错：返回非 nil 说明引擎把它当成"扩批"静默接受了。
            -- pcall 的错误值是 userdata（mlua 的 Error 包装），tostring 之后才能查文案。
            patch_oob = tostring((select(2, pcall(mesh.set_batch_range, tri, 3, { { x = 1.0, y = 0.0, z = 0.0 } }))))
        end,
        run = function()
            lab.note("mesh.set_batch_range 替换 " .. tostring(patch_replaced) .. " 只；第 1 只 x="
                .. tostring(patch_x) .. "；越界报错：" .. tostring(patch_oob))
        end,
        read = function()
            return {
                patch_replaced = tostring(patch_replaced),
                patch_count = tostring(patch_count),
                patch_x = tostring(patch_x),
                patch_zero_x = tostring(patch_zero and patch_zero.x),
                patch_oob = tostring(patch_oob),
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            t:eq(patch_replaced, 1, "set_batch_range replaced exactly one instance")
            t:eq(patch_count, 3, "range patch kept the batch size")
            t:ok(type(patch_x) == "number" and math.abs(patch_x - 9.0) < 1e-4,
                "patched instance moved to x=9")
            t:ok(type(patch_zero) == "table" and math.abs(patch_zero.x) < 1e-4,
                "instance #0 is untouched")
            t:ok(type(patch_oob) == "string" and patch_oob:find("out of bounds", 1, true) ~= nil,
                "an out-of-range patch is rejected, not silently extended")
        end,
    },
    {
        id = "crowd_motion",

        title = "Crowd motion",
        claim = "mesh.set_batch 的第五个参数是批次级程序化运动：位姿由引擎按路径与场景时钟求值，脚本只写一次静态表；mesh.batch_pose 读回的就是引擎算出来的那一个位姿",
        api = { "mesh.set_batch", "mesh.batch_pose", "mesh.batch_count", "entity.find", "entity.is_alive" },
        code = 'mesh.set_batch(tri, ground, {{s=0, align=true, forward="z"}}, nil, {kind="path", path={{x=10,y=0,z=0},{x=14,y=0,z=0}}, speed=2}); mesh.batch_pose(tri, 0)',
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            -- 路径故意**不从原点起**：带运动时实例表里不许写 x/y/z（写了引擎直接报错），
            -- 于是"退回静态表"的位置只能是 (0,0,0) ⇒ 读回位置落在 10..14 就说明引擎真的
            -- 按路径算了，而不是退回静态。
            mesh.set_batch(tri, GROUND_MESH, {
                { s = 0.0, align = true, forward = "z", sx = 0.5, sy = 0.5, sz = 0.5 },
            }, nil, {
                kind = "path",
                path = {
                    { x = 10.0, y = 0.0, z = 0.0 },
                    { x = 14.0, y = 0.0, z = 0.0 },
                },
                speed = 2.0,
            })
            motion_count = mesh.batch_count(tri)
            motion_pose = mesh.batch_pose(tri, 0)
        end,
        run = function()
            local tri = live(TRI)
            if not tri then
                return
            end
            motion_pose = mesh.batch_pose(tri, 0)
        end,
        read = function()
            return {
                motion_count = tostring(motion_count),
                motion_x = tostring(motion_pose and motion_pose.x),
                motion_yaw = tostring(motion_pose and motion_pose.yaw),
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            t:eq(motion_count, 1, "motion batch stored one instance")
            t:ok(motion_pose ~= nil, "mesh.batch_pose read back a pose")
            if not motion_pose then
                return
            end
            t:ok(
                motion_pose.x >= 10.0 and motion_pose.x <= 14.0,
                "pose is on the path (a static fallback would land at 0)"
            )
            -- 模型前向轴写的是 "+Z 前向" ⇒ 路径沿 +X 走时 yaw = atan2(dx, dz) = π/2。
            t:near(motion_pose.yaw, math.pi * 0.5, 0.01, "yaw follows the tangent (forward = z)")
        end,
    },
    {
        id = "crowd_batch_lod",
        title = "Crowd batch mesh LOD",
        claim = "mesh.set_batch_lod 给实例批次挂远距替代网格链：超过阈值的实例在 GPU 位姿档改画低模，第二台阶再切一张更远的模；mesh.batch_lod 读回同一份组件（含第二台阶），mesh.clear_batch_lod 摘掉；非正/非有限距离、等于批次网格的替代、只给一半的第二台阶、远阈值不大于近阈值都直接报错（不静默落空档）",
        api = { "mesh.set_batch", "mesh.set_batch_lod", "mesh.batch_lod", "mesh.clear_batch_lod", "entity.find", "entity.is_alive" },
        code = "mesh.set_batch(tri, ground, {{x=0},{x=30}}); mesh.set_batch_lod(tri, triangle, 20, sphere, 40); mesh.batch_lod(tri)",
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            -- 两只：一只在原点（近侧），一只在 30 米外（远侧）；阈值 20 ⇒ 远侧那只走替代网格。
            mesh.set_batch(tri, GROUND_MESH, {
                { x = 0.0, y = 0.0, z = 0.0 },
                { x = 30.0, y = 0.0, z = 0.0 },
            })
            mesh.set_batch_lod(tri, TRIANGLE_MESH, 20.0, SPHERE_MESH, 40.0)
            local lod = mesh.batch_lod(tri)
            lod_mesh = lod and lod.mesh
            lod_distance = lod and lod.distance
            lod_far_mesh = lod and lod.far_mesh
            lod_far_distance = lod and lod.far_distance
            -- 0/负/NaN 距离意味着"整批永远近侧"，等于批次网格的替代意味着"远侧其实就是近侧"：
            -- 两种都是静默空档，必须报错。第二台阶的两条护栏同理：只给一半 = 不知道挂多高，
            -- 远阈值不大于近阈值 = 中段永远空档。pcall 的错误值是 userdata，tostring 之后才查文案。
            lod_bad_err = tostring((select(2, pcall(mesh.set_batch_lod, tri, TRIANGLE_MESH, 0.0))))
            lod_same_err = tostring((select(2, pcall(mesh.set_batch_lod, tri, GROUND_MESH, 20.0))))
            lod_far_pair_err = tostring((select(2, pcall(mesh.set_batch_lod, tri, TRIANGLE_MESH, 20.0, SPHERE_MESH))))
            lod_far_order_err = tostring((select(2, pcall(mesh.set_batch_lod, tri, TRIANGLE_MESH, 20.0, SPHERE_MESH, 20.0))))
            mesh.clear_batch_lod(tri)
            lod_cleared = mesh.batch_lod(tri)
        end,
        run = function()
            lab.note("batch_lod = " .. tostring(lod_mesh) .. " @ " .. tostring(lod_distance)
                .. " > " .. tostring(lod_far_mesh) .. " @ " .. tostring(lod_far_distance)
                .. "；clear 之后 " .. tostring(lod_cleared))
        end,
        read = function()
            return {
                lod_mesh = tostring(lod_mesh),
                lod_distance = tostring(lod_distance),
                lod_far_mesh = tostring(lod_far_mesh),
                lod_far_distance = tostring(lod_far_distance),
                lod_cleared = tostring(lod_cleared),
                lod_bad_err = tostring(lod_bad_err),
                lod_same_err = tostring(lod_same_err),
                lod_far_pair_err = tostring(lod_far_pair_err),
                lod_far_order_err = tostring(lod_far_order_err),
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            t:eq(lod_mesh, TRIANGLE_MESH, "batch_lod reads back the middle mesh")
            t:near(lod_distance, 20.0, 1e-4, "batch_lod reads back the middle distance")
            t:eq(lod_far_mesh, SPHERE_MESH, "batch_lod reads back the far mesh")
            t:near(lod_far_distance, 40.0, 1e-4, "batch_lod reads back the far distance")
            t:eq(lod_cleared, nil, "clear_batch_lod removed the LOD chain")
            t:ok(type(lod_bad_err) == "string" and lod_bad_err:find("finite and > 0", 1, true) ~= nil,
                "a non-positive distance is rejected, not silently ignored")
            t:ok(type(lod_same_err) == "string" and lod_same_err:find("must differ", 1, true) ~= nil,
                "a lod mesh equal to the batch mesh is rejected")
            t:ok(type(lod_far_pair_err) == "string" and lod_far_pair_err:find("given together", 1, true) ~= nil,
                "half of the far band is rejected, not silently ignored")
            t:ok(type(lod_far_order_err) == "string" and lod_far_order_err:find("> distance", 1, true) ~= nil,
                "a far distance not beyond the middle one is rejected")
        end,
    },
    {
        id = "crowd_batch_planar",
        title = "Crowd batch planar exclusion",
        claim = "mesh.set_batch_planar 把实例批次排除出水面平面反射 pass（主视图与两条阴影链照旧）：组件缺席读回 true，置 false 后读回 false；没有实例批次的实体直接报错（不静默插组件让调用方以为生效）",
        api = { "mesh.set_batch", "mesh.set_batch_planar", "mesh.batch_planar", "entity.find", "entity.is_alive" },
        code = "mesh.set_batch(tri, ground, {{x=0}}); mesh.batch_planar(tri); mesh.set_batch_planar(tri, false); mesh.batch_planar(tri)",
        enter = function()
            reset_world()
            local tri = live(TRI)
            if not tri then
                return
            end
            mesh.set_batch(tri, GROUND_MESH, { { x = 0.0, y = 0.0, z = 0.0 } })
            -- 组件缺席 = 照旧进反射；置 false 才排掉，两个方向都要读回。
            planar_default = mesh.batch_planar(tri)
            mesh.set_batch_planar(tri, false)
            planar_off = mesh.batch_planar(tri)
            -- 没有批次的实体：静默插组件会让下一帧什么都没发生，必须报错。
            local hero = live(HERO)
            if hero then
                planar_no_batch_err = tostring((select(2, pcall(mesh.set_batch_planar, hero, false))))
            end
        end,
        run = function()
            lab.note("batch_planar 缺席=" .. tostring(planar_default)
                .. "，置 false 后=" .. tostring(planar_off))
        end,
        read = function()
            return {
                planar_default = tostring(planar_default),
                planar_off = tostring(planar_off),
                planar_no_batch_err = tostring(planar_no_batch_err),
            }
        end,
        check = function(t)
            local tri = live(TRI)
            if not tri then
                t:ok(false, "TriMesh is required")
                return
            end
            -- 读回发生在 enter 里；这里再读一次当前值，确认组件挂在实体上而不是只回了个值。
            planar_live = mesh.batch_planar(tri)
            t:eq(planar_default, true, "a batch without the component reads back true")
            t:eq(planar_off, false, "set_batch_planar(false) reads back false")
            t:eq(planar_live, false, "the exclusion survives the component round-trip")
            t:ok(type(planar_no_batch_err) == "string"
                and planar_no_batch_err:find("no instance batch", 1, true) ~= nil,
                "setting the exclusion on a non-batch entity is rejected")
        end,
    },
    {
        id = "sun_and_fill",
        title = "Sun / fill",
        claim = "light.set 写方向/颜色/强度、light.get 读回同一组数字；L 在两盏灯间切 light.set_active",
        api = { "light.set", "light.get", "light.move", "light.set_active", "entity.find", "entity.is_alive" },
        code = "light.set(sun, ...); light.move(ember, -0.25, 0.8, 0); light.get(sun)",
        enter = function()
            reset_world()
            local sun = live(SUN)
            if sun then
                light.set(sun, 0.40, -0.85, -0.45, 1.00, 0.96, 0.88, 1.2)
            end
        end,
        run = function()
            local sun = live(SUN)
            local ember = live(EMBER)
            if ember then
                -- 点光源的 position 可以独立于 Transform 移动：暖光跟着火花左右偏一点。
                light.move(ember, -0.25, 0.8, 0.0)
            end
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
            local ember = live(EMBER)
            return {
                sun = sun and light.get(sun).intensity or "-",
                fill = fill and light.get(fill).intensity or "-",
                ember = ember and light.get(ember).px or "-",
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
            local ember = live(EMBER)
            if ember then
                local point = light.get(ember)
                t:near(point.px, -0.25, 1e-3, "light.move updates a point light's x")
                t:near(point.py, 0.8, 1e-3, "light.move updates a point light's y")
                t:eq(point.kind, "point", "the moved light is a point light")
            end
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
