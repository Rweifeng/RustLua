local state = require("production_state")

local M = {}

local BAR_X, BAR_Y, BAR_W, BAR_H = 20, 20, 240, 16
local TEXT_X, TEXT_Y = 20, 44
local TEXT_SCALE = 2.0
local RETRY = { x = 520, y = 380, w = 240, h = 52 }

local function draw_bar(ratio)
    ui.bar("hp", BAR_X, BAR_Y, BAR_W, BAR_H, ratio, 0.25, 0.9, 0.35)
end

function M.init_objective()
    draw_bar(1.0)
    M.set_objective(
        "WASD move  SPACE attack  F1 save  F2 load  F4 menu  collect "
            .. tostring(state.coin_total)
            .. " pickups"
    )
end

function M.set_objective(text)
    ui.text_at("objective", TEXT_X, TEXT_Y, text, TEXT_SCALE)
end

function M.update_hp(ratio)
    draw_bar(ratio)
    ui.text_at(
        "score",
        TEXT_X,
        72,
        "Score " .. tostring(state.score) .. "/" .. tostring(state.coin_total),
        1.6
    )
end

function M.show_retry_button()
    ui.bar("retry_btn", RETRY.x, RETRY.y, RETRY.w, RETRY.h, 1.0, 0.18, 0.42, 0.72)
    ui.text_at("retry_label", RETRY.x + 26, RETRY.y + 12, "Retry Round (R)", 2.0)
end

function M.retry_button_clicked()
    if not input.mouse_just_pressed("left") then
        return false
    end
    local pos = input.cursor_position()
    local x, y = pos[1], pos[2]
    return x >= RETRY.x
        and x <= RETRY.x + RETRY.w
        and y >= RETRY.y
        and y <= RETRY.y + RETRY.h
end

return M
