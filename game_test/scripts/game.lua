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
    -- 非 2D 游戏场景（如 3D 演示 demo3d）：不初始化游戏/菜单/存档
    if not state.map_id or not entity.is_alive(state.map_id) then
        state.game_active = false
        state.game_over = false
        state.game_win = false
        state.menu = false
        log.info("scene '" .. state.scene_name .. "' is a non-2D scene; game logic idle")
        return
    end
    if state.map_id and entity.is_alive(state.map_id) then
        local d = tilemap.get(state.map_id)
        if d then
            state.tile_size = d.tile_size
            log.info("scene '" .. state.scene_name .. "' tilemap " .. d.width .. "x" .. d.height)
        end
    end
    -- 死亡重玩：满血开局、跳过存档恢复、直接进入游戏
    if state.fresh_round then
        state.fresh_round = false
        enemies.find_all()
        coins.find_all()
        state.score = 0
        state.game_active = state.coin_total > 0
        state.game_over = false
        state.game_win = false
        state.menu = false
        player.set_hp(player.find(), state.max_health)
        audio.volume(0.8)
        if not state.music_started then
            audio.music("bgm")
            state.music_started = true
        end
        hud.init_objective()
        log.info("fresh round started in '" .. state.scene_name .. "'")
        return
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
    state.menu = true
    player.set_hp(player.find(), player.initial_hp())
    audio.volume(0.8)
    -- 背景音乐：启动时播放一次，跨场景切换保持循环
    if not state.music_started then
        audio.music("bgm")
        state.music_started = true
    end
    hud.init_menu()
end

function M.update()
    local dt = time.delta()
    handle_scene_switch()
    local horizontal = input.axis("horizontal")
    local vertical = input.axis("vertical")
    local id = player.find()
    local position = "?"
    local cell = "?"
    local anim_state = "?"
    local frame = -1
    if id and entity.is_alive(id) then
        player.update_facing(horizontal, vertical)
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
        -- 主菜单：SPACE 开始，开始前敌人不追击、不受伤
        if state.menu then
            if input.just_pressed("space") then
                state.menu = false
                hud.init_objective()
                log.info("game started")
            else
                hud.update_menu()
            end
        elseif not state.game_over and state.game_active then
            enemies.chase(id)
            if input.just_pressed("space") then
                player.attack(id)
            end
        end
        if not state.menu then
            handle_save_load(id)
            handle_events(id)
            update_status(id)
            -- 终局：显示重玩按钮，点击或按 R 重开一局
            if state.game_over then
                hud.show_retry_button()
                if input.just_pressed("r") or hud.retry_button_clicked() then
                    retry_round()
                end
            end
        end
        player.choose_animation(id, horizontal, vertical)
        anim_state = animation.get(id)
        frame = animation.frame(id)
        hud.update_hp(health.get(id) / state.max_health)
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
        hud.set_objective("VICTORY! R 再来一局  F4 换场")
    elseif health.get(id) <= 0 then
        state.game_over = true
        state.game_win = false
        hud.set_objective("DEFEATED! R 重玩  F1 存  F2 读")
    end
end

--- 死亡/胜利后重玩一局：满血重开当前场景
function retry_round()
    state.fresh_round = true
    if scene.load(state.scene_name) then
        log.info("retry: fresh round '" .. state.scene_name .. "'")
    end
end

--- 快捷键：F1 存 / F2 读（F4 场景切换见 handle_scene_switch）
function handle_save_load(id)
    if input.just_pressed("f1") then
        save_manager.save(id)
    elseif input.just_pressed("f2") then
        save_manager.load(id)
    end
end

--- F4 三场景循环：main -> arena -> game3d -> main（任意场景可切出）
function handle_scene_switch()
    if not input.just_pressed("f4") then
        return
    end
    local target = "main"
    if state.scene_name == "main" then
        target = "arena"
    elseif state.scene_name == "arena" then
        target = "game3d"
    end
    if scene.load(target) then
        log.info("switching to scene '" .. target .. "'")
    end
end

return M
