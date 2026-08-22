-- M8-06 3D 集成演示：用 Lua 移动 3D 网格并周期性切换相机。
-- 防御式：实体缺失或 API 出错时仅记录日志，绝不影响 2D 样板游戏。
local M = {}

local triangle_name = "ShadowTriangle"
local camera_names = { "Camera", "Camera2" }
local camera_index = 1
local elapsed = 0
local speed = 2.0

function M.on_create()
    camera_index = 1
    elapsed = 0
    local cam = entity.find(camera_names[1])
    if cam and entity.is_alive(cam) then
        camera.set_active(cam)
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
