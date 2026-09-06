local M = {}

local elapsed = 0

function M.on_create()
    ui.text_at("layout_status", 20, 620, "Anchor + Offset + Container + ButtonStates", 2)
    log.info("ui_layout: static UI demo ready")
end

function M.on_click(name)
    elapsed = elapsed + 1
    ui.text_at(
        "layout_status",
        20,
        620,
        "clicked " .. tostring(name) .. " count=" .. tostring(elapsed),
        2
    )
end

function M.update()
    return "ui_layout_ok clicks=" .. tostring(elapsed)
end

return M
