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

--- 攻击：半径 48 内击杀史莱姆，返回击杀数
function M.attack(id)
    audio.play("attack")
    local p = transform.get(id)
    particles.burst(p.x, p.y, 10)
    local killed = 0
    for _, slime in ipairs(state.enemies) do
        if entity.is_alive(slime) then
            local sp = transform.get(slime)
            local dx = sp.x - p.x
            local dy = sp.y - p.y
            if dx * dx + dy * dy < 48 * 48 then
                entity.destroy(slime)
                particles.burst(sp.x, sp.y, 8)
                audio.play("hit")
                killed = killed + 1
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
