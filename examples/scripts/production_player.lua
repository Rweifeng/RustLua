local state = require("production_state")

local M = {}

function M.find()
    return entity.find(state.player_name)
end

function M.set_hp(entity_id, hp)
    if entity_id and entity.is_alive(entity_id) then
        health.set(entity_id, hp)
    end
end

function M.damage(entity_id, amount)
    if entity_id and entity.is_alive(entity_id) then
        health.damage(entity_id, amount)
        return health.get(entity_id)
    end
    return nil
end

function M.update_facing(horizontal, vertical)
    if horizontal ~= 0 or vertical ~= 0 then
        local length = math.sqrt(horizontal * horizontal + vertical * vertical)
        state.facing.x = horizontal / length
        state.facing.y = vertical / length
    end
end

function M.move(entity_id, horizontal, vertical, locked)
    if not entity_id or not entity.is_alive(entity_id) then
        return
    end
    if locked then
        physics.set_velocity(entity_id, 0, 0)
    else
        physics.set_velocity(
            entity_id,
            horizontal * state.move_speed,
            vertical * state.move_speed
        )
    end
end

function M.attack(entity_id)
    if not entity_id or not entity.is_alive(entity_id) then
        return 0
    end
    audio.play("attack")
    local position = transform.get(entity_id)
    particles.burst(
        position.x + state.facing.x * 24,
        position.y + state.facing.y * 24,
        10
    )
    local killed = 0
    for _, enemy_id in ipairs(state.enemies) do
        if entity.is_alive(enemy_id) then
            local enemy_position = transform.get(enemy_id)
            local dx = enemy_position.x - position.x
            local dy = enemy_position.y - position.y
            local distance = math.sqrt(dx * dx + dy * dy)
            if distance <= state.attack_range and distance > 0.001 then
                local dot = (dx * state.facing.x + dy * state.facing.y) / distance
                if dot >= state.attack_cone then
                    entity.destroy(enemy_id)
                    particles.burst(enemy_position.x, enemy_position.y, 8)
                    audio.play("hit")
                    killed = killed + 1
                end
            end
        end
    end
    return killed
end

local function is_oneshot(animation_name)
    return animation_name == "attack" or animation_name == "hit" or animation_name == "death"
end

function M.choose_animation(entity_id, horizontal, vertical)
    if not entity_id or not entity.is_alive(entity_id) then
        return
    end
    local current = animation.get(entity_id)
    local ended = false
    if is_oneshot(current) then
        ended = animation.ended(entity_id)
    end
    if not (is_oneshot(current) and not ended) then
        if input.just_pressed("space") then
            animation.set(entity_id, "attack")
        elseif horizontal ~= 0 or vertical ~= 0 then
            animation.set(entity_id, "move")
        else
            animation.set(entity_id, "idle")
        end
    end
end

return M
