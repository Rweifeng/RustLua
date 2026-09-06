local M = {}

local ball = nil

function M.on_create()
    ball = entity.find("PhysicsBall")
    transform3d.set(ball, 0, 3, 0)
    physics3d.set_velocity(ball, 1.5, 0, 0.5)
    ui.text_at("physics3d_status", 20, 620, "SPACE: impulse  R: reset", 2)
end

function M.update()
    if not ball or not entity.is_alive(ball) then
        return "physics3d_idle"
    end
    if input.just_pressed("space") then
        local velocity = physics3d.get_velocity(ball)
        physics3d.set_velocity(ball, velocity.x * -0.75, 4.0, velocity.z)
    elseif input.just_pressed("r") then
        physics3d.set_position(ball, 0, 3, 0)
        physics3d.set_velocity(ball, 1.5, 0, 0.5)
    end
    local position = transform3d.get(ball)
    local contacts = #physics3d.events()
    return string.format(
        "physics3d_ok pos=%.2f,%.2f,%.2f contacts=%d",
        position.x,
        position.y,
        position.z,
        contacts
    )
end

return M
