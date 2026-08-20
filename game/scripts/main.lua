log.info("project script loaded")

function update()
    local dt = time.delta()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local space_pressed = input.just_pressed("space")
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) space_pressed=%s",
        dt,
        horizontal,
        vertical,
        tostring(space_pressed)
    )
end