-- One entry point routes each showcase scene to a focused demo module.
local route = require("route")

function on_create()
    route.on_create()
end

function update()
    return route.update()
end

function on_click(name)
    route.on_click(name)
end

function on_drag(from_id, from_index, to_id, to_index)
    route.on_drag(from_id, from_index, to_id, to_index)
end

function on_day_start()
    route.on_day_start()
end

function on_day_end()
    route.on_day_end()
end

function on_migrate(from, to)
    route.on_migrate(from, to)
end
