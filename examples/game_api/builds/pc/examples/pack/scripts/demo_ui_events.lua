local M = {}

local clicks = 0

function M.on_create()
    ui.text_at("ui_events_status", 20, 620, "Click Start or Quit", 2)
    log.info("ui_events: button demo ready")
end

function M.on_click(name)
    clicks = clicks + 1
    ui.text_at(
        "ui_events_status",
        20,
        620,
        "clicked " .. tostring(name) .. " count=" .. tostring(clicks),
        2
    )
end

function M.update()
    return "ui_events_ok clicks=" .. tostring(clicks)
end

return M
