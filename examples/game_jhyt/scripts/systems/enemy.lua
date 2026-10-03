-- enemy.lua：《江湖有田》敌人执行与追击 AI（M18-01）。
--
-- 引擎事实（与 npc.lua 同一约束，见其文件头）：
--   * entity.spawn(name, persist_id) 只插入 Transform —— 敌人没有刚体，
--     追击/移动必须 transform.get/set 逐帧推进，绝不调用 physics.*。
--   * tilemap.find_path(id, fx, fy, tx, ty, false) 返回 1-based {x=,y=} 格
--     序列或 nil, reason；阻挡来自本图 GroundMap 的 solid 瓦片。
--   * 玩家是场景原生刚体（JianghuPlayer），位置经 transform.get 读取即可。
--
-- 模块职责：
--   * on_enter(map)：绑定本图 GroundMap，本图敌人生成/落位，他图敌人销毁。
--   * tick()：AI 状态机 idle → chase → attack：
--       - 玩家在 aggro_radius 内 → A* 逐帧追击（repath 节流）；
--       - 距离 <= attack_range 且冷却就绪 → 经 combat.attack 发起近战攻击
--         （hitbox 形状取招式数据，伤害用敌人自己的 damage 字段）；
--       - 玩家战败后停手（不鞭尸，combat 侧同样拦截）。
--   * 伤害账本：敌人 hp 以本模块为准（引擎 health 表仅镜像调试），
--     combat 命中回调 hit() 结算；hp<=0 → 死亡、销毁实体、记战斗日志。
--   * 受击反馈（M25）：hit() 收到 combat 传来的 { hitstun, knockback, dir_* }，
--     按敌人自身抵抗（data/enemies.lua 的 knockback_resist/hitstun_resist，
--     缺省取 data/combat.lua）应用硬直与击退（不穿墙、撤销当前寻路）；
--     硬直期间 tick 只递减计数，不追击不出手。
--   * capture/restore：由 main.lua 注册为存档 provider；reset() 供新档与
--     玩家战败重开（combat.respawn）复活全部敌人。
local state = require("core.state")
local player = require("player.player")
local combat = require("systems.combat")
local loot = require("systems.loot")
local enemies = require("data.enemies")
local enemy_art = require("data.enemy_art")
local combat_data = require("data.combat")
local map_manager = require("world.map_manager")
-- docs/48 effects 接线：敌人受击闪白。
local effects = require("systems.effects")

local M = {}

local TILE = 32
-- 追击路径重算节流（帧），避免每帧 A*。
local REPATH_TICKS = 20

local ground_id = nil
local width, height = 0, 0
local actors = {} -- persist_id → 运行时记录
local diagnostics = {}
local diag_keys = {}
local sorted_enemies = nil

local function enemy_list()
  if not sorted_enemies then
    sorted_enemies = enemies.list()
  end
  return sorted_enemies
end

local function current_map()
  return map_manager.current()
end

local function cell_center(cx, cy)
  return (cx + 0.5) * TILE, (cy + 0.5) * TILE
end

local function cell_of(id)
  local ok, pos = pcall(transform.get, id)
  if not ok or type(pos) ~= "table" then
    return nil
  end
  local cell = grid.world_to_cell(pos.x, pos.y, TILE)
  return { x = cell[1], y = cell[2] }
end

local function report(actor, key, message)
  if diag_keys[key] then
    return
  end
  diag_keys[key] = true
  diagnostics[#diagnostics + 1] = message
  log.warn("[enemy] " .. message)
end

local function record_for(enemy)
  local actor = actors[enemy.persist_id]
  if not actor then
    actor = { enemy = enemy, hp = enemy.hp, dead = false }
    actors[enemy.persist_id] = actor
  end
  return actor
end

-- 朝向字符串：取主轴方向（combat.hitbox 用）。
local function facing_of(dx, dy)
  if math.abs(dx) >= math.abs(dy) then
    if dx >= 0 then
      return "right"
    end
    return "left"
  end
  if dy >= 0 then
    return "up"
  end
  return "down"
end

-- 确保本图存在该敌人实体并落位（pending_pos 优先，其次 home 格心）。
local function activate(actor)
  if actor.dead then
    return nil
  end
  if actor.id and entity.is_alive(actor.id) then
    return actor.id
  end
  local existing = entity.find_id(actor.enemy.persist_id)
  if existing then
    actor.id = existing
  else
    local ok, id = pcall(entity.spawn, actor.enemy.name, actor.enemy.persist_id)
    if not ok or not id then
      report(actor, "spawn:" .. actor.enemy.persist_id,
        "enemy=" .. tostring(actor.enemy.name) .. " spawn failed: " .. tostring(id))
      return nil
    end
    actor.id = id
    -- 美术：优先 data/enemy_art.lua（单帧 + 序列帧 attach），退回 data/enemies.lua 的灰盒串。
    -- 4-15：art 挂载失败必须 report（否则敌人静默变灰盒且无诊断）。
    local applied = false
    if actor.enemy.art then
      local art_ok, art_err = pcall(enemy_art.apply, id, actor.enemy.art, actor.facing or "s")
      if art_ok then
        applied = true
      else
        report(actor, "art:" .. actor.enemy.art,
          "enemy=" .. tostring(actor.enemy.name) .. " art apply failed: " .. tostring(art_err))
      end
    end
    if not applied and actor.enemy.sprite then
      pcall(sprite.set, id, actor.enemy.sprite)
    end
    actor.animated = applied
  end
  local x, y
  if actor.pending_pos then
    x, y = actor.pending_pos.x, actor.pending_pos.y
    actor.pending_pos = nil
  elseif actor.cell then
    x, y = cell_center(actor.cell.x, actor.cell.y)
  else
    x, y = 0, 0
  end
  transform.set(actor.id, x, y)
  actor.cell = cell_of(actor.id) or actor.cell
  -- 气血镜像到引擎 health API（调试观察；账本在本模块）。
  pcall(health.set, actor.id, actor.hp or 0)
  return actor.id
end

local function despawn(actor)
  if actor.id and entity.is_alive(actor.id) then
    entity.destroy(actor.id)
  end
  actor.id = nil
  actor.path = nil
  actor.path_index = nil
  actor.target_cell = nil
end

-- 朝向：与 npc.lua 同一套（世界 y 朝上，+y 背对 / -y 面向镜头）。
-- 八向条带：两轴同时显著非零时取对角（se/sw/ne/nw），否则按主轴取四正。
-- 注意：玩法朝向 facing_of()（right/left/up/down）保持四向，combat.hitbox 依赖它。
local function facing_from(dx, dy)
  local ax, ay = math.abs(dx), math.abs(dy)
  if ax > 0.001 and ay > 0.001 then
    if dx > 0 and dy > 0 then return "ne" end
    if dx > 0 then return "se" end
    if dy > 0 then return "nw" end
    return "sw"
  end
  if ax >= ay then
    return dx >= 0 and "e" or "w"
  end
  return dy >= 0 and "n" or "s"
end

-- 动画状态：走路时 walk_<dir>，停下 idle_<dir>；受伤/死亡由攻击与战败流程切。
local function update_animation(actor, moving)
  if not actor.animated or not actor.id then
    return
  end
  local state_name = (moving and "walk_" or "idle_") .. (actor.facing or "s")
  pcall(animation.set, actor.id, state_name)
end

-- 播放一次性动作（attack/hurt/faint），播完由调用方切回 idle。
function M.play_state(persist_id, state_name)
  local actor = actors[persist_id]
  if not actor or not actor.animated or not actor.id then
    return false
  end
  return pcall(animation.set, actor.id, state_name) == true
end

-- ------------------------------------------------------------------
-- 受击反馈（M25）：硬直 + 击退（数值来自招式，抵抗来自敌人数据）
-- ------------------------------------------------------------------

local function resist_of(actor, field)
  local value = actor.enemy[field]
  if type(value) == "number" then
    return math.max(0, math.min(1, value))
  end
  if field == "knockback_resist" then
    return combat_data.enemy_knockback_resist()
  end
  return combat_data.enemy_hitstun_resist()
end

-- 该格是否被 solid 瓦片占据（击退不穿墙；与 building.lua 同判定）。
local function cell_solid(cx, cy)
  if not ground_id then
    return false
  end
  local ok, solid = pcall(tilemap.is_solid, ground_id, cx, cy)
  if not ok then
    return false
  end
  return solid == true
end

-- 把敌人推开（沿攻击者 → 目标方向），返回实际位移距离；被墙挡住时不穿墙。
local function push_actor(actor, distance, dir_x, dir_y)
  if distance <= 0 or not actor.id or not entity.is_alive(actor.id) then
    return 0, false
  end
  local ok, pos = pcall(transform.get, actor.id)
  if not ok or type(pos) ~= "table" then
    return 0, false
  end
  local nx, ny = pos.x + dir_x * distance, pos.y + dir_y * distance
  local cell = grid.world_to_cell(nx, ny, TILE)
  if cell_solid(cell[1], cell[2]) then
    return 0, true
  end
  transform.set(actor.id, nx, ny)
  actor.cell = { x = cell[1], y = cell[2] }
  return distance, false
end

-- 沿路径推进：与 npc.step_path 相同的预算算法（dt 秒 × speed）。
local function step_path(actor, dt)
  if not actor.path then
    update_animation(actor, false)
    return
  end
  local budget = (tonumber(actor.enemy.speed) or 0) * dt
  local guard = 0
  local moved = false
  while actor.path and budget > 0 and guard < 64 do
    guard = guard + 1
    local waypoint = actor.path[actor.path_index]
    if not waypoint then
      actor.path = nil
      actor.path_index = nil
      break
    end
    local pos = transform.get(actor.id)
    local tx, ty = cell_center(waypoint.x, waypoint.y)
    local dx, dy = tx - pos.x, ty - pos.y
    local dist = math.sqrt(dx * dx + dy * dy)
    if dist > 0.0001 then
      actor.facing = facing_from(dx, dy)
      moved = true
    end
    if dist <= budget or dist < 0.0001 then
      transform.set(actor.id, tx, ty)
      actor.cell = { x = waypoint.x, y = waypoint.y }
      budget = budget - dist
      actor.path_index = actor.path_index + 1
      if actor.path_index > #actor.path then
        actor.path = nil
        actor.path_index = nil
      end
    else
      transform.set(actor.id, pos.x + dx / dist * budget, pos.y + dy / dist * budget)
      budget = 0
    end
  end
  update_animation(actor, moved)
end

-- 追击：向玩家所在格 A*，逐帧推进；返回是否仍处于追击态。
local function chase(actor, dt, player_pos)
  if not ground_id then
    return false
  end
  local current = cell_of(actor.id)
  if not current then
    return false
  end
  actor.cell = current
  local target_cell = grid.world_to_cell(player_pos.x, player_pos.y, TILE)
  target_cell = { x = target_cell[1], y = target_cell[2] }
  if not actor.target_cell
    or actor.target_cell.x ~= target_cell.x
    or actor.target_cell.y ~= target_cell.y then
    actor.target_cell = target_cell
    actor.path = nil
    actor.path_index = nil
    actor.repath_wait = 0
  end
  if actor.repath_wait and actor.repath_wait > 0 then
    actor.repath_wait = actor.repath_wait - 1
  end
  if actor.path == nil and (actor.repath_wait or 0) <= 0 then
    actor.repath_wait = REPATH_TICKS
    local path, reason = tilemap.find_path(
      ground_id, current.x, current.y, target_cell.x, target_cell.y, false)
    if not path then
      report(actor, "path:" .. actor.enemy.persist_id .. ":" .. tostring(reason),
        "enemy=" .. tostring(actor.enemy.name)
        .. " chase failed reason=" .. tostring(reason)
        .. " from=" .. tostring(current.x) .. "," .. tostring(current.y)
        .. " to=" .. tostring(target_cell.x) .. "," .. tostring(target_cell.y))
      return false
    end
    actor.path = path
    actor.path_index = 2
    if #path <= 1 then
      actor.path = nil
      actor.path_index = nil
    end
  end
  step_path(actor, dt)
  return true
end

local function tick_actor(actor, here, dt)
  local def = actor.enemy
  -- 他图敌人：逻辑上不在本图，销毁实体等玩家到那张图再重建。
  if def.map ~= here then
    actor.map = def.map
    despawn(actor)
    return
  end
  actor.map = here
  if not activate(actor) then
    return
  end
  -- 硬直（M25）：被打断的这段时间既不追击也不出手，也不重算路径。
  if (actor.stun_frames or 0) > 0 then
    actor.stun_frames = actor.stun_frames - 1
    actor.state = "stunned"
    actor.path = nil
    actor.path_index = nil
    update_animation(actor, false)
    return
  end
  -- 玩家不在/战败：敌人停手（攻击已被 combat 拦截，这里连追击也停）。
  local pid = player.ensure()
  local player_pos = pid and transform.get(pid) or nil
  if not player_pos or combat.is_player_dead() then
    actor.path = nil
    actor.state = "idle"
    return
  end

  local pos = transform.get(actor.id)
  local dx, dy = player_pos.x - pos.x, player_pos.y - pos.y
  local dist = math.sqrt(dx * dx + dy * dy)
  actor.facing = facing_of(dx, dy)

  local aggro = tonumber(def.aggro_radius) or 0
  local attack_range = tonumber(def.attack_range) or 0
  local damage = tonumber(def.damage) or 0
  local can_attack = damage > 0 and attack_range > 0

  if can_attack and dist <= attack_range then
    actor.path = nil
    actor.state = "attack"
    if (actor.next_attack or 0) <= combat.frame() then
      local ok = combat.attack(actor.id, def.persist_id, def.name, "enemy",
        def.skill, actor.facing, damage)
      if ok then
        actor.next_attack = combat.frame() + (tonumber(def.attack_cooldown) or 0)
      end
    end
    return
  end

  if aggro > 0 and (tonumber(def.speed) or 0) > 0 and dist <= aggro then
    actor.state = "chase"
    chase(actor, dt, player_pos)
    return
  end

  actor.state = "idle"
  actor.path = nil
end

-- 同步：本图敌人落位/重建；他图与死亡的确保无实体。
function M.sync()
  local here = current_map()
  for _, enemy in ipairs(enemy_list()) do
    local actor = record_for(enemy)
    if not actor.map then
      actor.map = enemy.map
      actor.cell = { x = enemy.home.x, y = enemy.home.y }
    end
    if actor.map == here and ground_id and not actor.dead then
      activate(actor)
    else
      despawn(actor)
    end
  end
end

function M.on_enter(map_id)
  ground_id = entity.find("GroundMap")
  local info = ground_id and tilemap.get(ground_id)
  width = (info and info.width) or 0
  height = (info and info.height) or 0
  if not ground_id or width == 0 or height == 0 then
    log.warn("[enemy] 未找到 GroundMap（" .. tostring(map_id) .. "），敌人不可用")
    ground_id = nil
  end
  -- 场景重建后实体 id 失效：清空 id 重建（不能 destroy 旧 id）。
  for _, actor in pairs(actors) do
    actor.id = nil
    actor.path = nil
    actor.path_index = nil
    actor.target_cell = nil
  end
  M.sync()
end

function M.tick()
  if not ground_id then
    return
  end
  local here = current_map()
  local dt = tonumber(time.delta()) or 0
  for _, enemy in ipairs(enemy_list()) do
    local actor = actors[enemy.persist_id]
    if actor then
      tick_actor(actor, here, dt)
    end
  end
end

-- combat 命中回调：结算伤害，返回剩余气血（0 = 死亡）；非敌方实体返回 nil。
-- meta（M25）：{ hitstun, knockback, dir_x, dir_y } —— 命中后的硬直与击退，
-- 按敌人自身的抵抗系数（data/enemies.lua，缺省取 data/combat.lua）打折应用。
function M.hit(id, damage, meta)
  for _, actor in pairs(actors) do
    if actor.id and actor.id == id and entity.is_alive(actor.id) and not actor.dead then
      local hp = math.max(0, (actor.hp or 0) - (tonumber(damage) or 0))
      actor.hp = hp
      pcall(health.set, id, hp)
      if hp <= 0 then
        actor.dead = true
        actor.state = "dead"
        actor.stun_frames = 0
        actor.last_knockback = nil
        -- 死亡表现：先播倒地条带，再移除实体（没有动画时直接移除）
        if actor.animated then
          pcall(animation.set, id, "faint_" .. (actor.facing or "s"))
        end
        despawn(actor)
        -- 击杀掉落（M28）：铜钱当场入账、物品进"当日待入库"袋（data/drops.lua）。
        -- 没有掉落记录的敌人（训练木人）不写任何状态；掉落失败不能影响战斗结算。
        local enemy_id = actor.enemy and actor.enemy.persist_id or nil
        if enemy_id then
          pcall(loot.on_kill, enemy_id)
        end
      else
        local reaction = type(meta) == "table" and meta or {}
        local stun = math.floor((tonumber(reaction.hitstun) or 0)
          * (1 - resist_of(actor, "hitstun_resist")))
        actor.stun_frames = math.max(actor.stun_frames or 0, stun)
        local pushed, blocked = push_actor(actor,
          (tonumber(reaction.knockback) or 0) * (1 - resist_of(actor, "knockback_resist")),
          tonumber(reaction.dir_x) or 0, tonumber(reaction.dir_y) or 0)
        actor.last_knockback = { distance = pushed, blocked = blocked }
        -- 被打断：撤销当前寻路，硬直结束后重新决策（否则会沿旧路径滑回去）。
        actor.path = nil
        actor.path_index = nil
        actor.target_cell = nil
        if actor.animated then
          -- 受击：切一次 hurt_<dir>（非循环，播完由 step_path/chase 切回 idle/walk）
          actor.facing = actor.facing or "s"
          pcall(animation.set, id, "hurt_" .. actor.facing)
        end
        -- docs/48 effects：敌人受击闪白（取实体位置）。
        local ok, pos = pcall(transform.get, id)
        if ok and type(pos) == "table" then
          effects.spawn("hurt_flash", pos.x, pos.y)
        end
      end
      return hp
    end
  end
  return nil
end

function M.is_enemy(id)
  if not id then
    return false
  end
  for _, actor in pairs(actors) do
    if actor.id and actor.id == id and not actor.dead then
      return true
    end
  end
  return false
end

function M.name_of(id)
  for _, actor in pairs(actors) do
    if actor.id and actor.id == id then
      return actor.enemy.name
    end
  end
  return nil
end

--- 该敌人命中后给目标挂的**瞬时状态**种类（M34-03；数据源是 `data/enemies.lua` 的 `status`）。
--- 按 `persist_id` 查而不是实体 id：攻击记录里带的是稳定身份（实体随切图销毁）。
--- 未声明 → nil，所以"只有毒蝎会下毒"这件事写在数据里，不写在 if 里。
function M.status_of(persist_id)
  local enemy = enemies.get(persist_id)
  if not enemy or type(enemy.status) ~= "string" then
    return nil
  end
  return enemy.status
end

-- 复活全部敌人（新档 / 玩家战败重开）：hp 回满、位置回 home、状态清空。
function M.reset()
  local here = current_map()
  for _, enemy in ipairs(enemy_list()) do
    local actor = record_for(enemy)
    actor.dead = false
    actor.state = "idle"
    actor.hp = enemy.hp
    actor.cell = { x = enemy.home.x, y = enemy.home.y }
    actor.pending_pos = nil
    actor.next_attack = 0
    actor.stun_frames = 0
    actor.last_knockback = nil
    actor.path = nil
    actor.path_index = nil
    actor.target_cell = nil
    if actor.map == here and ground_id then
      actor.map = here
      if actor.id and entity.is_alive(actor.id) then
        local x, y = cell_center(actor.cell.x, actor.cell.y)
        transform.set(actor.id, x, y)
        pcall(health.set, actor.id, actor.hp)
      end
    else
      despawn(actor)
    end
  end
end

-- 查询入口（headless 验收/调试）。
function M.query(persist_id)
  local enemy = enemies.get(persist_id)
  if not enemy then
    return nil
  end
  local actor = actors[persist_id]
  if not actor then
    return nil
  end
  local cell = actor.cell
  if actor.id and entity.is_alive(actor.id) then
    cell = cell_of(actor.id) or cell
  end
  return {
    persist_id = persist_id,
    name = enemy.name,
    map = actor.map,
    hp = actor.hp,
    max_hp = enemy.hp,
    alive = not actor.dead,
    state = actor.state,
    cell_x = cell and cell.x or nil,
    cell_y = cell and cell.y or nil,
    -- 受击反馈（M25）：剩余硬直步数与最近一次被击退的位移。
    stun_frames = actor.stun_frames or 0,
    last_knockback = actor.last_knockback and actor.last_knockback.distance or 0,
    knockback_blocked = actor.last_knockback ~= nil and actor.last_knockback.blocked == true,
    -- 命中会撤销当前寻路（path_active=false 表示"追击被打断"）。
    path_active = actor.path ~= nil,
    live = actor.id ~= nil and entity.is_alive(actor.id) == true
  }
end

function M.diagnostics()
  local out = {}
  for i, message in ipairs(diagnostics) do
    out[i] = message
  end
  return out
end

-- 存档：逐个敌人记 hp/存活/格与精确坐标。
function M.capture()
  local snapshot = { version = 1, enemies = {} }
  for _, enemy in ipairs(enemy_list()) do
    local actor = actors[enemy.persist_id]
    if actor then
      local cell = actor.cell
      local pos = nil
      if actor.id and entity.is_alive(actor.id) then
        cell = cell_of(actor.id) or cell
        local ok, t = pcall(transform.get, actor.id)
        if ok and type(t) == "table" then
          pos = { x = t.x, y = t.y }
        end
      end
      snapshot.enemies[#snapshot.enemies + 1] = {
        persist_id = enemy.persist_id,
        hp = actor.hp,
        alive = not actor.dead,
        cell = cell and { x = cell.x, y = cell.y } or nil,
        pos = pos
      }
    end
  end
  return snapshot
end

function M.restore(data)
  if type(data) ~= "table" or type(data.enemies) ~= "table" then
    return
  end
  for _, entry in ipairs(data.enemies) do
    if type(entry) == "table" and type(entry.persist_id) == "string" then
      local enemy = enemies.get(entry.persist_id)
      if enemy then
        local actor = record_for(enemy)
        actor.hp = tonumber(entry.hp) or enemy.hp
        actor.dead = entry.alive == false
        actor.state = actor.dead and "dead" or "idle"
        actor.path = nil
        actor.path_index = nil
        actor.target_cell = nil
        actor.next_attack = 0
        actor.stun_frames = 0
        actor.last_knockback = nil
        actor.pending_pos = nil
        if type(entry.cell) == "table" then
          actor.cell = { x = entry.cell.x, y = entry.cell.y }
        end
        if type(entry.pos) == "table" then
          actor.pending_pos = { x = entry.pos.x, y = entry.pos.y }
        end
        if actor.id and entity.is_alive(actor.id) then
          if actor.pending_pos then
            transform.set(actor.id, actor.pending_pos.x, actor.pending_pos.y)
            actor.pending_pos = nil
            actor.cell = cell_of(actor.id) or actor.cell
          end
          pcall(health.set, actor.id, actor.hp)
        end
      end
    end
  end
  if ground_id then
    M.sync()
  end
end

-- 向 combat 注册目标协议（main.lua 先 require combat 再 require enemy，
-- 此处 combat 已完整加载，无循环依赖）。
combat.register_enemy({
  is_enemy = M.is_enemy,
  name_of = M.name_of,
  hit = M.hit,
  reset = M.reset,
  -- M34-03：命中给玩家挂哪种瞬时状态（种类在数据里，扣血与结算全在 combat/status 侧）。
  status_of = M.status_of
})

return M
