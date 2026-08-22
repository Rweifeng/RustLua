-- M8-06/07 3D 集成演示：移动网格、切换相机、切换骨骼动画剪辑。
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

function M.on_create()
    camera_index = 1
    warrior_index = 1
    elapsed = 0
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

        -- 每 6 秒切换一次骨骼动画剪辑，验证 animation3d API。
        local clip_cycle = math.floor(elapsed / 6) % #warrior_clips + 1
        if clip_cycle ~= warrior_index then
            warrior_index = clip_cycle
            local warrior = entity.find(warrior_name)
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
