-- M9-04 3D 样板游戏垂直切片：碰撞移动、敌人追击、拾取、HUD、胜负与重开。
-- 只在 game3d.scene（含 Player3d）激活；其他场景防御式跳过。
local M = {}

local scene_name = "game3d"
local player_name = "Player3d"
local enemy_names = { "Enemy1", "Enemy2" }
local pickup_prefix = "Pickup"
local camera_name = "Camera3d"
local max_health = 100
local move_speed = 6.0
local enemy_speed = 2.6
local hurt_cooldown = 1.0

local player_id = nil
local camera_id = nil
local enemies = {}
local pickups = {}
local game_over = false
local win = false
local hurt_timer = 0
local over_timer = 0
-- 玩家上次输入（仅在输入变化时设置刚体速度，避免每帧重复施加导致贴墙抖动）。
local last_vel_x, last_vel_z, last_moving = 0, 0, false
-- 竞技场边界（隐形传感器墙不物理阻挡，敌人手动夹紧）。
local arena_bound = 3.6

-- 轨道相机：右键拖拽旋转，滚轮缩放，WASD 相对相机朝向移动。
local cam_yaw = 0
local cam_pitch = 0.62 -- 默认高度角（atan(5/7)）
local cam_dist = 8.6
local cam_sens = 0.006
local pitch_min, pitch_max = 0.05, 1.3
local cam_zoom_speed = 1.2
local cam_dist_min, cam_dist_max = 4.0, 16.0

local function clamp_arena(value)
    return math.max(-arena_bound, math.min(arena_bound, value))
end

local function alive(name)
    local e = entity.find(name)
    return e and entity.is_alive(e) and e or nil
end

local function is_in(list, id)
    for _, e in ipairs(list) do
        if e == id then
            return true
        end
    end
    return false
end

local function count_pickups()
    local n = 0
    for _, e in ipairs(pickups) do
        if entity.is_alive(e) then
            n = n + 1
        end
    end
    return n
end

function M.on_create()
    player_id = alive(player_name)
    if not player_id then
        return
    end
    camera_id = alive(camera_name)
    enemies = {}
    pickups = {}
    for _, name in ipairs(enemy_names) do
        local e = alive(name)
        if e then
            table.insert(enemies, e)
            animation3d.set(e, "Running_A")
        end
    end
    local i = 1
    while true do
        local e = alive(pickup_prefix .. i)
        if not e then
            break
        end
        table.insert(pickups, e)
        i = i + 1
    end
    game_over = false
    win = false
    hurt_timer = 0
    over_timer = 0
    last_vel_x, last_vel_z, last_moving = 0, 0, false
    cam_yaw = 0
    cam_pitch = 0.62
    health.set(player_id, max_health)
    animation3d.set(player_id, "Idle")
    log.info(
        "game3d: scene='" .. scene_name .. "' player=" .. player_id
            .. " enemies=" .. #enemies
            .. " pickups=" .. #pickups
    )
end

function M.update()
    if not player_id or not entity.is_alive(player_id) then
        return "game3d-idle"
    end
    local dt = time.delta()
    hurt_timer = math.max(0, hurt_timer - dt)
    if game_over then
        over_timer = over_timer + dt
    end

    -- 轨道相机旋转：右键拖拽（yaw 水平 / pitch 俯仰，限制范围）。
    if input.mouse_is_down("right") then
        local delta = input.cursor_delta()
        local dx, dy = delta[1], delta[2]
        cam_yaw = cam_yaw + dx * cam_sens
        cam_pitch = cam_pitch - dy * cam_sens
        if cam_pitch < pitch_min then
            cam_pitch = pitch_min
        end
        if cam_pitch > pitch_max then
            cam_pitch = pitch_max
        end
    end

    -- 滚轮缩放：滚动调整相机距离（视野远近）。
    local scroll = input.scroll_delta()
    local sy = scroll[2]
    if sy ~= 0 then
        cam_dist = cam_dist - sy * cam_zoom_speed
        if cam_dist < cam_dist_min then
            cam_dist = cam_dist_min
        end
        if cam_dist > cam_dist_max then
            cam_dist = cam_dist_max
        end
    end

    -- 玩家碰撞移动：WASD 相对相机朝向；仅在期望速度变化时设置（避免贴墙抖动）。
    local h = input.axis("horizontal")
    local v = input.axis("vertical")
    local moving = math.abs(h) > 0.05 or math.abs(v) > 0.05
    local desired_vx, desired_vz = 0, 0
    if moving then
        -- 相机-相对基：forward = (-sin yaw, -cos yaw)，right = (cos yaw, -sin yaw)
        local forward_x, forward_z = -math.sin(cam_yaw), -math.cos(cam_yaw)
        local right_x, right_z = math.cos(cam_yaw), -math.sin(cam_yaw)
        local vx = forward_x * v + right_x * h
        local vz = forward_z * v + right_z * h
        local len = math.sqrt(vx * vx + vz * vz)
        if len > 1 then
            vx, vz = vx / len, vz / len
        end
        desired_vx, desired_vz = vx * move_speed, vz * move_speed
    end
    if desired_vx ~= last_vel_x or desired_vz ~= last_vel_z then
        physics3d.set_velocity(player_id, desired_vx, 0, desired_vz)
        last_vel_x, last_vel_z = desired_vx, desired_vz
    end
    if moving ~= last_moving then
        animation3d.set(player_id, moving and "Running_A" or "Idle")
        last_moving = moving
    end

    -- 敌人 AI：朝玩家步进（固定传感器体，不物理推挤玩家；手动夹紧竞技场边界）。
    local p = transform3d.get(player_id)
    if p then
        for _, e in ipairs(enemies) do
            if entity.is_alive(e) then
                local ep = transform3d.get(e)
                if ep then
                    local dx = p.x - ep.x
                    local dz = p.z - ep.z
                    local dist = math.sqrt(dx * dx + dz * dz)
                    local move = math.min(dist, enemy_speed * dt)
                    local nx, nz
                    if dist > 0.01 then
                        nx = ep.x + dx / dist * move
                        nz = ep.z + dz / dist * move
                    else
                        nx, nz = ep.x, ep.z
                    end
                    nx = clamp_arena(nx)
                    nz = clamp_arena(nz)
                    transform3d.set(e, nx, 0.9, nz)
                    physics3d.set_position(e, nx, 0.9, nz)
                end
            end
        end
    end

    -- 碰撞事件：敌人接触伤害（带冷却）/ 拾取收集。
    local events = physics3d.events()
    for _, ev in ipairs(events) do
        local other = nil
        if ev.a == player_id then
            other = ev.b
        elseif ev.b == player_id then
            other = ev.a
        end
        if other and ev.started and not game_over then
            if is_in(enemies, other) and hurt_timer <= 0 then
                hurt_timer = hurt_cooldown
                local hp = health.damage(player_id, 20)
                audio.play("hit")
                if hp <= 0 then
                    game_over = true
                    win = false
                end
            elseif is_in(pickups, other) then
                entity.destroy(other)
                audio.play("pickup")
                if count_pickups() == 0 then
                    game_over = true
                    win = true
                end
            end
        end
    end

    -- 轨道相机跟随：位于玩家后上方（yaw/pitch/dist）看向玩家。
    if camera_id and p then
        local cos_p = math.cos(cam_pitch)
        camera.set(
            camera_id,
            p.x + cam_dist * cos_p * math.sin(cam_yaw),
            p.y + cam_dist * math.sin(cam_pitch),
            p.z + cam_dist * cos_p * math.cos(cam_yaw),
            p.x,
            p.y,
            p.z,
            60
        )
    end

    -- HUD：血条 / 目标 / 终局。
    local hp = health.get(player_id) or 0
    ui.text_at("hp", 20, 20, string.format("HP %d/%d", math.ceil(hp), max_health), 1.5)
    ui.text_at("objective", 20, 48, "收集全部宝石：剩余 " .. count_pickups(), 1.5)
    ui.text_at("controls", 20, 76, "WASD 移动  右键拖拽旋转  滚轮缩放", 1.2)
    if game_over then
        ui.text_at("over", 320, 180, win and "VICTORY!" or "DEFEATED", 3.0)
        ui.text_at("restart", 320, 220, "按 R 重开一局", 1.5)
        if input.just_pressed("r") and over_timer > 0.5 then
            scene.load(scene_name)
        end
    end
    return "game3d-ok hp=" .. math.ceil(hp)
end

return M
