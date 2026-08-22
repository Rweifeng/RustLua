-- 共享游戏状态与工具函数（各模块唯一的可变状态来源）。
-- 本模块不包含玩法逻辑，只存放跨模块共享的字段和纯工具函数。

local M = {}

-- 常量与运行时状态
M.player_name = "Player"
M.map_name = "Tilemap"
M.move_speed = 240
M.enemy_speed = 90
M.max_health = 100
M.tile_size = 32

M.map_id = nil
M.scene_name = "main"
M.event_count = 0
M.enemies = {}
M.coins = {}
M.coin_total = 0
M.score = 0
M.game_active = false
M.game_over = false
M.game_win = false
M.menu = true          -- 主菜单状态（SPACE 开始）
M.facing = { x = 0, y = -1 }  -- 攻击朝向（默认向下）
M.attack_range = 56
M.attack_cone = 0.707  -- cos(45°)，朝向锥形内才命中
M.save_loaded = false
M.music_started = false

--- 按前缀收集存活的实体（如 Slime1..N）
function M.find_all(prefix)
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

function M.is_in(list, id)
    for _, e in ipairs(list) do
        if e == id then
            return true
        end
    end
    return false
end

function M.alive_count(list)
    local n = 0
    for _, e in ipairs(list) do
        if entity.is_alive(e) then
            n = n + 1
        end
    end
    return n
end

--- 返回当前场景名；场景未记录时回退到缓存值
function M.current_scene()
    local loaded = scene.reload()
    return loaded or M.scene_name
end

return M
