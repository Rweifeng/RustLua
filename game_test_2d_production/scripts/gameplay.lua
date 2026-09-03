local state = require("state")
local player = require("player")
local enemies = require("enemies")
local coins = require("coins")
local hud = require("hud")
local save_manager = require("save_manager")

local M = {}

function M.on_create()
    state.scene_name = state.current_scene()
    if state.scene_name ~= state.production_scene then
        return
    end
    state.map_id = entity.find(state.map_name)
    state.score = 0
    state.event_count = 0
    state.game_active = true
    state.game_over = false
    state.game_win = false
    local map = tilemap.get(state.map_id)
    if map then
        state.tile_size = map.tile_size
        log.info("production scene tilemap " .. map.width .. "x" .. map.height)
    end
    enemies.find_all()
    coins.find_all()
    local player_id = player.find()
    player.set_hp(player_id, state.max_health)
    audio.volume(0.8)
    if not state.music_started then
        audio.music("bgm")
        state.music_started = true
    end
    hud.init_objective()
end

local function retry_round()
    state.fresh_round = true
    scene.load(state.production_scene)
end

local function on_player_hit(player_id)
    local hp = player.damage(player_id, 20)
    local position = transform.get(player_id)
    particles.burst(position.x, position.y, 12)
    audio.play("hit")
    log.info("player hit: hp=" .. tostring(hp))
end

local function update_status(player_id)
    local coins_left = state.alive_count(state.coins)
    if state.game_active and coins_left == 0 then
        state.game_over = true
        state.game_win = true
        hud.set_objective("VICTORY! R retry  F4 menu")
    elseif health.get(player_id) <= 0 then
        state.game_over = true
        state.game_win = false
        hud.set_objective("DEFEATED! R retry  F1 save  F2 load")
    end
end

local function handle_events(player_id)
    local events = physics.events()
    for _, event in ipairs(events) do
        if event.started then
            state.event_count = state.event_count + 1
            local other = nil
            if event.a == player_id then
                other = event.b
            elseif event.b == player_id then
                other = event.a
            end
            if other and state.game_active and not state.game_over then
                if state.is_in(state.enemies, other) then
                    on_player_hit(player_id)
                elseif state.is_in(state.coins, other) then
                    coins.pick(other)
                end
            end
        end
    end
end

function M.update()
    if state.current_scene() ~= state.production_scene then
        return "scene_idle"
    end
    if state.fresh_round then
        state.fresh_round = false
        M.on_create()
    end
    local player_id = player.find()
    if not player_id or not entity.is_alive(player_id) then
        return "player_missing"
    end
    if input.just_pressed("f4") then
        scene.load(state.menu_scene)
        return "return_menu"
    end

    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    player.update_facing(horizontal, vertical)
    player.move(player_id, horizontal, vertical, state.game_over)

    if not state.game_over then
        enemies.chase(player_id)
        if input.just_pressed("space") then
            player.attack(player_id)
        elseif input.just_pressed("f1") then
            save_manager.save(player_id)
        elseif input.just_pressed("f2") then
            if save_manager.load(player_id) then
                return "load_requested"
            end
        end
    else
        hud.show_retry_button()
        if input.just_pressed("r") or hud.retry_button_clicked() then
            retry_round()
            return "retry_requested"
        end
    end

    handle_events(player_id)
    update_status(player_id)
    player.choose_animation(player_id, horizontal, vertical)
    hud.update_hp(health.get(player_id) / state.max_health)

    local position = transform.get(player_id)
    return string.format(
        "production_ok player=(%.1f,%.1f) coins=%d/%d hp=%s events=%d",
        position.x,
        position.y,
        state.score,
        state.coin_total,
        tostring(health.get(player_id)),
        state.event_count
    )
end

return M
