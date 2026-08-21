-- 游戏流程编排：场景初始化、每帧更新、物理事件分发、胜负判定。
-- 具体职责见同目录 player/enemies/coins/save_manager/hud 各模块。
local state = require("state")
local player = require("player")
local enemies = require("enemies")
local coins = require("coins")
local save_manager = require("save_manager")
local hud = require("hud")

local M = {}

function M.on_create()
    state.scene_name = state.current_scene()
    state.map_id = entity.find(state.map_name)
    if state.map_id and entity.is_alive(state.map_id) then
        local d = tilemap.get(state.map_id)
        if d then
            state.tile_size = d.tile_size
            log.info("scene '" .. state.scene_name .. "' tilemap " .. d.width .. "x" .. d.height)
        end
    end
    -- 启动自动读档（含存档场景恢复）；请求切换时直接返回
    if save_manager.auto_restore() then
        return
    end
    enemies.find_all()
    coins.find_all()
    state.score = 0
    state.game_active = state.coin_total > 0
    state.game_over = false
    state.game_win = false
    player.set_hp(player.find(), player.initial_hp())
    audio.volume(0.8)
    local objective = "F1 save F2 load F4 switch - SPACE attack"
    if state.game_active then
        objective = "Kill slimes, collect " .. state.coin_total .. " coins - SPACE attack"
    end
    hud.init(objective)
end

function M.update()
    local dt = time.delta()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local id = player.find()
    local position = "?"
    local cell = "?"
    local anim_state = "?"
    local frame = -1
    if id and entity.is_alive(id) then
        player.move(id, horizontal, vertical, state.game_over)
        local p = transform.get(id)
        position = string.format("(%.1f,%.1f)", p.x, p.y)
        if state.map_id and entity.is_alive(state.map_id) then
            local mt = transform.get(state.map_id)
            local cx = math.floor((p.x - mt.x) / state.tile_size)
            local cy = math.floor((p.y - mt.y) / state.tile_size)
            cell = string.format(
                "(%d,%d)%s",
                cx,
                cy,
                tostring(tilemap.is_solid(state.map_id, cx, cy))
            )
        end
        if not state.game_over and state.game_active then
            enemies.chase(id)
            if input.just_pressed("space") then
                player.attack(id)
            end
        end
        handle_save_load(id)
        player.choose_animation(id, horizontal, vertical)
        anim_state = animation.get(id)
        frame = animation.frame(id)
        hud.update_hp(health.get(id) / state.max_health)
        handle_events(id)
        update_status(id)
    end
    return string.format(
        "lua-update-ok dt=%.6f axis=(%.1f,%.1f) player=%s pos=%s cell=%s anim=%s frame=%d events=%d hp=%s scene=%s coins=%d/%d",
        dt,
        horizontal,
        vertical,
        tostring(id ~= nil),
        position,
        cell,
        anim_state,
        frame,
        state.event_count,
        (id and tostring(health.get(id)) or "?"),
        state.scene_name,
        state.score,
        state.coin_total
    )
end

--- 物理碰撞事件分发：史莱姆接触伤害 / 金币拾取
function handle_events(id)
    local events = physics.events()
    for _, e in ipairs(events) do
        if e.started then
            state.event_count = state.event_count + 1
            local other = nil
            if e.a == id then
                other = e.b
            elseif e.b == id then
                other = e.a
            end
            if other and state.game_active and not state.game_over then
                if state.is_in(state.enemies, other) then
                    on_player_hit(id)
                elseif state.is_in(state.coins, other) then
                    coins.pick(other)
                end
            end
        end
    end
end

function on_player_hit(id)
    local hp = player.damage(id, 20)
    local p = transform.get(id)
    particles.burst(p.x, p.y, 12)
    audio.play("hit")
    log.info("player hit: hp=" .. hp)
end

--- 胜负判定：全金币收集 VICTORY / 血量归零 DEFEATED
function update_status(id)
    local coins_left = state.alive_count(state.coins)
    if state.game_active and coins_left == 0 then
        state.game_over = true
        state.game_win = true
        hud.set_objective("VICTORY! Collected all " .. state.coin_total .. " coins - F4 for another arena")
    elseif health.get(id) <= 0 then
        state.game_over = true
        state.game_win = false
        hud.set_objective("DEFEATED! F1 save, F2 retry")
    end
end

--- 快捷键：F1 存 / F2 读 / F4 场景切换
function handle_save_load(id)
    if input.just_pressed("f1") then
        save_manager.save(id)
    elseif input.just_pressed("f2") then
        save_manager.load(id)
    elseif input.just_pressed("f4") then
        local target = (state.scene_name == "main") and "arena" or "main"
        if scene.load(target) then
            log.info("switching to scene '" .. target .. "'")
        end
    end
end

return M
