-- building.lua：《江湖有田》建筑施工与占用（M18-02）。
--
-- 职责：
--   * 多格 footprint 建筑的占用校验（预览给逐格冲突原因）、放置、拆除、升级。
--   * 视觉：每个已放建筑一个运行期 Sprite 实体（entity.spawn + sprite.set）；
--     阻挡：footprint 各格在 GroundMap 上 set_solid。
--   * 持久化：map.state("buildings") = { version=1, seq=n, list={ 记录... } }，
--     随 `__mapstate:*` 进存档；切图/读档后由 M.on_enter 重放视觉与阻挡。
--
-- 占用规则（验收：不能覆盖道路、出口、已耕地、作物和既有建筑）：
--   * out_of_bounds  越界
--   * ground         地表 tile 不在建筑 ground_tiles 白名单
--   * road           田前石路等石板 tile==3（可走但禁止建造）
--   * solid          GroundMap 已阻挡（院墙/水/屋舍基底/道具/场景建筑）
--   * exit           当前地图出口触发格
--   * farm_plot      该格已有开垦/作物（farm.plots）
--   * building       与既有建筑 footprint 重叠
--
-- 实体纪律（吸取 farm.lua 陈旧 farm_id 的教训）：所有缓存实体 id 使用前
-- `entity.is_alive` 判活；restore 只写内存与 map.state，视觉统一走 on_enter。
local state = require("core.state")
local player = require("player.player")
local maps = require("data.maps")
local building_data = require("data.buildings")
-- M19-02：建造/升级成功音效（失败路径无声，由 HUD 提示反馈）。
local audio = require("core.audio")

local M = {}

local TILE = 32
local STATE_KEY = "buildings"
local STATE_VERSION = 1
local SORTING = "sorting_layer=world;order=1"

-- 运行期
local ground_id = nil -- GroundMap（判活后绑定；占用读 tile/solid，建造写 solid）
local width, height = 0, 0
local current_map = nil
local records = {} -- key("cx,cy" 锚点) → { key, cx, cy, building, level, entity }
local order = {} -- key 稳定顺序（map.state 重放顺序）
local seq = 0
local selected = nil
local build_mode = false
local job_guard = nil -- crafting 挂接：key → 是否有进行中制作任务

local function key_of(cx, cy)
  return tostring(cx) .. "," .. tostring(cy)
end

local function def_of(id)
  return building_data.get(id)
end

local function footprint_of(def, cx, cy)
  local cells = {}
  for i = 0, def.w - 1 do
    for j = 0, def.h - 1 do
      cells[#cells + 1] = { x = cx + i, y = cy + j }
    end
  end
  return cells
end

local function record_cells(rec)
  local def = def_of(rec.building)
  if not def then
    return {}
  end
  return footprint_of(def, rec.cx, rec.cy)
end

local function cell_owner(cx, cy)
  for _, key in ipairs(order) do
    local rec = records[key]
    if rec then
      for _, cell in ipairs(record_cells(rec)) do
        if cell.x == cx and cell.y == cy then
          return rec
        end
      end
    end
  end
  return nil
end

-- 当前地图出口触发格查表
local function exit_cells()
  local set = {}
  local map_id = current_map or map.current()
  local map_def = map_id and maps.get(map_id) or nil
  for _, exit in ipairs(map_def and map_def.exits or {}) do
    local cell = exit.cell
    if cell then
      set[key_of(cell.x, cell.y)] = exit.id
    end
  end
  return set
end

local function tile_at(cx, cy)
  if not ground_id or not entity.is_alive(ground_id) then
    return nil
  end
  return tilemap.get_cell(ground_id, cx, cy) or 0
end

local function is_solid(cx, cy)
  if not ground_id or not entity.is_alive(ground_id) then
    return false
  end
  return tilemap.is_solid(ground_id, cx, cy) == true
end

local function bounds_ok(cx, cy)
  if width == 0 or height == 0 then
    return false
  end
  return grid.in_bounds(cx, cy, width, height)
end

-- farm 模块惰性引入（避免与 farm.lua 的加载顺序耦合）；必须在占用判定之前
-- 声明为局部变量，否则闭包会绑到全局 nil。
local farm = nil
local function ensure_farm()
  if farm == nil then
    local ok, module = pcall(require, "systems.farm")
    farm = ok and module or false
  end
  return farm or nil
end

-- 单格冲突判定；返回 reason 或 nil。顺序固定 → 输出确定。
-- 优先级：越界 > 农田地块 > 既有建筑 > 地表 > 阻挡 > 出口 > 道路。
-- （农田建在夯土上，若不先判 farm_plot 会被 ground 掩盖，预览就说不清原因。）
local function cell_conflict(def, cx, cy, exits)
  if not bounds_ok(cx, cy) then
    return "out_of_bounds"
  end
  if farm and farm.is_plot and farm.is_plot(cx, cy) then
    return "farm_plot"
  end
  if cell_owner(cx, cy) then
    return "building"
  end
  local allowed = false
  for _, tile in ipairs(def.ground_tiles or {}) do
    if tile_at(cx, cy) == tile then
      allowed = true
      break
    end
  end
  if not allowed then
    return "ground"
  end
  if is_solid(cx, cy) then
    return "solid"
  end
  -- 出口先于道路：出口格通常铺石板，语义上「出口」是更准确的拒绝原因。
  if exits[key_of(cx, cy)] then
    return "exit"
  end
  if tile_at(cx, cy) == 3 then
    return "road"
  end
  return nil
end

-- 建筑放置校验：返回 ok, conflicts（{ {x,y,reason}... }，逐格、顺序确定）
function M.can_place(building_id, cx, cy)
  local def = def_of(building_id)
  if not def then
    return false, { { x = cx, y = cy, reason = "unknown_building" } }
  end
  ensure_farm()
  local exits = exit_cells()
  local conflicts = {}
  for _, cell in ipairs(footprint_of(def, cx, cy)) do
    local reason = cell_conflict(def, cell.x, cell.y, exits)
    if reason then
      conflicts[#conflicts + 1] = { x = cell.x, y = cell.y, reason = reason }
    end
  end
  return #conflicts == 0, conflicts
end

-- 建造预览（建造模式 UI 与 headless 验收共用）：面向格为 footprint 锚点
function M.preview(facing)
  local id = selected
  if not id then
    return { ok = false, reason = "no_selection", cells = {}, conflicts = {} }
  end
  local pid = player.ensure()
  if not pid then
    return { ok = false, reason = "no_player", cells = {}, conflicts = {} }
  end
  local pos = transform.get(pid)
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  local cx, cy = cell[1], cell[2]
  local def = def_of(id)
  local ok, conflicts = M.can_place(id, cx, cy)
  return {
    building = id,
    name = def and def.name or id,
    anchor = { x = cx, y = cy },
    cells = def and footprint_of(def, cx, cy) or {},
    ok = ok,
    conflicts = conflicts
  }
end

local function apply_terrain(rec, blocked)
  if not ground_id or not entity.is_alive(ground_id) then
    return
  end
  for _, cell in ipairs(record_cells(rec)) do
    if bounds_ok(cell.x, cell.y) then
      tilemap.set_solid(ground_id, cell.x, cell.y, blocked and true or false)
    end
  end
end

local function despawn(rec)
  if rec.entity and entity.is_alive(rec.entity) then
    pcall(entity.destroy, rec.entity)
  end
  rec.entity = nil
end

local function spawn(rec)
  local def = def_of(rec.building)
  if not def then
    return
  end
  if rec.entity and entity.is_alive(rec.entity) then
    return
  end
  local level = math.max(1, math.min(#def.levels, tonumber(rec.level) or 1))
  local visual = def.levels[level]
  local ok, id = pcall(entity.spawn, "Building:" .. rec.building .. ":" .. tostring(rec.seq or 0), nil)
  if not ok or not id then
    log.warn("[building] spawn failed for " .. tostring(rec.building))
    return
  end
  rec.entity = id
  local cx = rec.cx + def.w / 2
  local cy = rec.cy + def.h / 2
  pcall(transform.set, id, cx * TILE, cy * TILE)
  local component = "texture="
    .. visual.sprite
    .. ";size="
    .. visual.size
    .. ";pivot=0.5,0.5;tint=1.000,1.000,1.000,1.000;"
    .. SORTING
  local sprite_ok, sprite_err = pcall(sprite.set, id, component)
  if not sprite_ok then
    log.warn("[building] sprite.set failed: " .. tostring(sprite_err))
  end
end

-- 版本门：map.state 与存档快照都写着 `version`，但此前**只写不校验**——schema
-- 变更后旧数据会被当成新格式静默读入（字段缺失走默认，看着像"存档没坏"其实丢了建筑）。
-- 口径与引擎的 `save_version` 门一致：不认识的版本一律拒绝、绝不猜。
local function version_ok(data, where)
  if type(data) ~= "table" then
    return false
  end
  if tonumber(data.version) == STATE_VERSION then
    return true
  end
  log.warn("[building] " .. where .. " 版本不匹配：" .. tostring(data.version)
    .. "，当前 " .. STATE_VERSION .. "，本次不恢复")
  return false
end

-- 把"这栋建筑建到过几级"记进 core.state 的 vars（M34-05）。两个读取方都不在山庄地图上：
-- 送礼发生在云泽村、节气条件在对话里判，而建筑记录是按地图存进 map.state 的，
-- 站在村里读不到山庄的茶棚——所以庄园属性单独记一份。取**历史最高**：拆了不回收，
-- 与"解锁过的节气事件不再锁回去"是同一口径（写方只有这里，读方走 data/buildings.lua 的 level_var）。
local function record_levels()
  for _, key in ipairs(order) do
    local rec = records[key]
    if rec then
      local var = building_data.level_var(rec.building)
      local reached = tonumber(state.get_var(var)) or 0
      if rec.level > reached then
        state.set_var(var, rec.level, "building", "level_" .. tostring(rec.level))
      end
    end
  end
end

local function persist()
  local list = {}
  for _, key in ipairs(order) do
    local rec = records[key]
    if rec then
      list[#list + 1] = {
        key = rec.key,
        cx = rec.cx,
        cy = rec.cy,
        building = rec.building,
        level = rec.level,
        seq = rec.seq
      }
    end
  end
  map.state(STATE_KEY, { version = STATE_VERSION, seq = seq, list = list })
  record_levels()
end

local function load_state()
  records = {}
  order = {}
  seq = 0
  local saved = map.state(STATE_KEY)
  if not version_ok(saved, "map.state") then
    return
  end
  seq = tonumber(saved.seq) or 0
  for _, entry in ipairs(saved.list or {}) do
    if type(entry) == "table" and def_of(entry.building) and type(entry.cx) == "number" and type(entry.cy) == "number" then
      local key = entry.key or key_of(entry.cx, entry.cy)
      records[key] = {
        key = key,
        cx = entry.cx,
        cy = entry.cy,
        building = entry.building,
        level = math.max(1, tonumber(entry.level) or 1),
        seq = tonumber(entry.seq) or 0
      }
      order[#order + 1] = key
    end
  end
  -- 老档回填：M34-05 之前建的茶棚没有等级 var，进图时按现有记录补一次，
  -- 否则"茶棚已经三级"的存档读回来会听不到春分茶会。
  record_levels()
end

-- 切图/读档后重放：绑定 GroundMap、重生视觉、重打阻挡（幂等）
function M.on_enter(map_id)
  current_map = map_id or current_map
  local ground = entity.find("GroundMap")
  ground_id = (ground and entity.is_alive(ground)) and ground or nil
  local dims = ground_id and tilemap.get(ground_id) or nil
  width = (dims and dims.width) or 0
  height = (dims and dims.height) or 0
  load_state()
  for _, key in ipairs(order) do
    local rec = records[key]
    spawn(rec)
    apply_terrain(rec, true)
  end
  return true
end

-- 放置：失败无任何副作用（不写 state、不 spawn、不改 solid）
function M.place(building_id, cx, cy)
  local def = def_of(building_id)
  if not def then
    state.last_action = "build_failed"
    return false, { { x = cx, y = cy, reason = "unknown_building" } }
  end
  local ok, conflicts = M.can_place(building_id, cx, cy)
  if not ok then
    state.last_action = "build_failed"
    return false, conflicts
  end
  seq = seq + 1
  local key = key_of(cx, cy)
  local rec = { key = key, cx = cx, cy = cy, building = building_id, level = 1, seq = seq }
  records[key] = rec
  order[#order + 1] = key
  persist()
  spawn(rec)
  apply_terrain(rec, true)
  state.last_action = "build_" .. building_id
  audio.sfx("sfx_craft")
  return true, rec
end

-- 面向格上的建筑记录（nil = 无）
function M.at(cx, cy)
  return cell_owner(cx, cy)
end

function M.facing_building(facing)
  local pid = player.ensure()
  if not pid then
    return nil
  end
  local pos = transform.get(pid)
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  return cell_owner(cell[1], cell[2])
end

local function remove_record(rec)
  despawn(rec)
  apply_terrain(rec, false)
  records[rec.key] = nil
  for index, key in ipairs(order) do
    if key == rec.key then
      table.remove(order, index)
      break
    end
  end
  persist()
end

-- 拆除：制作任务进行中拒绝（crafting 经 set_job_guard 挂接）
function M.remove(cx, cy)
  local rec = cell_owner(cx, cy)
  if not rec then
    state.last_action = "no_building"
    return false, "no_building"
  end
  if job_guard and job_guard(rec.key) then
    state.last_action = "crafting_pending"
    return false, "crafting_pending"
  end
  remove_record(rec)
  state.last_action = "demolish_" .. rec.building
  return true, rec
end

-- 升级：材料不足/满级拒绝且不扣材料
function M.upgrade(cx, cy)
  local rec = cell_owner(cx, cy)
  if not rec then
    return false, "no_building"
  end
  local def = def_of(rec.building)
  local max_level = building_data.max_level(def)
  if rec.level >= max_level then
    state.last_action = "max_level"
    return false, "max_level"
  end
  for _, cost in ipairs(def.upgrade_cost or {}) do
    if inv.count(state.inventory, cost.item) < cost.count then
      state.last_action = "not_enough"
      return false, "not_enough"
    end
  end
  for _, cost in ipairs(def.upgrade_cost or {}) do
    inv.remove(state.inventory, cost.item, cost.count)
  end
  rec.level = rec.level + 1
  persist()
  -- 换 level 贴图：销毁重建最稳（实体判活 + spawn 幂等）
  despawn(rec)
  spawn(rec)
  state.last_action = "upgrade_" .. rec.building .. "_" .. tostring(rec.level)
  audio.sfx("sfx_craft")
  return true, rec.level
end

function M.set_job_guard(fn)
  job_guard = fn
end

function M.has_job(key)
  return job_guard ~= nil and job_guard(key) == true
end

-- 建造模式（B 开关；不新增 state.lua 模式，移动不受限）
function M.build_mode()
  return build_mode
end

function M.set_build_mode(enabled)
  build_mode = enabled == true
  if build_mode and not selected then
    local ids = building_data.ids()
    selected = ids[1]
  end
  state.last_action = build_mode and "build_mode_on" or "build_mode_off"
  return build_mode
end

function M.selected()
  return selected
end

function M.set_selected(id)
  if not def_of(id) then
    return false, "unknown_building"
  end
  selected = id
  return true
end

function M.cycle_selected()
  local ids = building_data.ids()
  if #ids == 0 then
    return nil
  end
  local index = 1
  for i, id in ipairs(ids) do
    if id == selected then
      index = i
      break
    end
  end
  selected = ids[index % #ids + 1]
  state.last_action = "select_" .. selected
  return selected
end

function M.list()
  local out = {}
  for _, key in ipairs(order) do
    local rec = records[key]
    out[#out + 1] = {
      key = rec.key,
      cx = rec.cx,
      cy = rec.cy,
      building = rec.building,
      level = rec.level,
      seq = rec.seq
    }
  end
  return out
end

function M.reset()
  for _, key in ipairs(order) do
    despawn(records[key])
  end
  records = {}
  order = {}
  seq = 0
  build_mode = false
  selected = nil
  map.state(STATE_KEY, { version = STATE_VERSION, seq = 0, list = {} })
end

function M.capture()
  return {
    version = STATE_VERSION,
    seq = seq,
    list = M.list()
  }
end

-- restore 只写内存与 map.state，随后按当前地图重放视觉（实体判活，安全幂等）
-- 版本不匹配返回 `false, "bad_version"`：save_manager 会据此回滚整批恢复，
-- 而不是让建筑记录停在"按旧 schema 猜读出来"的空/半状态。
function M.restore(data)
  if not version_ok(data, "snapshot") then
    return false, "bad_version"
  end
  records = {}
  order = {}
  seq = tonumber(data.seq) or 0
  for _, entry in ipairs(data.list or {}) do
    if type(entry) == "table" and def_of(entry.building) then
      local key = entry.key or key_of(entry.cx, entry.cy)
      records[key] = {
        key = key,
        cx = tonumber(entry.cx) or 0,
        cy = tonumber(entry.cy) or 0,
        building = entry.building,
        level = math.max(1, tonumber(entry.level) or 1),
        seq = tonumber(entry.seq) or 0
      }
      order[#order + 1] = key
    end
  end
  persist()
  if ground_id and entity.is_alive(ground_id) then
    for _, key in ipairs(order) do
      local rec = records[key]
      spawn(rec)
      apply_terrain(rec, true)
    end
  end
end

return M
