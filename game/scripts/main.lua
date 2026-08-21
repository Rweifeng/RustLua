log.info("project script loaded")

local player_name = "Player"
local map_name = "Tilemap"
local move_speed = 240 -- world units per second
local enemy_speed = 90
local map_id = nil
local tile_size = 32
local event_count = 0
local max_health = 100
local scene_name = "main"
local enemies = {}
local coins = {}
local coin_total = 0
local score = 0
local game_active = false
local game_over = false
local game_win = false

local function is_oneshot(state)
    return state == "attack" or state == "hit" or state == "death"
end

local function find_all(prefix)
    local list = {}
    local i = 1
    while true do
        local e = entity.find(prefix .. i)
        if not e or not entity.is_alive(e) then
            break
        end
        table.insert(list, e)
        i = i + 1
    end
    return list
end

local function current_scene()
    local loaded = scene.reload()
    return loaded or scene_name
end

function on_create()
    scene_name = current_scene()
    map_id = entity.find(map_name)
    if map_id and entity.is_alive(map_id) then
        local d = tilemap.get(map_id)
        if d then
            tile_size = d.tile_size
            log.info("scene '" .. scene_name .. "' tilemap " .. d.width .. "x" .. d.height)
        end
    end
    enemies = find_all("Slime")
    coins = find_all("Coin")
    coin_total = #coins
    score = 0
    game_active = coin_total > 0
    game_over = false
    game_win = false
    local player = entity.find(player_name)
    if player then
        local saved = save.read("hp")
        local hp = saved and tonumber(saved) or max_health
        health.set(player, hp)
    end
    audio.volume(0.8)
    ui.bar("hp", 20, 20, 200, 16, 1.0, 0.25, 0.9, 0.35)
    local objective = "F1 save F2 load F4 switch - SPACE attack"
    if game_active then
        objective = "Kill slimes, collect " .. coin_total .. " coins - SPACE attack"
    end
    ui.text_at("objective", 20, 44, objective)
end

local function is_in(list, id)
    for _, e in ipairs(list) do
        if e == id then
            return true
        end
    end
    return false
end

local function alive_count(list)
    local n = 0
    for _, e in ipairs(list) do
        if entity.is_alive(e) then
            n = n + 1
        end
    end
    return n
end

local function do_attack(player)
    audio.play("attack")
    local p = transform.get(player)
    particles.burst(p.x, p.y, 10)
    for _, slime in ipairs(enemies) do
        if entity.is_alive(slime) then
            local sp = transform.get(slime)
            local dx = sp.x - p.x
            local dy = sp.y - p.y
            if dx * dx + dy * dy < 48 * 48 then
                entity.destroy(slime)
                particles.burst(sp.x, sp.y, 8)
                audio.play("hit")
            end
        end
    end
end

local function on_player_hit(player)
    health.damage(player, 20)
    local p = transform.get(player)
    particles.burst(p.x, p.y, 12)
    audio.play("hit")
    log.info("player hit: hp=" .. health.get(player))
end

local function on_coin(player, coin)
    entity.destroy(coin)
    score = score + 1
    audio.play("pickup")
    log.info("coin collected " .. score .. "/" .. coin_total)
end

local function handle_events(player)
    local events = physics.events()
    for _, e in ipairs(events) do
        if e.started then
            event_count = event_count + 1
            local other = nil
            if e.a == player then other = e.b
            elseif e.b == player then other = e.a
            end
            if other and game_active and not game_over then
                if is_in(enemies, other) then
                    on_player_hit(player)
                elseif is_in(coins, other) then
                    on_coin(player, other)
                end
            end
        end
    end
end

local function update_status(player)
    local coins_left = alive_count(coins)
    if game_active and coins_left == 0 then
        game_over = true
        game_win = true
        ui.text_at("objective", 20, 44, "VICTORY! Collected all " .. coin_total .. " coins - F4 for another arena")
    elseif health.get(player) <= 0 then
        game_over = true
        game_win = false
        ui.text_at("objective", 20, 44, "DEFEATED! F1 save, F2 retry")
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
        if game_over then
            physics.set_velocity(player, 0, 0)
        else
            physics.set_velocity(player, horizontal * move_speed, vertical * move_speed)
        end
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
        if not game_over and game_active then
            -- enemies chase the player
            for _, slime in ipairs(enemies) do
                if entity.is_alive(slime) then
                    local sp = transform.get(slime)
                    local dx = p.x - sp.x
                    local dy = p.y - sp.y
                    local len = math.sqrt(dx * dx + dy * dy)
                    if len > 1 then
                        physics.set_velocity(slime, dx / len * enemy_speed, dy / len * enemy_speed)
                    end
                end
            end
            if input.just_pressed("space") then
                do_attack(player)
            end
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
        update_status(player)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s cell=%s anim=%s frame=%d events=%d hp=%s scene=%s coins=%d/%d",
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
        scene_name,
        score,
        coin_total
    )
end
