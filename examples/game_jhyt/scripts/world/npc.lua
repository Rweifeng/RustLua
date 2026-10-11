-- npc.lua：《江湖有田》NPC 日程执行与路径跟随（M17-02）。
--
-- 引擎事实（core/script.rs + core/tilemap.rs）：
--   * entity.spawn(name, persist_id) 只插入 Transform —— Lua 侧没有创建 2D 刚体
--     的 API，所以 NPC **不能用 physics.set_velocity/set_position**（会报
--     UnknownBody），必须用 transform.get/set 逐帧推进。
--   * persist_id 是稳定身份：切图/场景重建后，引擎不会自动带走 Lua 侧
--     entity.spawn 的实体，也不清 persist 表（scene transition 会清）。因此
--     跨图 NPC 在目标图 on_enter 时按 PersistId 重建，靠 find_id 去重。
--   * tilemap.find_path(id, fx, fy, tx, ty, diagonal?) 返回 1-based 的
--     {x=,y=} 格序列（含起终点），或 nil, reason（reason 为字符串，如
--     "path endpoint (x, y) is solid" / "no path from ... to ..."）。
--   * grid.world_to_cell(x, y, TILE) 返回 1-based {cx, cy}；格心 =
--     (cx + 0.5) * TILE。
--
-- 模块职责：
--   * on_enter(map)：绑定本图 GroundMap，逻辑目标在本图的 NPC 落位/重建。
--   * tick()：按 clock 分钟 + 天气解析日程 → 目标地点 → A* 路径 → 逐帧移动；
--     目标换图时销毁本图实体（逻辑上"去了别的地图"，等玩家到那张图再重建）。
--   * 诊断：地点不可达/未知地点/时段缺失时输出 NPC、时间段、地图、目标点到
--     log.warn，并记入可查询的 diagnostics 列表（验收项③）。
--   * capture/restore：由 main.lua 注册为存档 provider，保证读档后位置/活动一致。
local places = require("data.places")
local npcs = require("data.npcs")
local characters = require("data.characters")
local depth = require("world.depth")
local schedule = require("world.schedule")
local weather = require("world.weather")

local M = {}

local TILE = 32
-- 路径失败后的重算节流（帧数），避免每帧重复 A* 与刷屏诊断。
local REPATH_TICKS = 30

-- 同地多人的落位分配。places.lua 一个地点只有一个 cell，而日程允许两人在同一时段引用
-- 同一地点（00:00 顾长风与玄鹤同睡 manor_home、19:00 云泽村茶摊挤 6 人）。
-- 为什么不是"格内像素抖动"：角色贴图 64×64，可见身体 alpha 包围盒 12 张里 11 张是
-- x∈[20,51]（宽正好 32 = **整整一格**），最宽的 han_tieshan 到 x∈[18,51]（34）⇒ 一格只有
-- 32 宽、格心到格边只剩 ±16 ⇒ 格内怎么抖都抖不开（旧表最大间距 12，两人还是叠在一起）。
-- 所以改成每人分配一个**真实格位**：从锚点格起
-- 按环形外扩，跳过实心格与已被本图其他 NPC 占用的格 ⇒ 同一地点的人围成一圈，全图也不会有
-- 两个人站在同一格。落点是真格子而不是 transform 偏移 ⇒ world_to_cell 读回、find_path
-- 端点、存档 cell、深度排序走的都是普通落位路径。
-- 环序：由近及远（中心 → 四邻 → 斜角 → 两格外的八格 → 四角），保证一群人留在锚点周围
-- 而不是散到隔壁地点去。半径 2 = 25 个候选格：全量日程 ×1440 分钟 ×晴/雨实测最挤的
-- 同地组是茶摊 (32,8) 19:00 的 6 人，该锚点 R=2 内筛掉实心后仍剩 24 个空格，够分。
local STAND_RADIUS = 2
local STAND_RING = {}
for dy = -STAND_RADIUS, STAND_RADIUS do
  for dx = -STAND_RADIUS, STAND_RADIUS do
    STAND_RING[#STAND_RING + 1] = { x = dx, y = dy, d = dx * dx + dy * dy }
  end
end
-- 距离平方升序；同距按 y、x 升序 —— 纯函数排序，跨帧/跨宿主结论一致。
table.sort(STAND_RING, function(a, b)
  if a.d ~= b.d then
    return a.d < b.d
  end
  if a.y ~= b.y then
    return a.y < b.y
  end
  return a.x < b.x
end)

local ground_id = nil -- 当前图 GroundMap 的 Tilemap 实体
local width, height = 0, 0
local actors = {} -- persist_id → 运行时记录
local diagnostics = {} -- 已输出的诊断（字符串，稳定顺序 = 首次发生顺序）
local diag_keys = {} -- 诊断去重键
local sorted_npcs = nil

local function npc_list()
  if not sorted_npcs then
    sorted_npcs = npcs.list()
  end
  return sorted_npcs
end

local function current_map()
  return map.current() or scene.reload()
end

local function clock_minute()
  local now = clock.now()
  return tonumber(now.minute_of_day) or 0, tonumber(now.day) or 1
end

local function is_rainy()
  return weather.is_rainy() == true
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

local function same_cell(a, b)
  return a ~= nil and b ~= nil and a.x == b.x and a.y == b.y
end

local function cell_key(cell)
  return tostring(cell.x) .. "," .. tostring(cell.y)
end

-- 候选格可用 = 在图内且非实心。实心格当 find_path 端点会直接失败，
-- 所以宁可少分一格也不能把 NPC 派进墙里。
local function cell_open(cell)
  if not grid.in_bounds(cell.x, cell.y, width, height) then
    return false
  end
  local ok, solid = pcall(tilemap.is_solid, ground_id, cell.x, cell.y)
  return ok and solid == false
end

-- 已有的落脚点是否还作数：必须是当前锚点半径 STAND_RADIUS 内的某一格（挡读档/换图后
-- 残留的远格）、在图内、非实心。归属冲突由 assign_stand_cells 的 taken 表另外判。
local function cell_on_ring(anchor, cell)
  return math.abs(cell.x - anchor.x) <= STAND_RADIUS
    and math.abs(cell.y - anchor.y) <= STAND_RADIUS
end

local function stand_cell_usable(actor, cell)
  return cell ~= nil and actor.place_cell ~= nil
    and cell_on_ring(actor.place_cell, cell) and cell_open(cell)
end

local function window_text(block)
  if type(block) ~= "table" then
    return "?"
  end
  return schedule.format_minute(block.from) .. "-" .. schedule.format_minute(block.to)
end

local function report(actor, key, message)
  if diag_keys[key] then
    return
  end
  diag_keys[key] = true
  diagnostics[#diagnostics + 1] = message
  log.warn("[npc] " .. message)
end

local function record_for(npc)
  local actor = actors[npc.persist_id]
  if not actor then
    actor = { npc = npc }
    actors[npc.persist_id] = actor
  end
  return actor
end

-- 解析当前时间/天气下的目标：{place, def, activity, block} 或 {reason}。
local function resolve_target(actor)
  local minute = clock_minute()
  local resolved, reason = schedule.resolve(actor.npc.schedule, minute, is_rainy())
  if not resolved then
    return { reason = tostring(reason) }
  end
  local def = places.get(resolved.place)
  if not def then
    return {
      activity = resolved.activity,
      block = resolved.block,
      reason = "unknown place: " .. tostring(resolved.place)
    }
  end
  return {
    place = resolved.place,
    def = def,
    activity = resolved.activity,
    block = resolved.block
  }
end

-- 统一诊断前缀：NPC、时间段、地图、目标点、活动（验收项③要求的字段）。
local function target_line(actor, place, def, activity, block)
  local cell = def and def.cell or {}
  return "npc=" .. tostring(actor.npc.name)
    .. " persist_id=" .. tostring(actor.npc.persist_id)
    .. " window=" .. window_text(block)
    .. " map=" .. tostring(def and def.map or "?")
    .. " place=" .. tostring(place)
    .. " cell=" .. tostring(cell.x) .. "," .. tostring(cell.y)
    .. " activity=" .. tostring(activity)
end

-- 朝向：世界 y 朝上，所以 +y 是"北/背对"，-y 是"南/面向镜头"。
-- 位移方向 → 八向条带朝向（s/n/e/w/se/sw/ne/nw）。
-- 世界坐标 y 向上：dy>0 向北（屏幕上方）。两轴同时显著非零时取对角，
-- 否则按主轴取四正；阈值 0.001 过滤路径寻路的浮点噪声，避免乱切对角。
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

-- 挂美术：优先用 data/characters.lua 的序列帧（static_sprite + animation），
-- 失败则退回 data/npcs.lua 的灰盒 sprite 串。attached 标记决定要不要切状态。
-- 形象一律经 depth.attach 挂上（= sprite.set + 按脚位 Y 写 order），否则 NPC 之间、
-- NPC 与主角之间的遮挡只会落在"实体 id 先后"上（web 上就是主角被 NPC 盖住）。
local function apply_art(actor, id)
  local npc = actor.npc
  local entry = npc.character and characters.get(npc.character) or nil
  if not entry then
    if npc.sprite then
      local ok, err = pcall(depth.attach, id, npc.sprite)
      if not ok then
        report(actor, "sprite:" .. npc.persist_id,
          "npc=" .. tostring(npc.name) .. " sprite failed: " .. tostring(err))
      end
    end
    return
  end
  local ok, err = pcall(depth.attach, id, entry.static_sprite.s)
  if not ok then
    report(actor, "sprite:" .. npc.persist_id,
      "npc=" .. tostring(npc.name) .. " sprite failed: " .. tostring(err))
    return
  end
  local attached = pcall(animation.attach, id, entry.animation)
  if attached then
    actor.animated = true
    actor.facing = actor.facing or "s"
    pcall(animation.set, id, "idle_" .. actor.facing)
  else
    report(actor, "anim:" .. npc.persist_id,
      "npc=" .. tostring(npc.name) .. " animation.attach 不可用（引擎需支持），退化单帧")
  end
end

-- 每帧按"说话 > 移动 > 待机 + 朝向"切 talk/walk/idle（引擎 animation.set 同状态时返回 false，开销可忽略）。
-- talk 由对话面板在开场/收尾时通过 M.set_talking 显式打开；优先级最高，
-- 让玩家在对话中看到的 NPC 有"正在说话"的手势，而不是僵立或继续踱步。
local function update_animation(actor, moving)
  if not actor.animated or not actor.id then
    return
  end
  local prefix
  if actor.talking then
    prefix = "talk_"
  elseif moving then
    prefix = "walk_"
  else
    prefix = "idle_"
  end
  pcall(animation.set, actor.id, prefix .. (actor.facing or "s"))
end

-- 对话面板钩子：切 talk_<dir> 手势。缺失 talk_ 条带时静默回落到 idle（不影响功能）。
function M.set_talking(persist_id, talking)
  local actor = actors[persist_id]
  if not actor then
    return false
  end
  actor.talking = talking == true
  if actor.id and entity.is_alive(actor.id) then
    update_animation(actor, false)
  end
  return true
end

-- 确保本图存在该 NPC 的实体并落位（pending_pos 优先于逻辑格心）。
local function activate(actor)
  if actor.id and entity.is_alive(actor.id) then
    return actor.id
  end
  local existing = entity.find_id(actor.npc.persist_id)
  if existing then
    actor.id = existing
  else
    local ok, id = pcall(entity.spawn, actor.npc.name, actor.npc.persist_id)
    if not ok or not id then
      report(actor, "spawn:" .. actor.npc.persist_id,
        "npc=" .. tostring(actor.npc.name) .. " spawn failed: " .. tostring(id))
      return nil
    end
    actor.id = id
    apply_art(actor, id)
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

-- 沿路径推进：waypoint 为格心；用 dt 秒 × speed 的移动预算跨多个 waypoint。
local function step_path(actor, dt)
  if not actor.path then
    update_animation(actor, false)
    return
  end
  local budget = (tonumber(actor.npc.speed) or 40) * dt
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

local function tick_actor(actor, here, dt, day)
  local target = resolve_target(actor)
  local def = target.def
  local place = target.place
  if not def then
    -- 日程缺失/地点未知：退回 home 并输出一次诊断。
    report(actor, "target:" .. actor.npc.persist_id .. ":" .. tostring(target.reason) .. ":" .. tostring(day),
      target_line(actor, target.reason, nil, target.activity, target.block))
    def = places.get(actor.npc.home)
    place = actor.npc.home
    if not def then
      return
    end
  end

  actor.place = place
  actor.activity = target.activity or actor.activity or "idle"
  actor.block = target.block or actor.block

  -- 目标在别的图：逻辑上离开本图（销毁本图实体），等玩家到那张图再重建。
  if def.map ~= here then
    if actor.map ~= def.map then
      -- 换图后存档坐标已失效，避免在下一次 activate 用旧 pending_pos 落位。
      actor.pending_pos = nil
    end
    actor.map = def.map
    actor.cell = { x = def.cell.x, y = def.cell.y }
    despawn(actor)
    return
  end

  actor.map = here
  if not activate(actor) then
    return
  end

  -- 日程换地点 → 作废落脚点，回到锚点格重新排队（stand_cell 由 assign_stand_cells 重排）。
  -- 注意比较的是 place_cell（日程锚点）而不是 target_cell（当前走去的格子）：
  -- 后者会被落位分配改成锚点周围的外扩格，拿它比日程就会每帧自我推翻、来回踱步。
  if not actor.place_cell
    or actor.place_cell.x ~= def.cell.x
    or actor.place_cell.y ~= def.cell.y then
    actor.place_cell = { x = def.cell.x, y = def.cell.y }
    actor.stand_cell = nil
    actor.target_cell = { x = def.cell.x, y = def.cell.y }
    actor.path = nil
    actor.path_index = nil
    actor.repath_wait = 0
  elseif not actor.target_cell then
    actor.target_cell = { x = actor.place_cell.x, y = actor.place_cell.y }
  end

  if actor.repath_wait and actor.repath_wait > 0 then
    actor.repath_wait = actor.repath_wait - 1
  end

  local current = cell_of(actor.id)
  if not current then
    return
  end
  actor.cell = current

  local arrived = actor.path == nil
    and current.x == actor.target_cell.x
    and current.y == actor.target_cell.y
  if actor.path == nil and not arrived and (actor.repath_wait or 0) <= 0 then
    actor.repath_wait = REPATH_TICKS
    local path, reason = tilemap.find_path(
      ground_id, current.x, current.y, actor.target_cell.x, actor.target_cell.y, false)
    if not path then
      local key = "path:" .. actor.npc.persist_id .. ":" .. tostring(day)
        .. ":" .. tostring(place) .. ":" .. tostring(actor.target_cell.x) .. ","
        .. tostring(actor.target_cell.y)
        .. ":" .. tostring(reason)
      report(actor, key,
        target_line(actor, place, def, actor.activity, actor.block) .. " reason=" .. tostring(reason))
    else
      actor.path = path
      actor.path_index = 2
      if #path <= 1 then
        actor.path = nil
        actor.path_index = nil
      end
    end
  end

  step_path(actor, dt)
end

-- 给本图每个已激活的 NPC 分配落脚格。三趟：
--   ① 保住已有的 stand_cell（按 persist_id 升序先到先得）——否则有人到场/离场会让
--      整群人重新排队、集体绕场一圈；
--   ② 没落脚的按 STAND_RING 从自己的锚点外扩，取第一格"没被占且非实心"的格子；
--   ③ 把落脚格写成要去的那格（target_cell），NPC 于是**走过去站定**（一格约 0.8 秒），
--      不是被推离原位。③ 必须对①保住的人同样做：despawn/on_enter 只清 target_cell、
--      不清 stand_cell，回到本图时若只改派新来的人，"保住落脚点"的那个就会一直站在
--      锚点上和锚点的第一人重叠（茶摊 32,8 的阿七与玄鹤）。
local function assign_stand_cells(here)
  local taken = {}
  local pending = {}
  local standing = {}
  for _, npc in ipairs(npc_list()) do
    local actor = actors[npc.persist_id]
    if actor and actor.id and actor.map == here and actor.place_cell then
      local cell = actor.stand_cell
      local key = cell and cell_key(cell) or nil
      if key and not taken[key] and stand_cell_usable(actor, cell) then
        taken[key] = true
        standing[#standing + 1] = actor
      else
        actor.stand_cell = nil
        pending[#pending + 1] = actor
      end
    end
  end
  for _, actor in ipairs(pending) do
    for _, off in ipairs(STAND_RING) do
      local cell = { x = actor.place_cell.x + off.x, y = actor.place_cell.y + off.y }
      local key = cell_key(cell)
      if not taken[key] and cell_open(cell) then
        actor.stand_cell = cell
        taken[key] = true
        standing[#standing + 1] = actor
        break
      end
    end
    if not actor.stand_cell then
      -- 分不到格子是要**看得见**的失败：画面上的症状只是"两个人叠在一起"，
      -- 没人会为此去翻日程表，所以把它报成一条诊断（同一地点只报一次）。
      report(actor, "spread:" .. tostring(here) .. ":" .. cell_key(actor.place_cell),
        target_line(actor, actor.place, { map = here, cell = actor.place_cell },
          actor.activity, actor.block) .. " reason=落脚格全被占或实心，退回锚点格")
      standing[#standing + 1] = actor
    end
  end
  for _, actor in ipairs(standing) do
    local spot = actor.stand_cell or actor.place_cell
    if not same_cell(actor.target_cell, spot) then
      actor.target_cell = { x = spot.x, y = spot.y }
      actor.path = nil
      actor.path_index = nil
      actor.repath_wait = 0
    end
  end
end

-- 同步：逻辑目标在本图的 NPC 落位；不在本图的确保无实体。
function M.sync()
  local here = current_map()
  for _, npc in ipairs(npc_list()) do
    local actor = record_for(npc)
    if not actor.map then
      local target = resolve_target(actor)
      local def = target.def or places.get(npc.home)
      if def then
        actor.map = def.map
        actor.place = target.place or npc.home
        actor.activity = target.activity or "idle"
        actor.cell = { x = def.cell.x, y = def.cell.y }
        actor.block = target.block
      end
    end
    if actor.map == here and ground_id then
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
    log.warn("[npc] 未找到 GroundMap（" .. tostring(map_id) .. "），NPC 不可用")
    ground_id = nil
  end
  -- 场景重建后实体 id 失效：先清空再按需重建（不能 destroy 旧 id）。
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
  local _, day = clock_minute()
  for _, npc in ipairs(npc_list()) do
    local actor = actors[npc.persist_id]
    if actor then
      tick_actor(actor, here, dt, day)
    end
  end
  assign_stand_cells(here)
end

-- 查询入口（headless 验收/调试）：单个 NPC 的逻辑状态。
-- 无实体时 id 存在与否如实反映（本图未激活 → id=nil）。
function M.query(persist_id)
  local npc = npcs.get(persist_id)
  if not npc then
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
    name = npc.name,
    map = actor.map,
    place = actor.place,
    activity = actor.activity,
    cell_x = cell and cell.x or nil,
    cell_y = cell and cell.y or nil,
    moving = actor.path ~= nil,
    -- 站定 = 没有路径**且**人已在自己要去的那格。assign_stand_cells 在 tick 末尾改派
    -- 落脚格时会顺手清 path，只看 moving 会把"刚被改派、人还在原格"当成站定。
    at_target = actor.path == nil and same_cell(cell, actor.target_cell),
    live = actor.id ~= nil and entity.is_alive(actor.id) == true
  }
end

-- 落位分配现场（headless 验收/调试）：本图每个已激活 NPC 一行
-- `persist_id 锚点 落脚格 目标格 当前格 重算等待 walk|idle`。判"为什么这俩人还在同一格"
-- 要看的就是这五列（落脚格不同却同格 = 走不过去；落脚格相同 = 分配器没生效；
-- 目标格 ≠ 落脚格 = 分配器没把它写进 target_cell）。
function M.standing_report()
  local here = current_map()
  local out = {}
  for _, npc in ipairs(npc_list()) do
    local actor = actors[npc.persist_id]
    if actor and actor.id and actor.map == here then
      out[#out + 1] = table.concat({
        npc.persist_id,
        actor.place_cell and cell_key(actor.place_cell) or '-',
        actor.stand_cell and cell_key(actor.stand_cell) or '-',
        actor.target_cell and cell_key(actor.target_cell) or '-',
        actor.cell and cell_key(actor.cell) or '-',
        tostring(actor.repath_wait or 0),
        actor.path and 'walk' or 'idle'
      }, ' ')
    end
  end
  return table.concat(out, ' | ')
end

-- 诊断列表（验收项③）：不可达/未知地点/时段缺失等去重后的文案，首次发生顺序。
function M.diagnostics()
  local out = {}
  for i, message in ipairs(diagnostics) do
    out[i] = message
  end
  return out
end

-- 存档：逐个 NPC 记逻辑地图、地点、活动与精确坐标（读档后一致，验收项④）。
function M.capture()
  local _, day = clock_minute()
  local snapshot = { day = day, npcs = {} }
  for _, npc in ipairs(npc_list()) do
    local actor = actors[npc.persist_id]
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
      snapshot.npcs[#snapshot.npcs + 1] = {
        persist_id = npc.persist_id,
        map = actor.map,
        place = actor.place,
        activity = actor.activity,
        cell = cell and { x = cell.x, y = cell.y } or nil,
        pos = pos
      }
    end
  end
  return snapshot
end

-- 读档：写回逻辑状态；本图实体立即落位到存档坐标，他图实体等 on_enter 重建。
function M.restore(data)
  if type(data) ~= "table" or type(data.npcs) ~= "table" then
    return
  end
  for _, entry in ipairs(data.npcs) do
    if type(entry) == "table" and type(entry.persist_id) == "string" then
      local npc = npcs.get(entry.persist_id)
      if npc then
        local actor = record_for(npc)
        actor.map = entry.map
        actor.place = entry.place
        actor.activity = entry.activity
        actor.block = nil
        actor.path = nil
        actor.path_index = nil
        actor.target_cell = nil
        actor.repath_wait = 0
        actor.pending_pos = nil
        if type(entry.cell) == "table" then
          actor.cell = { x = entry.cell.x, y = entry.cell.y }
        end
        -- 存档那一格就是它上一局的落脚格：连锚点一起写回，分配器会重验一遍（在环内、
        -- 非实心、没和别人撞格）后认下来 ⇒ 读档不会先走回锚点再重新散开。
        local anchor = places.get(entry.place)
        actor.place_cell = anchor and { x = anchor.cell.x, y = anchor.cell.y } or nil
        actor.stand_cell = type(entry.cell) == "table"
          and { x = entry.cell.x, y = entry.cell.y } or nil
        if type(entry.pos) == "table" then
          actor.pending_pos = { x = entry.pos.x, y = entry.pos.y }
        end
        -- 本图已有实体：activate 会早退，直接落位到存档坐标。
        if actor.id and entity.is_alive(actor.id) then
          if actor.pending_pos then
            transform.set(actor.id, actor.pending_pos.x, actor.pending_pos.y)
            actor.pending_pos = nil
            actor.cell = cell_of(actor.id) or actor.cell
          end
        end
      end
    end
  end
  if ground_id then
    M.sync()
  end
end

return M