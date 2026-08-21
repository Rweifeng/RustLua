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
local scene_name = "main"

local function is_oneshot(state)
    return state == "attack" or state == "hit" or state == "death"
end

local function current_scene()
    local loaded = scene.reload()
    return loaded or scene_name
end

function on_create()
    scene_name = current_scene()
    local map = entity.find(map_name)
    if map and entity.is_alive(map) then
        local d = tilemap.get(map)
        if d then
            map_id = map
            tile_size = d.tile_size
            log.info("scene '" .. scene_name .. "' tilemap " .. d.width .. "x" .. d.height)
        end
    end
    trigger_id = entity.find(trigger_name)
    hit_in_trigger = false
    local player = entity.find(player_name)
    if player then
        local saved = save.read("hp")
        local hp = saved and tonumber(saved) or max_health
        health.set(player, hp)
        log.info("hp restored to " .. hp)
    end
    audio.volume(0.8)
    ui.bar("hp", 20, 20, 200, 16, 1.0, 0.25, 0.9, 0.35)
    ui.text_at("objective", 20, 44, "Scene: " .. scene_name .. " - F1 save F2 load F4 switch - SPACE attack")
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

local function handle_save_load(player)
    if input.just_pressed("f1") then
        save.write("hp", tostring(health.get(player)))
        save.write("scene", scene_name)
        if save.commit() then
            log.info("save committed: hp=" .. health.get(player) .. " scene=" .. scene_name)
        end
    elseif input.just_pressed("f2") then
        if save.load() then
            local saved_hp = save.read("hp")
            if saved_hp then
                health.set(player, tonumber(saved_hp))
            end
            local saved_scene = save.read("scene")
            if saved_scene and saved_scene ~= scene_name then
                scene.load(saved_scene)
            end
            log.info("save loaded: hp=" .. health.get(player))
        end
    elseif input.just_pressed("f4") then
        local target = (scene_name == "main") and "arena" or "main"
        if scene.load(target) then
            log.info("switching to scene '" .. target .. "'")
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
        handle_save_load(player)
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
        ui.bar("hp", 20, 20, 200, 16, health.get(player) / max_health, 0.25, 0.9, 0.35)
        handle_events(player)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s cell=%s anim=%s frame=%d events=%d hp=%s scene=%s",
        dt,
        horizontal,
        vertical,
        tostring(player ~= nil),
        position,
        cell,
        state,
        frame,
        event_count,
        (player and tostring(health.get(player)) or "?"),
        scene_name
    )
end
