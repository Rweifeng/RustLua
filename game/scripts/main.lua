log.info("project script loaded")

function update()
    local dt = time.delta()
    return string.format("lua-update-ok dt=%.6f", dt)
end