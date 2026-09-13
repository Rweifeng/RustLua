-- gather.lua：《江湖有田》野外采集系统（M19-01）。
--
-- 纯数据驱动：采集点全部来自 data/gather.lua，新增/调整只改数据，不改本文件
-- 与 Rust 核心（与 data/enemies / data/maps 同约定）。
--
-- 职责：
--   * 在 interaction 框架里注册一个 `gather` 目标（priority 5，介于农田 0 与
--     设施 10 之间）：玩家面向的格命中当前地图的采集点即可按 E 采集。
--   * 采集成功：inv.add 产出（背包满则拒绝且不消耗节点）→ 记录已采
--     （map.state("gather")，含 state_version 与采集日）→ state.last_action
--     = "gather_<item>" → state.labor_minutes += minutes。
--   * 刷新语义：节点记 { day = 采集日 }；clock.now().day - day >= respawn_days
--     视为已刷新（惰性清除，幂等）。respawn_days = 0/缺省 = 一次性。
--   * on_enter / capture / restore / reset：只写内存与 map.state，
--     **不直接操作实体**（M18-02 的陈旧 id 教训：视觉/实体统一由调用方重放）。
--
-- 与农田的一致取舍：采集只累计 state.labor_minutes，**不推进 clock**。
--
-- 接入点：interaction.lua 的 ensure_builtins() 里 `pcall(require, "systems.gather")`
-- 成功后调用 M.ensure_target()（缺文件/加载失败时静默跳过，不影响其它交互）。
local state = require("core.state")
local player = require("player.player")
local gather_data = require("data.gather")
local items_data = require("data.items")
local tools_data = require("data.tools")

local M = {}

local TILE = 32
local STATE_KEY = "gather"
local STATE_VERSION = 1

-- 运行期：已采集记录 node_id → { day = 采集发生的游戏日 }
local gathered = {}
-- 当前装载的地图（map.state 命名空间的装载键）
local loaded_map = nil
-- interaction 目标注册幂等标志
local registered = false

local function definition(id)
  return gather_data.get(id)
end

-- 物品显示名（items / tools 都接受；未知回退 id）
local function item_name(id)
  local entry = items_data[id] or tools_data[id]
  return (entry and entry.name) or tostring(id)
end

local function item_exists(id)
  return type(id) == "string" and (items_data[id] ~= nil or tools_data[id] ~= nil)
end

local function current_day()
  local now = clock.now()
  return tonumber(now.day) or 1
end

local function current_map()
  local ok, map_manager = pcall(require, "world.map_manager")
  if ok and type(map_manager) == "table" and type(map_manager.current) == "function" then
    local map_id = map_manager.current()
    if type(map_id) == "string" then
      return map_id
    end
  end
  return map.current() or scene.reload()
end

-- 是否已刷新（respawn_days <= 0 视为一次性，永不刷新）
local function refreshed(node, record)
  if not record then
    return true
  end
  local days = tonumber(node.respawn_days) or 0
  if days <= 0 then
    return false
  end
  return (current_day() - (tonumber(record.day) or 0)) >= days
end

local function persist()
  local nodes = {}
  for id, record in pairs(gathered) do
    nodes[id] = { day = tonumber(record.day) or 0 }
  end
  map.state(STATE_KEY, { state_version = STATE_VERSION, day = current_day(), nodes = nodes })
end

-- 幂等装载：切图或首次进入时按 map.state("gather") 恢复已采记录。
function M.ensure_loaded(map_id)
  local target = map_id or current_map()
  if loaded_map == target then
    return false
  end
  loaded_map = target
  gathered = {}
  local saved = map.state(STATE_KEY)
  if type(saved) == "table" and type(saved.nodes) == "table" then
    for id, record in pairs(saved.nodes) do
      if type(id) == "string" and type(record) == "table" then
        gathered[id] = { day = tonumber(record.day) or 0 }
      end
    end
  end
  return true
end

function M.on_enter(map_id)
  return M.ensure_loaded(map_id)
end

-- 睡觉/跨天：清掉已到刷新期的记录（真实的“次日刷新”）。UI 刷新可调用；
-- 即使不接线，query/act 里的惰性刷新也会给出同样结果（幂等）。
function M.on_day_start()
  M.ensure_loaded()
  local day = current_day()
  local changed = false
  for id, record in pairs(gathered) do
    local node = definition(id)
    if node and refreshed(node, record) then
      gathered[id] = nil
      changed = true
    end
    if not node then
      gathered[id] = nil
      changed = true
    end
  end
  if changed then
    persist()
  end
  return changed
end

-- 面前格（cell 0-index；与 farm/map 同约定）
local function facing_cell(facing)
  local pid = player.ensure()
  if not pid then
    return nil
  end
  local pos = transform.get(pid)
  if type(pos) ~= "table" then
    return nil
  end
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  return { x = cell[1], y = cell[2] }
end

-- 当前地图上位于 (cx, cy) 的采集点（数据表顺序确定；同格取第一个）
function M.node_at(map_id, cx, cy)
  local target = map_id or current_map()
  for _, node in ipairs(gather_data.nodes) do
    if node.map == target and node.cell.x == cx and node.cell.y == cy then
      return node
    end
  end
  return nil
end

local function state_of(node)
  M.ensure_loaded()
  local record = gathered[node.id]
  if not record then
    return "available"
  end
  if refreshed(node, record) then
    gathered[node.id] = nil
    persist()
    return "available"
  end
  return "gathered"
end

-- 装备槽里是否有某件工具/武器/物品
local function equipped(item_id)
  if type(item_id) ~= "string" then
    return true
  end
  if not inv.exists(state.inventory) then
    return false
  end
  for _, entry in pairs(inv.equip_list(state.inventory) or {}) do
    if entry == item_id then
      return true
    end
  end
  return inv.count(state.inventory, item_id) > 0
end

-- 采集前置判定：返回 ok, reason（reason 为可读中文，prompt 直接展示）
local function gate(node)
  if not node then
    return false, "无采集目标"
  end
  if not item_exists(node.item) then
    return false, "资料缺失：" .. tostring(node.item)
  end
  if node.requires and not equipped(node.requires) then
    return false, "需要" .. item_name(node.requires)
  end
  return true
end

-- 只读：当前地图声明表（headless 断言/UI 提示用）
function M.nodes(map_id)
  return gather_data.list(map_id)
end

-- 只读：面向格可采集性快照（不改状态；会惰性刷新过期节点）
-- 返回 { node = 定义, cell = {x,y}, state = "available"|"gathered", ok, reason }
function M.available(facing)
  local cell = facing_cell(facing)
  if not cell then
    return nil, "no_player"
  end
  local map_id = current_map()
  local node = map_id and M.node_at(map_id, cell.x, cell.y) or nil
  if not node then
    return nil, "no_node"
  end
  local node_state = state_of(node)
  local ok, reason = gate(node)
  return {
    node = node,
    cell = cell,
    state = node_state,
    ok = ok and node_state == "available",
    reason = reason
  }
end

-- 已采集记录只读快照（headless 断言用）：node_id → 采集日
function M.gathered()
  M.ensure_loaded()
  local out = {}
  for id, record in pairs(gathered) do
    out[id] = tonumber(record.day) or 0
  end
  return out
end

function M.gathered_ids()
  M.ensure_loaded()
  local ids = {}
  for id in pairs(gathered) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

function M.is_gathered(node_id)
  M.ensure_loaded()
  local node = definition(node_id)
  local record = gathered[node_id]
  if not node or not record then
    return false
  end
  if refreshed(node, record) then
    gathered[node_id] = nil
    persist()
    return false
  end
  return true
end

-- 采集（interaction.interact 真实路径）：成功返回 "gather_<item>"，
-- 失败返回原因字符串（写入 state.last_action）。
function M.act(facing)
  M.ensure_loaded()
  local cell = facing_cell(facing)
  if not cell then
    state.last_action = "no_player"
    return state.last_action
  end
  local map_id = current_map()
  local node = map_id and M.node_at(map_id, cell.x, cell.y) or nil
  if not node then
    state.last_action = "no_target"
    return state.last_action
  end
  local node_state = state_of(node)
  if node_state == "gathered" then
    state.last_action = "gathered"
    return state.last_action
  end
  local ok, reason = gate(node)
  if not ok then
    state.last_action = "gather_blocked"
    return reason
  end
  local count = tonumber(node.count) or 1
  local leftover = tonumber(inv.add(state.inventory, node.item, count)) or 0
  if leftover >= count then
    -- 一件也没装下：拒绝且不消耗节点（与 farming/crafting 的容量语义一致）
    state.last_action = "full"
    return state.last_action
  end
  if leftover > 0 then
    -- 部分装入：按实际装入量记账（不吞物品）
    count = count - leftover
  end
  gathered[node.id] = { day = current_day() }
  persist()
  local minutes = tonumber(node.minutes) or 0
  if minutes > 0 then
    state.labor_minutes = (state.labor_minutes or 0) + minutes
  end
  state.last_action = "gather_" .. tostring(node.item)
  -- 音效（缺文件/无设备静默降级）
  pcall(function()
    require("core.audio").sfx("sfx_harvest")
  end)
  return state.last_action
end

-- interaction 目标（priority 5：高于农田 0，低于设施 10）
function M.ensure_target()
  if registered then
    return true
  end
  registered = true
  local interaction = require("systems.interaction")
  interaction.register({
    id = "gather",
    priority = 5,
    query = function(facing)
      local map_id = current_map()
      if not map_id then
        return nil, "no_map"
      end
      local cell = facing_cell(facing)
      if not cell then
        return nil, "no_player"
      end
      local node = M.node_at(map_id, cell.x, cell.y)
      if not node then
        return nil, "no_node"
      end
      local node_state = state_of(node)
      local ok, reason = gate(node)
      return {
        cell = cell,
        node = node,
        state = node_state,
        distance = 1,
        facing = 1,
        reason = (not ok) and reason or (node_state == "gathered" and "gathered" or nil)
      }
    end,
    can_interact = function(candidate)
      local node = candidate and candidate.node
      if not node then
        return false, "no_node"
      end
      if candidate.state == "gathered" then
        return false, "已采集"
      end
      local ok, reason = gate(node)
      if not ok then
        return false, reason
      end
      return true
    end,
    prompt = function(candidate, reason)
      local node = candidate and candidate.node
      if not node then
        return reason or "E: 采集"
      end
      if candidate.state == "gathered" then
        return "（已采集 · " .. item_name(node.item) .. "）"
      end
      if reason then
        return reason
      end
      return "E: 采集 " .. item_name(node.item)
    end,
    interact = function(candidate, facing)
      return M.act(facing)
    end
  })
  return true
end

-- 新档：清空内存记录并落一份空快照（map.state 是当前场景命名空间）。
function M.reset()
  gathered = {}
  loaded_map = nil
  persist()
end

function M.capture()
  local nodes = {}
  for id, record in pairs(gathered) do
    nodes[id] = { day = tonumber(record.day) or 0 }
  end
  return { state_version = STATE_VERSION, day = current_day(), nodes = nodes }
end

-- 只写内存与 map.state（实体由调用方重放；本模块不持有实体 id）
function M.restore(data)
  gathered = {}
  if type(data) == "table" and type(data.nodes) == "table" then
    for id, record in pairs(data.nodes) do
      if type(id) == "string" and type(record) == "table" then
        gathered[id] = { day = tonumber(record.day) or 0 }
      end
    end
  end
  loaded_map = current_map()
  persist()
end

return M
