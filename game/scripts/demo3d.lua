-- M8-06/07/08/09 3D 集成演示：移动网格、切换相机、骨骼动画剪辑、实例化+剔除基线。
-- 防御式：实体缺失或 API 出错时仅记录日志，绝不影响 2D 样板游戏。
local M = {}

local triangle_name = "ShadowTriangle"
local warrior_name = "SkeletonWarrior"
local camera_names = { "Camera", "Camera2" }
local camera_index = 1
local elapsed = 0
local speed = 2.0
local warrior_clips = { "Idle", "Running_A", "1H_Melee_Attack_Chop", "Idle" }
local warrior_index = 1
local grid_spawned = false

-- 实例网格数量：默认 64，可用环境变量 P2_INSTANCE_COUNT 覆盖（性能基线）。
local grid_count = 64
if os and os.getenv then
    grid_count = tonumber(os.getenv("P2_INSTANCE_COUNT")) or 64
end

--- 生成 N 个相同 mesh 实例（触发实例化 + 视锥剔除基线）。
function M.spawn_grid()
    if grid_spawned then
        return
    end
    grid_spawned = true
    local side = math.ceil(math.sqrt(grid_count))
    local n = 0
    for gx = 0, side - 1 do
        for gz = 0, side - 1 do
            if n >= grid_count then
                break
            end
            local id = entity.spawn("Inst_" .. gx .. "_" .. gz)
            transform3d.attach(id)
            transform3d.set(id, (gx - (side - 1) * 0.5) * 2.5, 0.7, (gz - (side - 1) * 0.5) * 2.5)
            mesh.set(id, "assets/models/m8-test-triangle.glb")
            n = n + 1
        end
    end
    log.info("demo3d: spawned " .. n .. " instanced triangles")
end

function M.on_create()
    -- 只在 3D 演示场景（demo3d，含 SkeletonWarrior）激活；其他场景（2D 游戏）不生成任何 3D 实体。
    if not entity.find(warrior_name) then
        return
    end
    camera_index = 1
    warrior_index = 1
    elapsed = 0
    grid_spawned = false
    M.spawn_grid()
    local cam = entity.find(camera_names[1])
    if cam and entity.is_alive(cam) then
        camera.set_active(cam)
    end
    local warrior = entity.find(warrior_name)
    if warrior and entity.is_alive(warrior) then
        animation3d.set(warrior, warrior_clips[1])
    end
end

function M.update()
    local ok, err = pcall(function()
        local dt = time.delta()
        elapsed = elapsed + dt

        -- 让三角形绕原点小幅摆动，验证 transform3d.move 驱动场景。
        local triangle = entity.find(triangle_name)
        if triangle and entity.is_alive(triangle) then
            transform3d.move(
                triangle,
                math.sin(elapsed * speed) * dt * 0.5,
                0,
                math.cos(elapsed * speed) * dt * 0.5
            )
        end

        -- 骷髅战士绕场行走（transform3d.set 驱动角色移动）。
        local warrior = entity.find(warrior_name)
        if warrior and entity.is_alive(warrior) then
            local radius = 2.2
            local angle = elapsed * 0.6
            transform3d.set(warrior, math.cos(angle) * radius, 0, math.sin(angle) * radius)
        end

        -- 每 6 秒切换一次骨骼动画剪辑，验证 animation3d API。
        local clip_cycle = math.floor(elapsed / 6) % #warrior_clips + 1
        if clip_cycle ~= warrior_index then
            warrior_index = clip_cycle
            if warrior and entity.is_alive(warrior) then
                animation3d.set(warrior, warrior_clips[warrior_index])
                log.info("demo3d: warrior clip -> '" .. warrior_clips[warrior_index] .. "'")
            end
        end

        -- 每 4 秒切换一次相机。
        local cycle = math.floor(elapsed / 4) % #camera_names + 1
        if cycle ~= camera_index then
            camera_index = cycle
            local cam = entity.find(camera_names[camera_index])
            if cam and entity.is_alive(cam) then
                camera.set_active(cam)
                log.info("demo3d: switched camera to '" .. camera_names[camera_index] .. "'")
            end
        end
    end)
    if not ok then
        log.warn("demo3d: " .. tostring(err))
    end
end

return M
