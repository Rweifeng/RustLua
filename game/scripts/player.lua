-- 玩家职责：移动、攻击、血量、动画状态选择。
local state = require("state")

local M = {}

function M.find()
    return entity.find(state.player_name)
end

--- 初始血量：读存档优先，否则满血
function M.initial_hp()
    local saved = save.read("hp")
    return saved and tonumber(saved) or state.max_health
end

function M.set_hp(id, hp)
    if id then
        health.set(id, hp)
    end
end

function M.hp(id)
    return id and health.get(id) or nil
end

function M.damage(id, amount)
    if id then
        health.damage(id, amount)
        return health.get(id)
    end
    return nil
end

--- 移动：游戏结束后停止驱动，否则按输入轴速度移动
function M.move(id, horizontal, vertical, game_over)
    if game_over then
        physics.set_velocity(id, 0, 0)
    else
        physics.set_velocity(id, horizontal * state.move_speed, vertical * state.move_speed)
    end
end

--- 更新攻击朝向：以最近的非零输入方向为准
function M.update_facing(horizontal, vertical)
    if horizontal ~= 0 or vertical ~= 0 then
        local len = math.sqrt(horizontal * horizontal + vertical * vertical)
        state.facing.x = horizontal / len
        state.facing.y = vertical / len
    end
end

--- 有方向的范围攻击：命中需在攻击距离内且在朝向锥形范围内。
--- 返回击杀数。
function M.attack(id)
    audio.play("attack")
    local p = transform.get(id)
    -- 朝向方向的攻击特效
    particles.burst(
        p.x + state.facing.x * 24,
        p.y + state.facing.y * 24,
        10
    )
    local fx = state.facing.x
    local fy = state.facing.y
    local range = state.attack_range
    local cone = state.attack_cone
    local killed = 0
    for _, slime in ipairs(state.enemies) do
        if entity.is_alive(slime) then
            local sp = transform.get(slime)
            local dx = sp.x - p.x
            local dy = sp.y - p.y
            local dist = math.sqrt(dx * dx + dy * dy)
            if dist <= range and dist > 0.001 then
                -- 朝向点积 > cone 才命中（朝向锥形判定）
                local dot = (dx * fx + dy * fy) / dist
                if dot >= cone then
                    entity.destroy(slime)
                    particles.burst(sp.x, sp.y, 8)
                    audio.play("hit")
                    killed = killed + 1
                end
            end
        end
    end
    return killed
end

local function is_oneshot(anim_state)
    return anim_state == "attack" or anim_state == "hit" or anim_state == "death"
end

--- 根据输入与当前动画状态选择下一步动画
function M.choose_animation(id, horizontal, vertical)
    local current = animation.get(id)
    local ended = false
    if is_oneshot(current) then
        ended = animation.ended(id)
    end
    if not (is_oneshot(current) and not ended) then
        if input.just_pressed("space") then
            animation.set(id, "attack")
        elseif horizontal ~= 0 or vertical ~= 0 then
            animation.set(id, "move")
        else
            animation.set(id, "idle")
        end
    end
end

return M
