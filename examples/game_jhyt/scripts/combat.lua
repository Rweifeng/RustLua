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
--
-- 依赖约定：本模块不 require enemy.lua；enemy.lua 加载后调用
-- M.register_enemy 注入目标判定/伤害回调，避免循环依赖（main.lua 先
-- require combat 再 require enemy）。
local state = require("state")
local player = require("player")
local skills = require("skills")
local maps = require("data.maps")
local map_manager = require("map_manager")

local M = {}

local PLAYER_MAX_HP = 100
local LOG_LIMIT = 64
-- 目标身体半径容差：hitbox 贴身擦过也算命中（世界单位）。
local TARGET_BODY_RADIUS = 10
-- 攻击者身体半径：hitbox 从攻击者中心前伸 range，再加身体半径避免贴脸打空。
local ATTACKER_BODY_RADIUS = 14

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
local attacks = {} -- 进行中的攻击记录（数组）
local cooldowns = {} -- attacker_key:skill_id → 冷却结束帧
local log_entries = {} -- 战斗日志环形缓冲
local stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0 }
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

-- 伤害结算入口：区分玩家/敌方目标；其余实体（NPC/地图）不参与战斗。
-- 返回 remaining（敌方）或 true（玩家已结算）/ nil（目标不参与）。
local function apply_damage(atk, target_id)
  local pid = player.ensure()
  if pid and target_id == pid then
    if player_dead then
      return nil
    end
    player_hp = math.max(0, player_hp - atk.damage)
    stats.taken = stats.taken + atk.damage
    mirror_health(pid, player_hp)
    if player_hp <= 0 then
      player_dead = true
      stats.deaths = stats.deaths + 1
      player.stop()
      push_log({ event = "death", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = atk.damage, remaining = 0 })
      log.warn(string.format(
        "[combat] death target=player attacker=%s skill=%s frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), frame))
    else
      push_log({ event = "hit", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = atk.damage, remaining = player_hp })
      log.info(string.format(
        "[combat] hit attacker=%s target=player skill=%s damage=%d remaining=%d frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), atk.damage, player_hp, frame))
    end
    return true
  end
  if enemy_kind and enemy_kind.is_enemy(target_id) then
    -- 名字必须在结算前取：死亡路径会销毁实体并清空 id → is_enemy 失配。
    local target_display = enemy_kind.name_of(target_id) or tostring(target_id)
    local remaining = enemy_kind.hit(target_id, atk.damage, atk)
    if remaining == nil then
      return nil
    end
    if atk.attacker_kind == "player" then
      stats.dealt = stats.dealt + atk.damage
    end
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
  local ok, candidates = pcall(physics.overlap, rect.ax, rect.ay, rect.broad_radius)
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
    ready_at = frame + windup,
    expires_at = frame + windup + active,
    hits = {}
  }
  cooldowns[key] = frame + math.max(0, math.floor(tonumber(skill.cooldown) or 0))
  stats.attacks = stats.attacks + 1
  push_log({ event = "attack", attacker = attacker_name, target = nil,
    skill = skill_id, damage = math.max(0, tonumber(damage) or 0) })
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
      remaining = entry.remaining
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
    deaths = stats.deaths
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

-- 新档：气血/战败态/攻击窗/冷却/日志全部复位（enemy 复位由 enemy.reset 负责）。
function M.reset()
  player_hp = PLAYER_MAX_HP
  player_dead = false
  attacks = {}
  cooldowns = {}
  log_entries = {}
  stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0 }
  mirror_health(player.ensure(), player_hp)
end

-- 存档：只记玩家气血（敌人账本在 enemy provider；攻击窗/冷却不进档）。
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
  mirror_health(player.ensure(), player_hp)
end

return M
