-- HUD 职责：血条与目标文本的绘制。
local state = require("state")

local M = {}

local BAR_X, BAR_Y, BAR_W, BAR_H = 20, 20, 200, 16
local TEXT_X, TEXT_Y = 20, 44
local TEXT_SCALE = 2.0

function M.init(objective)
    ui.bar("hp", BAR_X, BAR_Y, BAR_W, BAR_H, 1.0, 0.25, 0.9, 0.35)
    M.set_objective(objective)
end

function M.set_objective(text)
    ui.text_at("objective", TEXT_X, TEXT_Y, text, TEXT_SCALE)
end

function M.update_hp(ratio)
    ui.bar("hp", BAR_X, BAR_Y, BAR_W, BAR_H, ratio, 0.25, 0.9, 0.35)
end

return M
