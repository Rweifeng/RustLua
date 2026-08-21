log.info("project script loaded")

local player_name = "Player"
local map_name = "Tilemap"
local trigger_name = "Trigger"
local move_speed = 240 -- world units per second
local map_id = nil
local tile_size = 32
local trigger_id = nil
local event_count = 0
local max_health = 100
local hit_in_trigger = false

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
    trigger_id = entity.find(trigger_name)
    local player = entity.find(player_name)
    if player then
        health.set(player, max_health)
    end
    audio.volume(0.8)
    ui.bar("hp", 20, 20, 200, 16, 1.0, 0.25, 0.9, 0.35)
    ui.text_at("objective", 20, 44, "WASD move - SPACE attack - walk into the orange trigger")
end

local function is_oneshot(state)
    return state == "attack" or state == "hit" or state == "death"
end

local function on_trigger_entered(player)
    if hit_in_trigger then
        return
    end
    hit_in_trigger = true
    health.damage(player, 25)
    local p = transform.get(player)
    particles.burst(p.x, p.y, 14)
    audio.play("hit")
    log.info("hit taken: hp=" .. health.get(player) .. "/" .. max_health)
end

local function handle_events(player)
    local events = physics.events()
    for _, e in ipairs(events) do
        if e.started then
            event_count = event_count + 1
            local is_trigger = trigger_id
                and ((e.a == trigger_id and e.b == player) or (e.a == player and e.b == trigger_id))
            if is_trigger then
                on_trigger_entered(player)
            end
        end
    end
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
        physics.set_velocity(player, horizontal * move_speed, vertical * move_speed)
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
        if input.just_pressed("space") then
            audio.play("attack")
            particles.burst(p.x, p.y, 10)
        end
        -- animation state selection
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
        -- health bar tracks current health
        ui.bar("hp", 20, 20, 200, 16, health.get(player) / max_health, 0.25, 0.9, 0.35)
        handle_events(player)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s cell=%s anim=%s frame=%d events=%d hp=%s",
        dt,
        horizontal,
        vertical,
        tostring(player ~= nil),
        position,
        cell,
        state,
        frame,
        event_count,
        (player and tostring(health.get(player)) or "?")
    )
end
