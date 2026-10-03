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
--   * 伤害公式（docs/33 §6.1，M34-04b）：玩家受击按**平减在前、比例在后**结算——
--     先 `min(原始, max(1, 原始 - defense))`（defense 来自 `player/stats.lua` 三层模型），
--     正面格挡再乘 `(1 - absorb)` 并向下取整。DOT 不吃这两层（见 settle_status_damage）。
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
-- M34-03：瞬时状态（中毒等）的账本。本模块是它的**唯一推进者**（每固定步一次 tick）
-- 和唯一扣血者——status.lua 只产出事件、不碰气血，所以"毒能毒死人，但走的是同一套
-- 战败流程"是结构性的。方向是单向的：combat require status，status 不 require combat。
local status = require("systems.status")
-- M34-04b：防御属性。伤害公式从三层属性模型读 `defense`（基础 + 装备 + Buff，
-- 见 `player/stats.lua`）。名字用 `stats_model` 是因为本模块已经有一个局部 `stats`
-- 是战斗账本（见下方 `local stats = { attacks = … }`），两者不能同名。
local stats_model = require("player.stats")
-- 玩家侧状态的持有者键（敌人侧将来再扩：今天没有敌人会流血）。
local PLAYER_ACTOR = "player"
-- M19-02：命中/受击音效（命中结算处播放，缺文件/无设备静默降级）。
local audio = require("core.audio")
-- docs/48 effects 接线：攻击/命中/受击/掌风挂特效条带。
local effects = require("systems.effects")

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
local last_dash = nil -- 最近一次轻功的结算（格数/位移/当步无敌帧），headless 与调试面板读它
-- 格挡（M34-04 / 守心诀）：`guard_wanted` 是输入层每步报上来的"按没按"，
-- `guard_active` 是本模块结算出来的实际状态（内力不够就自动落防）。
-- `guard_charge` = 离下一次扣内力还剩几步（docs/33 §6.1 的「10/秒」= 每 60 步扣 10）。
-- 与硬直/无敌帧同一类**瞬时状态**：不进存档，战败/重开/读档一律落防。
local guard_wanted = false
local guard_active = false
local guard_charge = 0
local guard_skill_id = nil
local guard_absorbed = 0

local attacks = {} -- 进行中的攻击记录（数组）
local cooldowns = {} -- attacker_key:skill_id → 冷却结束帧
local log_entries = {} -- 战斗日志环形缓冲
-- dashes/guard_hits/guard_absorbed（M34-04）：轻功与格挡各记自己的账，**不混进 attacks**
-- ——追风步一下都没打中任何人，把它算成"攻击次数"会让 m28 的每刀读数失真。
local stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0, ignored = 0,
  hits = 0, dashes = 0, guard_hits = 0, guard_absorbed = 0 }
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

-- 轻功位移（M34-04 / 追风步）：**逐格走、第一格被挡就停**，不是"落点被挡就整个不动"。
-- 这是 9277524（WebGL 固体格碰撞）立下的语义在轻功侧的对应物：贴墙按轻功应当"挪一格"，
-- 而不是原地烧掉 15 点内力什么也没发生。返回实际位移的格数（0 = 四面都不通，此时调用方
-- 不扣内力、不进冷却——什么都没发生就不该收费）。
local function dash_player(dir_x, dir_y, tiles)
  local pid = player.ensure()
  if not pid or tiles <= 0 then
    return 0, 0
  end
  local ok, pos = pcall(transform.get, pid)
  if not ok or type(pos) ~= "table" then
    return 0, 0
  end
  local moved = 0
  local x, y = pos.x, pos.y
  for _ = 1, tiles do
    local nx, ny = x + dir_x * TILE, y + dir_y * TILE
    if cell_blocked(nx, ny) then
      break
    end
    x, y, moved = nx, ny, moved + 1
  end
  if moved == 0 then
    return 0, 0
  end
  player.teleport(x, y)
  player.stop()
  return moved, moved * TILE
end

-- 正面判定（M34-04 / 守心诀）：攻击来源落在以玩家朝向为轴、张角 front_degrees 的扇形内
-- 才算"正面"。用 `knockback_direction`（攻击者 → 玩家）取反得到"玩家 → 攻击者"，
-- 再与朝向单位向量点乘比 cos(半角)——**没有新的计时器，也没有新的坐标通道**，
-- 用的就是击退那一条已经算好的方向。
local function is_front_hit(atk, target_id, front_degrees)
  local dir = facing_vector(state.facing)
  local dx, dy = knockback_direction(atk, target_id)
  local len = math.sqrt(dx * dx + dy * dy)
  if len < 0.001 then
    -- 攻击者与玩家重合（贴脸/寻路卡住）：判成正面，宁可对玩家有利。
    return true
  end
  local to_attacker_x, to_attacker_y = -dx / len, -dy / len
  local dot = dir.x * to_attacker_x + dir.y * to_attacker_y
  local half = math.rad(tonumber(front_degrees) or 120) / 2
  return dot >= math.cos(half)
end


-- 当前生效的格挡招式（没在格挡/招式记录被删则为 nil）。姿态与数据之间只留这一个通道，
-- 于是"减伤比例/正面角度/每期消耗"三件事在代码里都只有数据来源，没有第二个常数。
local function active_guard()
  if not guard_active or not guard_skill_id then
    return nil
  end
  return skills.get(guard_skill_id)
end

-- 目标的防御平减（M34-04b）：三层属性模型（基础 + 装备 + Buff）合成出的 defense。
-- **只有玩家侧有这一层**：`data/enemies.lua` 没有任何 defense 字段（docs/33 §6.2 的
-- 防御列写的是 Alpha 目标值，不是实现值），所以敌方受击仍直吃 atk.damage，
-- 见 apply_damage 的敌方分支与 jhyt_m34_skills ④ 的那一条对照断言。
local function defense_of(actor)
  return math.max(0, math.floor(tonumber(stats_model.query(actor, "defense")) or 0))
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

-- 攻击方声明的"打中挂什么状态"（M34-03）：种类写在 `data/enemies.lua` 的 `status` 字段，
-- 数值写在 `data/combat.lua` 的 `status` 表，本模块只按攻击者的稳定身份去问一句
-- （敌人个体差异因此不落到 if 分支上，加第二个带状态的敌人只改数据）。
local function attacker_status_kind(atk)
  if atk.attacker_kind ~= "enemy" then
    return nil
  end
  if not enemy_kind or type(enemy_kind.status_of) ~= "function" then
    return nil
  end
  return enemy_kind.status_of(atk.attacker_key)
end

-- 玩家战败的统一收口（M34-03 从命中结算里抽出来）：**直接被打死与被子弹/毒跳死走同一条**。
-- 冻结受击反馈、清掉瞬时状态（死人身上不该留着还在掉血的毒，否则复活后继续掉血），
-- 播倒地、记 death 日志。
local function fall_player(source_name, skill_id, damage)
  player_dead = true
  stats.deaths = stats.deaths + 1
  player_hurt = 0
  player_iframes = 0
  last_knockback = nil
  -- 倒地就**当场**落防，不等下一个 tick：死人身上不该挂着"正面减伤 70%"，
  -- 否则死亡动画期间补到的每一刀仍然按减伤后的数字结算。
  guard_wanted = false
  guard_active = false
  guard_charge = 0
  status.clear(PLAYER_ACTOR)
  player.stop()
  -- 主角形象：倒地（非循环条带，播完由 player.tick 落回 idle/walk）
  player.play("faint")
  push_log({ event = "death", attacker = source_name, target = "player",
    skill = skill_id, damage = damage, remaining = 0 })
  log.warn(string.format(
    "[combat] death target=player attacker=%s skill=%s frame=%d",
    tostring(source_name), tostring(skill_id), frame))
end

-- 状态伤害的结算（M34-03）：**只扣血**，不给硬直、不给无敌帧、不给击退——
-- 中毒不是"被打了一下"，人要能继续走动，只是气血在往下走。
-- 致死路径仍然交给 fall_player（要点 4：DOT 可以致死但不绕过死亡流程）。
local function settle_status_damage(event)
  if player_dead or event.effect ~= "damage" then
    return
  end
  -- 状态伤害（中毒这类 DOT）**不走命中公式**：没有防御平减，也没有格挡比例。
  -- 两层减免各自按自己的锚点结算——DOT 每 N 步一跳、减免只在 hit 发生时生效，
  -- 让 DOT 再吃一次防御等于同一层减免跨两套计时重复扣（M34-03 文件头那条分工的延续）。
  local amount = math.max(0, math.floor(tonumber(event.amount) or 0))
  if amount <= 0 then
    return
  end
  player_hp = math.max(0, player_hp - amount)
  mirror_health(player.ensure(), player_hp)
  push_log({ event = "dot", attacker = event.source, target = "player",
    skill = event.kind, damage = amount, remaining = player_hp })
  log.info(string.format(
    "[combat] dot source=%s kind=%s damage=%d remaining=%d frame=%d",
    tostring(event.source), tostring(event.kind), amount, player_hp, frame))
  if player_hp <= 0 then
    fall_player(event.source, event.kind, amount)
  end
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
    -- 伤害公式（docs/33 §6.1）：**平减在前、比例在后**，两条通路各扣一次是双倍减伤。
    -- ① 防御平减：`max(1, 攻击力+招式伤害-防御力)` 的兜 1 点只给**有伤害的**命中，
    --    所以外面再 `min(raw, …)` 夹一次——0 伤害的试探（训练木人/摆姿势）不会被
    --    防御公式**加成**成 1 点。格挡那一层用同一个形状，理由相同。
    -- ② 格挡减伤（M34-04 / 守心诀）：只有**正面**命中才乘 `(1 - absorb)`，背后挨打
    --    一分不减；比例减完向下取整。盾不是无敌，站着一动不动挨 12 刀仍然会倒。
    local guard = active_guard()
    local guarded = guard ~= nil and is_front_hit(atk, target_id, guard.front_degrees)
    local raw = math.max(0, tonumber(atk.damage) or 0)
    local defense = defense_of(PLAYER_ACTOR)
    local after_flat = math.min(raw, math.max(1, raw - defense))
    local flat_reduced = raw - after_flat
    local damage = after_flat
    if guarded then
      damage = math.min(after_flat, math.max(1, math.floor(after_flat * (1 - guard.absorb))))
    end
    -- `absorbed` 只记**盾**挡下的那部分（比例层），防御平减另存 `defense` 字段：
    -- 两者混记会让"格挡挡了多少"这种读数里混进玩家天生那 1 点防御。
    local absorbed = math.max(0, after_flat - damage)
    player_hp = math.max(0, player_hp - damage)
    stats.taken = stats.taken + damage
    stats.hits = stats.hits + 1
    if guarded then
      stats.guard_hits = stats.guard_hits + 1
      stats.guard_absorbed = stats.guard_absorbed + absorbed
    end
    mirror_health(pid, player_hp)
    audio.sfx("sfx_hit")
    -- 命中挂状态（M34-03）：无敌帧的分支在上面已经 return，所以"整次命中被吸收"
    -- 连状态一起吃掉——这是有意的（挡了一下就该整下都挡住）。
    local status_kind = attacker_status_kind(atk)
    if status_kind then
      status.apply(status_kind, PLAYER_ACTOR, atk.attacker_name)
    end
    -- docs/48 effects：玩家受击闪白 + 命中火花。
    local ok, pos = pcall(transform.get, pid)
    if ok and type(pos) == "table" then
      effects.spawn("hurt_flash", pos.x, pos.y)
      effects.spawn("hit_spark", pos.x, pos.y)
    end
    if player_hp <= 0 then
      fall_player(atk.attacker_name, atk.skill, damage)
    else
      -- 受击反馈三件套：硬直（打断操作）+ 无敌帧（防连击致死）+ 击退（真实位移）。
      -- 硬直/击退**不吃减伤**（docs §6.1 那一格只说"减伤"，没说免打断；盾扛得住伤害，
      -- 扛不住推力——这一条口径由 jhyt_m34_skills 的用例钉住）。
      player_hurt = math.max(player_hurt, math.floor(tonumber(atk.hitstun) or 0))
      player_iframes = combat_data.player_iframes()
      local pushed = push_player(dir_x, dir_y, tonumber(atk.knockback) or 0)
      push_log({ event = "hit", attacker = atk.attacker_name, target = "player",
        skill = atk.skill, damage = damage, remaining = player_hp,
        knockback = pushed, hitstun = player_hurt, status = status_kind,
        absorbed = absorbed, defense = flat_reduced })
      log.info(string.format(
        "[combat] hit attacker=%s target=player skill=%s damage=%d remaining=%d knockback=%d hitstun=%d iframes=%d status=%s absorbed=%d defense=%d frame=%d",
        tostring(atk.attacker_name), tostring(atk.skill), damage, player_hp,
        pushed, player_hurt, player_iframes, tostring(status_kind), absorbed, flat_reduced, frame))
      -- 主角形象：受击（非循环；faint 已在上面的致死分支播过）
      player.play("hurt")
    end
    return true
  end
  if enemy_kind and enemy_kind.is_enemy(target_id) then
    -- 名字必须在结算前取：死亡路径会销毁实体并清空 id → is_enemy 失配。
    local target_display = enemy_kind.name_of(target_id) or tostring(target_id)
    -- docs/48 effects：命中火花（目标位置，hit 结算前取——死亡路径会销毁实体）。
    local ok, pos = pcall(transform.get, target_id)
    if ok and type(pos) == "table" then
      effects.spawn("hit_spark", pos.x, pos.y)
    end
    -- 敌方侧不做防御平减：`data/enemies.lua` 没有 defense 字段（docs/33 §6.2 的防御列
    -- 是 Alpha 目标值）。玩家的基础 attack 已经在 `player/skills.lua` 的招式数值里，
    -- 这一路今天吃满 atk.damage——jhyt_m34_skills ④ 有对照断言钉住。
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

-- 冷却键：攻击者稳定身份 + 招式（玩家 "player"，敌人 persist_id）。
local function cooldown_key(attacker_key, skill_id)
  return tostring(attacker_key) .. ":" .. tostring(skill_id)
end

-- 施放前门禁（M34-04 从 M.attack 里抽出来）：普攻/武学、轻功共用同一套"能不能出手"的判断，
-- **reason 字符串就是 HUD 的 last_action 键**，抽取前后逐条一致（用例把 dead/mode/stunned/
-- player_dead/cooldown 钉死，改这个函数不能顺手改文案键）。
local function cast_gate(attacker_kind, attacker_key, skill_id)
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
  if (cooldowns[cooldown_key(attacker_key, skill_id)] or 0) > frame then
    return false, "cooldown"
  end
  return true, "ok"
end

-- 内力结算（docs/33 §6.1 的"消耗内力"栏）：够就扣，不够返回 false。
-- 与门禁分开写，是因为**轻功要"先确定付得起、再走位、最后才扣"**：反过来的话
-- 一次按错的轻功会把人瞬移走还白扣内力。
local function pay_inner(cost)
  local amount = math.max(0, tonumber(cost) or 0)
  if amount <= 0 then
    return true
  end
  local inner = tonumber(state.inner_power) or 0
  if inner < amount then
    return false
  end
  state.inner_power = inner - amount
  return true
end

-- 本步的格挡姿态结算（M34-04 / 守心诀）：**按下 + 能行动 + 付得起内力**才站得住，
-- 三者任一不满足就落防。内力按期扣（每 inner_period 步扣一次 inner_cost），那正是
-- docs/33 §6.1「10/秒」的字面实现：60 步 = 1 秒 ↔ 每秒 10 点，而不是"每秒检查一次"。
-- 起手当步就扣第一笔（按住的那一刻开始消耗），之后每 inner_period 步一笔。
local function settle_guard()
  local skill = guard_skill_id and skills.get(guard_skill_id)
  local can_hold = guard_wanted
    and skill ~= nil
    and skill.kind == "guard"
    and not player_dead
    and state.is_mode("Playing")
    and player_hurt <= 0
  local cost = math.max(0, tonumber(skill and skill.inner_cost) or 0)
  local period = math.max(1, math.floor(tonumber(skill and skill.inner_period) or 1))
  if not can_hold then
    if guard_active then
      push_log({ event = "guard_down", attacker = "player", target = nil,
        skill = guard_skill_id, damage = 0 })
      log.info("[combat] guard down reason=gate frame=%d", frame)
    end
    guard_active = false
    guard_charge = 0
    return
  end
  if not guard_active then
    if not pay_inner(cost) then
      -- 内力不够 → 自动落防：不报错、不进冷却（格挡没有冷却），松一下再按就能重新起。
      log.info("[combat] guard refused inner=%s cost=%d frame=%d",
        tostring(state.inner_power), cost, frame)
      return
    end
    guard_active = true
    guard_charge = period
    push_log({ event = "guard", attacker = "player", target = nil,
      skill = guard_skill_id, damage = 0 })
    log.info(string.format("[combat] guard up skill=%s absorb=%s arc=%s charge=%d frame=%d",
      tostring(guard_skill_id), tostring(skill.absorb), tostring(skill.front_degrees),
      guard_charge, frame))
    return
  end
  guard_charge = guard_charge - 1
  if guard_charge > 0 then
    return
  end
  if not pay_inner(cost) then
    push_log({ event = "guard_down", attacker = "player", target = nil,
      skill = guard_skill_id, damage = 0 })
    log.info("[combat] guard down reason=inner frame=%d", frame)
    guard_active = false
    guard_charge = 0
    return
  end
  guard_charge = period
end

-- 固定步推进：帧号 +1 → 结算格挡姿态 → 结算判定窗内的攻击 → 清理过期窗口。
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
  -- 格挡姿态先结算再判命中：**这一步站住的盾，挡得住这一步打来的刀**
  -- （按键本身在上一帧的 input.update 里报上来，与攻击前摇同样晚一步生效）。
  settle_guard()
  -- 瞬时状态与受击反馈同一口径：**本步挂上的状态从下一步才开始走**，
  -- 所以"第 N 步挂毒、第 N+tick_steps 步第一次掉血"是可精确断言的（M34-03）。
  for _, event in ipairs(status.tick(PLAYER_ACTOR)) do
    settle_status_damage(event)
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
  -- 底座只服务"打出去一击"的招式：判定窗字段（windup/active/range/width）全从这一条路径读。
  -- 轻功/格挡各有自己的入口（M.player_dash / M.player_guard），不走这里。
  -- 玩家侧的 input.lua 按 kind 分派，因此这个 reason 永远不会成为 HUD 上的一句英文。
  if not skills.uses_hitbox(skill) then
    return false, "not_hitbox"
  end
  local ok, reason = cast_gate(attacker_kind, attacker_key, skill_id)
  if not ok then
    return false, reason
  end
  if not pay_inner(skills.inner_cost(skill_id)) then
    return false, "inner_power"
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
  cooldowns[cooldown_key(attacker_key, skill_id)] = frame + math.max(0, math.floor(tonumber(skill.cooldown) or 0))
  stats.attacks = stats.attacks + 1
  push_log({ event = "attack", attacker = attacker_name, target = nil,
    skill = skill_id, damage = math.max(0, tonumber(damage) or 0) })
  -- 主角形象：普攻/武学起手就播 attack_<dir>（与判定窗口同帧起算，观感对齐）
  if attacker_kind == "player" then
    player.play("attack")
    -- docs/48 effects：刀光落在 hitbox 中心（攻击者前方 range/2），千风掌带身法残影。
    local ok, pos = pcall(transform.get, attacker_id)
    if ok and type(pos) == "table" then
      if skill_id == "palm_gust" then
        effects.spawn("dash_streak", pos.x, pos.y)
      end
      local dir = FACING_VECTORS[facing] or FACING_VECTORS.down
      local dist = (tonumber(skill.range) or 0) / 2
      effects.spawn("blade_arc", pos.x + dir.x * dist, pos.y + dir.y * dist)
    end
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

-- 轻功（M34-04 / 追风步）入口（input.lua 的 dash 动作）：沿当前朝向位移 dash_tiles 格。
-- 门禁/内力/冷却复用 M.attack 抽出来的那三件套；与普攻的区别只有两处：**不出判定窗**
-- （没有攻击记录进 attacks），以及被墙挡住时按实际可走格数结算。
-- 收费顺序写死在这里：门禁 → 确认内力付得起（只查不扣）→ 走位 → 真扣 → 进冷却。
-- 身前一个落脚点都没有时什么都不发生：不扣内力、不进冷却、返回 dash_blocked。
function M.player_dash(skill_id)
  local skill = skills.get(skill_id)
  if not skill or skill.kind ~= "movement" then
    return false, "unknown_skill"
  end
  local pid = player.ensure()
  if not pid then
    return false, "no_player"
  end
  local ok, reason = cast_gate("player", "player", skill_id)
  if not ok then
    return false, reason
  end
  local cost = math.max(0, tonumber(skill.inner_cost) or 0)
  if cost > (tonumber(state.inner_power) or 0) then
    return false, "inner_power"
  end
  local tiles = math.max(1, math.floor(tonumber(skill.dash_tiles) or 1))
  local dir = facing_vector(state.facing)
  local moved, distance = dash_player(dir.x, dir.y, tiles)
  if moved == 0 then
    return false, "dash_blocked"
  end
  pay_inner(cost)
  cooldowns[cooldown_key("player", skill_id)] = frame
    + math.max(0, math.floor(tonumber(skill.cooldown) or 0))
  -- 无敌帧走受击反馈**同一个计数器**（docs/33 §10「轻功无敌 | 不受伤害和硬直」）：
  -- 命中结算里 `player_iframes > 0` 那条分支把伤害/硬直/击退一起吸收，正是那句话。
  local steps = math.max(0, math.floor(tonumber(skill.invincible_steps) or 0))
  player_iframes = math.max(player_iframes, steps)
  stats.dashes = stats.dashes + 1
  last_dash = { skill = skill_id, tiles = moved, distance = distance, iframes = player_iframes }
  push_log({ event = "dash", attacker = "player", target = nil, skill = skill_id, damage = 0 })
  log.info(string.format(
    "[combat] dash skill=%s tiles=%d distance=%d iframes=%d inner=%s frame=%d",
    tostring(skill_id), moved, distance, player_iframes, tostring(state.inner_power), frame))
  -- 形象：主角条带里没有 dash 动作（player.play 会因条带缺失返回 false），
  -- 于是用 docs/48 已有的身法残影，落在位移**之后**的位置。
  local ok_pos, pos = pcall(transform.get, pid)
  if ok_pos and type(pos) == "table" then
    effects.spawn("dash_streak", pos.x, pos.y)
  end
  return true, "ok"
end

-- 格挡（M34-04 / 守心诀）入口（input.lua 的 guard 动作**长按**）：本函数只把
-- "按没按"记进账，实际生效由 combat.tick 里的 settle_guard 结算——所以按下的当步还不减伤，
-- 与攻击前摇同一口径（操作晚一个固定步才落到世界里）。招式不是 kind=guard 就直接落防：
-- 报错留给调用方，姿态这边绝不"顺手当成格挡"。
function M.player_guard(wanted, skill_id)
  local skill = skills.get(skill_id)
  if not skill or skill.kind ~= "guard" then
    guard_wanted = false
    return false, "unknown_skill"
  end
  guard_skill_id = skill_id
  guard_wanted = wanted == true
  return true, "ok"
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

-- 最近一次轻功的结算（tiles = 实际走的格数，永远 <= 数据里的 dash_tiles）。
function M.last_dash()
  if not last_dash then
    return nil
  end
  return {
    skill = last_dash.skill,
    tiles = last_dash.tiles,
    distance = last_dash.distance,
    iframes = last_dash.iframes
  }
end

-- 当前是否站着格挡姿态（HUD 与 headless 断言读同一个判据，不再有第二份"盾"的状态）。
function M.is_guarding()
  return guard_active
end

-- 离下一次扣内力还剩几步（0 = 这一步就扣）。
function M.guard_charge()
  return guard_charge
end

-- headless/调试：直接设置玩家气血（0 触发战败流程，便于确定性验收）。
function M.set_player_hp(value)
  local hp = math.max(0, tonumber(value) or 0)
  player_hp = math.min(PLAYER_MAX_HP, hp)
  mirror_health(player.ensure(), player_hp)
  if player_hp <= 0 and not player_dead then
    -- 与被打死同一条收口（M34-03）：连"清掉身上还在跳的状态"一起拿到，不留第二条死亡实现。
    fall_player("debug", "-", 0)
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
  -- 复活把瞬时状态一起归零：毒不该跟着人回庄（§6.3 的口径是"回庄后生命/体力/内力回满"）。
  status.clear(PLAYER_ACTOR)
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
      hitstun = entry.hitstun,
      -- M34-03：这一击挂上的状态种类（没有则 nil）——"带中毒"从此是可回读的事实。
      -- DOT 自己记的条目用 event = "dot"，种类落在 skill 字段上（同一张表，不另立形状）。
      status = entry.status,
      -- M34-04：这一击被格挡吃掉的那部分伤害（打在玩家身上但未格挡为 0，打向敌人的条目为 nil）。
      -- M34-04b：防御平减掉的点数另存 `defense`（比例层与平减层分开记，混在一起就没法回答
      -- "盾挡了多少"）。于是打在玩家身上的条目满足 `damage + absorbed + defense = 原始威力`；
      -- 日志里只留一个数就核不了这两层各扣了多少。
      absorbed = entry.absorbed,
      defense = entry.defense }
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
    hits_taken = stats.hits,
    -- 轻功与格挡（M34-04）：dashes 不计入 attacks（追风步一下都没打中任何人，
    -- 算进"攻击次数"会让 m28 的每刀读数失真）；后两个只统计**正面**被减伤的命中。
    dashes = stats.dashes,
    guard_hits = stats.guard_hits,
    guard_absorbed = stats.guard_absorbed
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
  last_dash = nil
  guard_wanted = false
  guard_active = false
  guard_charge = 0
  status.reset()
  attacks = {}
  cooldowns = {}
  log_entries = {}
  stats = { attacks = 0, dealt = 0, taken = 0, kills = 0, deaths = 0, ignored = 0,
    hits = 0, dashes = 0, guard_hits = 0, guard_absorbed = 0 }
  mirror_health(player.ensure(), player_hp)
end

-- 存档：只记玩家气血（敌人账本在 enemy provider；攻击窗/冷却/硬直/无敌帧/击退/中毒
-- 都是**瞬时状态**，不进档——读档后玩家站在安全位置，不该继承上一局的僵直或毒性）。
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
  last_dash = nil
  -- 读档清状态（M34-03）：中毒与硬直/无敌帧同类——**瞬时状态不入档**（docs/33 既有裁定），
  -- 读档回来站在安全位置，不该继承上一局的毒素。血线（上面的 hp）才是入档的那一份。
  -- 格挡（M34-04）同一类：姿态由"此刻按没按键"决定，继承上一局的盾等于凭空多一层血。
  guard_wanted = false
  guard_active = false
  guard_charge = 0
  status.clear(PLAYER_ACTOR)
  mirror_health(player.ensure(), player_hp)
end

return M
