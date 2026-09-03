-- HUD 职责：主菜单、血条、目标文本的绘制。
local state = require("state")

local M = {}

local BAR_X, BAR_Y, BAR_W, BAR_H = 20, 20, 200, 16
local TEXT_X, TEXT_Y = 20, 44
local TEXT_SCALE = 2.0

-- 重玩按钮区域（屏幕像素坐标）
local RETRY = { x = 540, y = 380, w = 200, h = 52 }

local function draw_bar(ratio)
    ui.bar("hp", BAR_X, BAR_Y, BAR_W, BAR_H, ratio, 0.25, 0.9, 0.35)
end

--- 主菜单：标题 + 操作说明
function M.init_menu()
    draw_bar(1.0)
    ui.text_at("objective", 20, 60, "P2 俯视角 Roguelite", 3.0)
    ui.text_at("menu", 20, 100, "SPACE 开始", 2.0)
    ui.text_at(
        "menu_help",
        20,
        130,
        "WASD 移动  SPACE 方向攻击  F1 存  F2 读  F4 切场景",
        1.5
    )
end

--- 每帧刷新主菜单文本（避免被其他绘制覆盖）
function M.update_menu()
    M.init_menu()
end

--- 开始游戏后的目标文本
function M.init_objective()
    draw_bar(1.0)
    local objective = "F1 save F2 load F4 switch - SPACE attack"
    if state.game_active then
        objective = "Kill slimes, collect " .. state.coin_total .. " coins - SPACE attack"
    end
    M.set_objective(objective)
end

function M.set_objective(text)
    ui.text_at("objective", TEXT_X, TEXT_Y, text, TEXT_SCALE)
end

function M.update_hp(ratio)
    draw_bar(ratio)
end

--- 终局重玩按钮：背景 + 标签
function M.show_retry_button()
    ui.bar("retry_btn", RETRY.x, RETRY.y, RETRY.w, RETRY.h, 1.0, 0.18, 0.42, 0.72)
    ui.text_at("retry_label", RETRY.x + 30, RETRY.y + 12, "重玩一局 (R)", 2.0)
end

--- 重玩按钮命中检测：鼠标左键点击且在按钮区域内
function M.retry_button_clicked()
    if not input.mouse_just_pressed("left") then
        return false
    end
    local pos = input.cursor_position()
    local cx, cy = pos[1], pos[2]
    return cx >= RETRY.x
        and cx <= RETRY.x + RETRY.w
        and cy >= RETRY.y
        and cy <= RETRY.y + RETRY.h
end

return M
