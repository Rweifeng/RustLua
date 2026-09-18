-- combat.lua：近战战斗底座（M18-01）。
--
-- 设计（对应任务验收）：
--   * 固定时间步：所有计时（冷却/前摇/判定窗/帧号）都以 60Hz 固定步计数，
--     不读真实时间；time.delta() 恒为 1/60，30/60/120 FPS 下同输入同结果。
--   * 命中去重：每次攻击生成 attack_id（attacker:skill:序号）与判定窗
--     [ready_at, expires_at]；窗内每步扫描 hitbox，但同一攻击对同一目标
--     只结算一次（hits 集合），窗结束记录即弃。
--   * hitbox：攻击者朝向单位向量 + 招式 range/width 组成的矩形（显式判定，
--     不重复实现 Rapier）；用 physics.overlap（Transform 圆查询）做宽相，
--     再做矩形精确判定。目标给 10 单位身体半径容差。
--   * 真实伤害/气血账本在 Lua 侧（引擎 health 表是内存态且按实体 id 记账，
--     跨图实体重建后 id 失效）：玩家气血在本模块，敌人在 enemy.lua，
--     两边镜像写入引擎 health API 仅供调试观察。
--   * 战斗日志：attack/hit/kill/death/respawn 事件环形记录（64 条），
--     每条含帧号、攻击者、目标、招式与伤害——关键命中和死亡可追踪。
--   * 可查询：frame/log/stats/hitbox_rect/last_attack 供 headless 验收与
--     后续编辑器调试面板使用。
--   * 受击反馈（M25）：命中后目标获得硬直（固定步数）与击退（世界单位位移），
--     数值全部来自 `data/skills.lua` 的招式记录；玩家额外获得无敌帧
--     （`data/combat.lua:player.iframes`），无敌期间整次命中被吸收并记 dodge。
--     敌人侧的抵抗（如训练木人完全不动）在 `data/enemies.lua` 按个体声明，
--     由 enemy.lua 应用；本模块只负责"算出这一击造成什么"。
--
-- 依赖约定：本模块不 require enemy.lua；enemy.lua 加载后调用
-- M.register_enemy 注入目标判定/伤害回调，避免循环依赖（main.lua 先
-- require combat 再 require enemy）。
local state = require("core.state")
local player = require("player.player")
local skills = require("player.skills")
local maps = require("data.maps")
local map_manager = require("world.map_manager")
local combat_data = require("data.combat")
-- M19-02：命中/受击音效（命中结算处播放，缺文件/无设备静默降级）。
local audio = require("core.audio")

local M = {}

local PLAYER_MAX_HP = 100
local LOG_LIMIT = 64
-- 目标身体半径容差：hitbox 贴身擦过也算命中（世界单位）。
local TARGET_BODY_RADIUS = 10
-- 攻击者身体半径：hitbox 从攻击者中心前伸 range，再加身体半径避免贴脸打空。
local ATTACKER_BODY_RADIUS = 14
-- 世界格边长（与 farm/enemy/map 同一约定：32）。
local TILE = 32

-- 朝向 → 单位向量（y 朝上，与 grid/tilemap cell 同约定）。
local FACING_VECTORS = {
  right = { x = 1, y = 0 },
  left = { x = -1, y = 0 },
  up = { x = 0, y = 1 },
  down = { x = 0, y = -1 }
}

local frame = 0
local attack_seq = 0
local player_hp = PLAYER_MAX_HP
local player_dead = false
-- 玩家受击反馈（M25）：硬直剩余步数 / 无敌帧剩余步数 / 最近一次被击退的位移。
local player_hurt = 0
local player_iframes = 0
local last_knockback = nil
local attacks = {} -- 进行中的攻击记录（数组）
local cooldowns = {} -- attacker_key:skill_id → 冷却结束帧
local log_entries = {} -- 战斗日志环形缓冲
local stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0, ignored = 0, hits = 0 }
-- enemy.lua 注入的目标协议：
--   is_enemy(id) → bool
--   name_of(id) → string|nil
--   hit(id, damage, meta) → remaining（0 = 死亡；nil = 非敌方目标）
--   reset() → 复活全部敌人（玩家重开时调用）
local enemy_kind = nil

local function push_log(entry)
  entry.frame = frame
  log_entries[#log_entries + 1] = entry
  while #log_entries > LOG_LIMIT do
    table.remove(log_entries, 1)
  end
  return entry
end

local function mirror_health(id, value)
  if not id then
    return
  end
  pcall(health.set, id, value)
end

local function facing_vector(facing)
  return FACING_VECTORS[facing] or FACING_VECTORS.down
end

-- 纯函数：给定位置/朝向/招式，返回 hitbox 矩形（调试/编辑器预览用）。
-- anchor = 中心 + dir*(range/2)；half_along 含攻击者身体半径。
function M.hitbox_rect(skill_id, facing, x, y)
  local skill = skills.get(skill_id)
  if not skill or type(x) ~= "number" or type(y) ~= "number" then
    return nil
  end
  local dir = facing_vector(facing)
  local range = tonumber(skill.range) or 0
  local width = tonumber(skill.width) or 0
  local half_along = range / 2 + ATTACKER_BODY_RADIUS
  local half_across = width / 2
  return {
    ax = x + dir.x * range / 2,
    ay = y + dir.y * range / 2,
    dir_x = dir.x,
    dir_y = dir.y,
    half_along = half_along,
    half_across = half_across,
    broad_radius = half_along + half_across
  }
end

-- 暴击/兜底都不存在：判定 = |along| <= half_along + TARGET_BODY_RADIUS
-- 且 |across| <= half_across + TARGET_BODY_RADIUS。
local function in_hitbox(rect, tx, ty)
  local dx, dy = tx - rect.ax, ty - rect.ay
  local along = dx * rect.dir_x + dy * rect.dir_y
  local across = dx * rect.dir_y - dy * rect.dir_x
  return math.abs(along) <= rect.half_along + TARGET_BODY_RADIUS
    and math.abs(across) <= rect.half_across + TARGET_BODY_RADIUS
end

local function target_name(id)
  if enemy_kind and enemy_kind.is_enemy(id) then
    return enemy_kind.name_of(id) or tostring(id)
  end
  return "player"
end

-- ------------------------------------------------------------------
-- 受击反馈（M25）：硬直 / 无敌帧 / 击退
-- ------------------------------------------------------------------

-- 落点是否被 solid 瓦片挡住（击退不穿墙）。
local function cell_blocked(x, y)
  local ground = entity.find("GroundMap")
  if not ground then
    return false
  end
  local cell = grid.world_to_cell(x, y, TILE)
  local ok, solid = pcall(tilemap.is_solid, ground, cell[1], cell[2])
  if not ok then
    return false
  end
  return solid == true
end

-- 击退方向：攻击者 → 目标（单位向量）。贴身重合时退化为攻击者朝向。
local function knockback_direction(atk, target_id)
  local from = nil
  if atk.attacker and entity.is_alive(atk.attacker) then
    local ok, pos = pcall(transform.get, atk.attacker)
    if ok and type(pos) == "table" then
      from = pos
    end
  end
  local to = nil
  local ok, pos = pcall(transform.get, target_id)
  if ok and type(pos) == "table" then
    to = pos
  end
  if not from or not to then
    local dir = facing_vector(atk.facing)
    return dir.x, dir.y
  end
  local dx, dy = to.x - from.x, to.y - from.y
  local len = math.sqrt(dx * dx + dy * dy)
  if len < 0.001 then
    local dir = facing_vector(atk.facing)
    return dir.x, dir.y
  end
  return dx / len, dy / len
end

-- 把玩家推开：直接位移（玩家是刚体，位移比给速度更确定，也便于固定步验收）。
-- 被墙挡住时不穿墙——位移记 0，但 last_knockback 仍记录"打到了墙"。
local function push_player(dir_x, dir_y, distance)
  if distance <= 0 then
    return 0
  end
  local pid = player.ensure()
  if not pid then
    return 0
  end
  local ok, pos = pcall(transform.get, pid)
  if not ok or type(pos) ~= "table" then
    return 0
  end
  local nx, ny = pos.x + dir_x * distance, pos.y + dir_y * distance
  if cell_blocked(nx, ny) then
    last_knockback = { dx = 0, dy = 0, distance = distance, blocked = true }
    return 0
  end
  player.teleport(nx, ny)
  player.stop()
  last_knockback = { dx = nx - pos.x, dy = ny - pos.y, distance = distance, blocked = false }
  return distance
end

-- 命中目标的受击反应元数据（交给 enemy.lua 按自身抵抗系数应用）。
local function hit_reaction(atk, target_id)
  local dir_x, dir_y = knockback_direction(atk, target_id)
  return {
    skill = atk.skill,
    attacker_key = atk.attacker_key,
    attacker_kind = atk.attacker_kind,
    hitstun = math.max(0, math.floor(tonumber(atk.hitstun) or 0)),
    knockback = math.max(0, tonumber(atk.knockback) or 0),
    dir_x = dir_x,
    dir_y = dir_y
  }
end

-- 伤害结算入口：区分玩家/敌方目标；其余实体（NPC/地图）不参与战斗。
-- 返回 remaining（敌方）或 true（玩家已结算）/ nil（目标不参与）。
local function apply_damage(atk, target_id)
  local pid = player.ensure()
  if pid and target_id == pid then
    if player_dead then
      return nil
    end
    -- 无敌帧：吸收整次命中（伤害/硬直/击退都不生效），记 dodge 便于追踪。
    if player_iframes > 0 then
      stats.ignored = stats.ignored + 1
      push_log({ event = "dodge", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = 0, remaining = player_hp })
      log.info(string.format(
        "[combat] dodge attacker=%s target=player skill=%s iframes=%d frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), player_iframes, frame))
      return true
    end
    local dir_x, dir_y = knockback_direction(atk, target_id)
    player_hp = math.max(0, player_hp - atk.damage)
    stats.taken = stats.taken + atk.damage
    stats.hits = stats.hits + 1
    mirror_health(pid, player_hp)
    audio.sfx("sfx_hit")
    if player_hp <= 0 then
      player_dead = true
      stats.deaths = stats.deaths + 1
      -- 战败清空受击反馈：死亡冻结不该再叠硬直/无敌帧。
      player_hurt = 0
      player_iframes = 0
      last_knockback = nil
      player.stop()
      -- 主角形象：倒地（非循环条带，播完由 player.tick 落回 idle/walk）
      player.play("faint")
      push_log({ event = "death", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = atk.damage, remaining = 0 })
      log.warn(string.format(
        "[combat] death target=player attacker=%s skill=%s frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), frame))
    else
      -- 受击反馈三件套：硬直（打断操作）+ 无敌帧（防连击致死）+ 击退（真实位移）。
      player_hurt = math.max(player_hurt, math.floor(tonumber(atk.hitstun) or 0))
      player_iframes = combat_data.player_iframes()
      local pushed = push_player(dir_x, dir_y, tonumber(atk.knockback) or 0)
      push_log({ event = "hit", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = atk.damage, remaining = player_hp,
        knockback = pushed, hitstun = player_hurt })
      log.info(string.format(
        "[combat] hit attacker=%s target=player skill=%s damage=%d remaining=%d knockback=%d hitstun=%d iframes=%d frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), atk.damage, player_hp,
        pushed, player_hurt, player_iframes, frame))
      -- 主角形象：受击（非循环；faint 已在上面的致死分支播过）
      player.play("hurt")
    end
    return true
  end
  if enemy_kind and enemy_kind.is_enemy(target_id) then
    -- 名字必须在结算前取：死亡路径会销毁实体并清空 id → is_enemy 失配。
    local target_display = enemy_kind.name_of(target_id) or tostring(target_id)
    local remaining = enemy_kind.hit(target_id, atk.damage, hit_reaction(atk, target_id))
    if remaining == nil then
      return nil
    end
    if atk.attacker_kind == "player" then
      stats.dealt = stats.dealt + atk.damage
    end
    audio.sfx("sfx_hit")
    if remaining <= 0 then
      stats.kills = stats.kills + 1
      push_log({ event = "kill", attacker = atk.attacker_name, target = target_display,
        skill = atk.skill, damage = atk.damage, remaining = 0 })
      log.info(string.format(
        "[combat] kill attacker=%s target=%s skill=%s damage=%d frame=%d",
        tostring(atk.attacker_name), target_display, tostring(atk.skill), atk.damage, frame))
    else
      push_log({ event = "hit", attacker = atk.attacker_name, target = target_display,
        skill = atk.skill, damage = atk.damage, remaining = remaining })
      log.info(string.format(
        "[combat] hit attacker=%s target=%s skill=%s damage=%d remaining=%d frame=%d",
        tostring(atk.attacker_name), target_display, tostring(atk.skill),
        atk.damage, remaining, frame))
    end
    return remaining
  end
  return nil
end

-- 判定窗内的一步扫描：宽相 overlap → 矩形精确判定 → 去重结算。
local function resolve_hits(atk)
  if atk.hits == nil then
    return
  end
  if not entity.is_alive(atk.attacker) then
    atk.hits = nil -- 攻击者已随切图销毁：整窗作废
    return
  end
  local pos = transform.get(atk.attacker)
  if type(pos) ~= "table" then
    atk.hits = nil
    return
  end
  local rect = M.hitbox_rect(atk.skill, atk.facing, pos.x, pos.y)
  if not rect then
    return
  end
  -- 场景空间宽相：按实体 Transform 位置筛候选，再用精确 hitbox 判定。
  -- 注意名字：`physics.overlap` 是**碰撞体**查询（与 physics3d 同义），本处要的是位置宽相，
  -- 因此用 `physics.positions_in_radius`（docs/54 §8-⑥：同名必同义）。
  local ok, candidates = pcall(physics.positions_in_radius, rect.ax, rect.ay, rect.broad_radius)
  if not ok or type(candidates) ~= "table" then
    return
  end
  for _, candidate in ipairs(candidates) do
    if candidate ~= atk.attacker and not atk.hits[candidate] then
      local target_pos = transform.get(candidate)
      if type(target_pos) == "table" and in_hitbox(rect, target_pos.x, target_pos.y) then
        local result = apply_damage(atk, candidate)
        if result ~= nil then
          atk.hits[candidate] = true
        end
      end
    end
  end
end

-- 固定步推进：帧号 +1 → 结算判定窗内的攻击 → 清理过期窗口。
function M.tick()
  frame = frame + 1
  -- 受击反馈计时先递减再结算命中：本步被打中时写入满值，
  -- 因此"命中那一步读到的硬直/无敌帧步数"就是数据里的原值（可精确断言）。
  if player_hurt > 0 then
    player_hurt = player_hurt - 1
  end
  if player_iframes > 0 then
    player_iframes = player_iframes - 1
  end
  local keep = {}
  for _, atk in ipairs(attacks) do
    if atk.hits and frame >= atk.ready_at and frame <= atk.expires_at then
      resolve_hits(atk)
    end
    if atk.hits and frame <= atk.expires_at then
      keep[#keep + 1] = atk
    end
  end
  attacks = keep
end

-- 注册一次攻击（玩家与敌人共用底座）：
--   attacker_id     攻击者实体 id（判定中心）
--   attacker_key    冷却键（稳定身份：玩家 "player"，敌人 persist_id）
--   attacker_name   日志显示名
--   attacker_kind   "player" | "enemy"
--   skill_id        招式（data/skills.lua）
--   facing          朝向字符串 right/left/up/down
--   damage          本次攻击伤害（调用方算好：玩家走 skills.player_damage，
--                   敌人用自己的 damage 字段）
-- 返回 true,"ok" 或 false,reason（cooldown/unknown_skill/dead/mode/...）。
function M.attack(attacker_id, attacker_key, attacker_name, attacker_kind, skill_id, facing, damage)
  local skill = skills.get(skill_id)
  if not skill then
    return false, "unknown_skill"
  end
  if attacker_kind == "player" then
    if player_dead then
      return false, "dead"
    end
    if not state.is_mode("Playing") then
      return false, "mode"
    end
    -- 硬直期间不能出手（移动/交互由 input.lua 同样拦住）。
    if player_hurt > 0 then
      return false, "stunned"
    end
  elseif attacker_kind == "enemy" and player_dead then
    return false, "player_dead" -- 不鞭尸：玩家已战败，敌人停手
  end
  local key = tostring(attacker_key) .. ":" .. tostring(skill_id)
  if (cooldowns[key] or 0) > frame then
    return false, "cooldown"
  end
  local cost = skills.inner_cost(skill_id)
  if cost > 0 then
    local inner = tonumber(state.inner_power) or 0
    if inner < cost then
      return false, "inner_power"
    end
    state.inner_power = inner - cost
  end
  local windup = math.max(0, math.floor(tonumber(skill.windup) or 0))
  local active = math.max(1, math.floor(tonumber(skill.active) or 1))
  attack_seq = attack_seq + 1
  attacks[#attacks + 1] = {
    id = tostring(attacker_key) .. ":" .. tostring(skill_id) .. ":" .. tostring(attack_seq),
    attacker = attacker_id,
    attacker_key = tostring(attacker_key),
    attacker_name = tostring(attacker_name),
    attacker_kind = attacker_kind,
    skill = skill_id,
    facing = FACING_VECTORS[facing] and facing or "down",
    damage = math.max(0, tonumber(damage) or 0),
    -- 受击反馈来自招式数据（M25）：命中时按这一招推多远/打断多久结算。
    hitstun = math.max(0, math.floor(tonumber(skill.hitstun) or 0)),
    knockback = math.max(0, tonumber(skill.knockback) or 0),
    ready_at = frame + windup,
    expires_at = frame + windup + active,
    hits = {}
  }
  cooldowns[key] = frame + math.max(0, math.floor(tonumber(skill.cooldown) or 0))
  stats.attacks = stats.attacks + 1
  push_log({ event = "attack", attacker = attacker_name, target = nil,
    skill = skill_id, damage = math.max(0, tonumber(damage) or 0) })
  -- 主角形象：普攻/武学起手就播 attack_<dir>（与判定窗口同帧起算，观感对齐）
  if attacker_kind == "player" then
    player.play("attack")
  end
  return true, "ok"
end

-- 玩家普攻/武学入口（input.lua 的 J/K 键）：伤害由 skills.player_damage 计算。
function M.player_attack(skill_id)
  local pid = player.ensure()
  if not pid then
    return false, "no_player"
  end
  local damage = skills.player_damage(skill_id)
  return M.attack(pid, "player", "player", "player", skill_id, state.facing, damage)
end

-- 玩家气血查询/镜像（HUD 与 headless 断言用）。
function M.player_hp()
  return player_hp
end

function M.player_max_hp()
  return PLAYER_MAX_HP
end

function M.is_player_dead()
  return player_dead
end

-- 受击反馈查询（HUD/headless 验收/调试面板用）。
function M.player_hurt_frames()
  return player_hurt
end

function M.player_iframes()
  return player_iframes
end

-- 玩家能否操作（移动/攻击/交互）：硬直期间为 false；战败期间由 is_player_dead 单独拦。
function M.player_can_act()
  return player_hurt <= 0 and not player_dead
end

-- 最近一次玩家被击退的位移（含 blocked：被墙挡住时位移为 0）。
function M.last_knockback()
  if not last_knockback then
    return nil
  end
  return {
    dx = last_knockback.dx,
    dy = last_knockback.dy,
    distance = last_knockback.distance,
    blocked = last_knockback.blocked
  }
end

-- headless/调试：直接设置玩家气血（0 触发战败流程，便于确定性验收）。
function M.set_player_hp(value)
  local hp = math.max(0, tonumber(value) or 0)
  player_hp = math.min(PLAYER_MAX_HP, hp)
  mirror_health(player.ensure(), player_hp)
  if player_hp <= 0 and not player_dead then
    player_dead = true
    stats.deaths = stats.deaths + 1
    player.stop()
    push_log({ event = "death", attacker = "debug", target = "player",
      skill = "-", damage = 0, remaining = 0 })
  end
  return player_hp
end

-- 战败重开（R 键 / headless）：气血回满、回到当前地图出生点、敌人全部复活。
function M.respawn()
  if not player_dead then
    return false, "not_dead"
  end
  player_hp = PLAYER_MAX_HP
  player_dead = false
  local pid = player.ensure()
  mirror_health(pid, player_hp)
  local map_id = map_manager.current()
  local definition = maps.get(map_id)
  if definition and pid then
    local wx, wy = maps.to_world({ to_cell = definition.spawn })
    player.teleport(wx, wy)
  end
  if enemy_kind then
    enemy_kind.reset()
  end
  push_log({ event = "respawn", attacker = nil, target = "player",
    skill = "-", damage = 0, remaining = player_hp })
  log.info(string.format("[combat] respawn map=%s frame=%d", tostring(map_id), frame))
  return true, "ok"
end

-- enemy.lua 注入目标协议（见文件头注释）。
function M.register_enemy(handler)
  if type(handler) ~= "table"
    or type(handler.is_enemy) ~= "function"
    or type(handler.hit) ~= "function" then
    return false, "invalid enemy handler"
  end
  enemy_kind = handler
  return true
end

-- 战斗日志只读副本（headless 验收/调试）。
function M.log()
  local out = {}
  for i, entry in ipairs(log_entries) do
    out[i] = {
      frame = entry.frame,
      event = entry.event,
      attacker = entry.attacker,
      target = entry.target,
      skill = entry.skill,
      damage = entry.damage,
      remaining = entry.remaining,
      knockback = entry.knockback,
      hitstun = entry.hitstun
    }
  end
  return out
end

function M.stats()
  return {
    attacks = stats.attacks,
    dealt = stats.dealt,
    taken = stats.taken,
    kills = stats.kills,
    deaths = stats.deaths,
    -- 被无敌帧吸收的命中次数（0 说明无敌帧没生效或没被连击）。
    ignored = stats.ignored,
    -- 真实打在玩家身上的命中次数（taken 是伤害总和，次数要单独记：
    -- 无敌帧有没有封顶掉血速度，只能看次数）。
    hits_taken = stats.hits
  }
end

function M.frame()
  return frame
end

function M.last_attack()
  local atk = attacks[#attacks]
  if not atk then
    return nil
  end
  return {
    id = atk.id,
    attacker = atk.attacker_name,
    kind = atk.attacker_kind,
    skill = atk.skill,
    facing = atk.facing,
    damage = atk.damage,
    ready_at = atk.ready_at,
    expires_at = atk.expires_at
  }
end

-- 新档：气血/战败态/攻击窗/冷却/日志/受击反馈全部复位（enemy 复位由 enemy.reset 负责）。
function M.reset()
  player_hp = PLAYER_MAX_HP
  player_dead = false
  player_hurt = 0
  player_iframes = 0
  last_knockback = nil
  attacks = {}
  cooldowns = {}
  log_entries = {}
  stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0, ignored = 0, hits = 0 }
  mirror_health(player.ensure(), player_hp)
end

-- 存档：只记玩家气血（敌人账本在 enemy provider；攻击窗/冷却/硬直/无敌帧/击退
-- 都是**瞬时状态**，不进档——读档后玩家站在安全位置，不该继承上一局的僵直）。
function M.capture()
  return { version = 1, hp = player_hp, dead = player_dead }
end

function M.restore(data)
  if type(data) ~= "table" then
    return
  end
  local hp = tonumber(data.hp)
  if hp then
    player_hp = math.max(0, math.min(PLAYER_MAX_HP, hp))
  end
  player_dead = data.dead == true and player_hp <= 0
  player_hurt = 0
  player_iframes = 0
  last_knockback = nil
  mirror_health(player.ensure(), player_hp)
end

return M
