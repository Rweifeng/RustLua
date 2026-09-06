local M = {}

local selected = 1
local dragging = false

function M.on_create()
    ui.panel("panel", 40, 40, 480, 240, 0.06, 0.08, 0.12, 1)
    ui.image("icon", "assets/textures/coin.png", 68, 68, 64, 64, 1, 1, 1, 1)
    ui.grid("bag", 40, 300, 3, 2, 96, 96, 12, 0.10, 0.13, 0.18, 1)
    ui.grid_cell("bag", 1, "assets/textures/coin.png", 1, 1, 1, 1)
    ui.grid_cell("bag", 3, "assets/textures/slime.png", 0.6, 1, 0.6, 1)
    ui.tooltip("bag", 1, 40, 560, 180, 28, "Coin")
    ui.modal("confirm", 300, 200, 680, 280, "Move item?", 0, 0, 0, 1)
    ui.text_at("dynamic_status", 40, 610, "Move: drag grid cells. Space: close modal", 2)
    log.info("ui_dynamic: runtime UI demo ready")
end

function M.on_drag(from_id, from_index, to_id, to_index)
    dragging = true
    log.info(
        string.format(
            "ui_dynamic drag %s:%d -> %s:%d",
            tostring(from_id),
            from_index,
            tostring(to_id),
            to_index
        )
    )
end

function M.update()
    if input.just_pressed("space") then
        ui.modal_hide("confirm")
    elseif input.just_pressed("e") then
        ui.modal_show("confirm")
    elseif input.just_pressed("q") then
        ui.clear()
    end
    selected = selected % 6 + 1
    ui.grid_hit("bag", input.cursor_position()[1], input.cursor_position()[2])
    return "ui_dynamic_ok dragging=" .. tostring(dragging)
end

return M
