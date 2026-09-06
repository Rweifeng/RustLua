local M = {}

local elapsed = 0

function M.on_create()
    audio.volume(0.7)
    ui.text_at("effects_status", 20, 620, "SPACE burst  E sound  B music", 2)
end

function M.update()
    elapsed = elapsed + time.delta()
    if input.just_pressed("space") then
        particles.burst(240, 180, 24)
    elseif input.just_pressed("e") then
        audio.play("pickup")
    elseif input.just_pressed("b") then
        audio.music("bgm")
    end
    if input.just_pressed("r") then
        audio.stop_music()
    end
    return string.format("effects_ok time=%.2f", elapsed)
end

return M
