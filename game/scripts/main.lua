log.info("project script loaded")

local player_name = "Player"
local map_name = "Tilemap"
local move_speed = 240 -- world units per second
local map_id = nil
local tile_size = 32

function on_create()
    local map = entity.find(map_name)
    if map and entity.is_alive(map) then
        local d = tilemap.get(map)
        if d then
            map_id = map
            tile_size = d.tile_size
            log.info("tilemap loaded " .. d.width .. "x" .. d.height .. " tile_size=" .. d.tile_size)
        end
    end
end

local function is_oneshot(state)
    return state == "attack" or state == "hit" or state == "death"
end

function update()
    local dt = time.delta()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local player = entity.find(player_name)
    local position = "?"
    local cell = "?"
    local state = "?"
    local frame = -1
    if player and entity.is_alive(player) then
        transform.move(player, horizontal * move_speed * dt, vertical * move_speed * dt)
        local p = transform.get(player)
        position = string.format("(%.1f,%.1f)", p.x, p.y)
        if map_id and entity.is_alive(map_id) then
            local mt = transform.get(map_id)
            local cx = math.floor((p.x - mt.x) / tile_size)
            local cy = math.floor((p.y - mt.y) / tile_size)
            cell = string.format(
                "(%d,%d)%s",
                cx,
                cy,
                tostring(tilemap.is_solid(map_id, cx, cy))
            )
        end
        -- animation state selection: one-shots (attack/hit/death) play to their
        -- end event before normal idle/move selection resumes.
        local current = animation.get(player)
        local ended = false
        if is_oneshot(current) then
            ended = animation.ended(player)
        end
        if not (is_oneshot(current) and not ended) then
            if input.just_pressed("space") then
                animation.set(player, "attack")
            elseif horizontal ~= 0 or vertical ~= 0 then
                animation.set(player, "move")
            else
                animation.set(player, "idle")
            end
        end
        state = animation.get(player)
        frame = animation.frame(player)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s cell=%s anim=%s frame=%d",
        dt,
        horizontal,
        vertical,
        tostring(player ~= nil),
        position,
        cell,
        state,
        frame
    )
end
