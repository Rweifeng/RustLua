-- relationship.lua：角色关系与门派/村庄声望运行时（M18-04）。
--
-- 职责：
--   * 关系分：0..100（默认 0），档位由 data.relationships.tiers 推导；
--     add 记录有界变更流水（仿 state.lua var_events，LIMIT 32，含来源/事件）。
--   * 声望：per faction 的 [min, max]（默认 0），add_reputation clamp 到范围。
--   * 幂等奖励：reward_once(key, fn) 首次执行 fn 并记录 key，重复调用拒绝；
--     keys 随存档持久化（任务/对话/日常一次性奖励的统一锚点）。
--   * 送礼：gift 校验 NPC/礼物配置/背包持有，成功 = 消耗 1 件 + 加分；
--     每日限次（docs/33 §8.1）：同一 NPC 每日有效礼物最多 1 次，重复送
--     返回 "gift_today" 且不扣物不加分；gift_facing 面向 NPC 送礼（面前格或
--     相邻 1 格，只读 npc.query 定位）。
--
-- 数据定义见 data/relationships.lua（纯数据 + validate_all）；引用完整性在
-- 加载期校验（main.validate_content），运行时对未知 NPC/阵营返回明确 reason，
-- 不静默失败。
local state = require("core.state")
local data = require("data.relationships")
local npcs = require("data.npcs")
local npc = require("world.npc")
local player = require("player.player")

local M = {}

local TILE = 32
-- 变更流水上限（与 state.var_events 同约定），避免存档无限膨胀。
local EVENT_LIMIT = 32

local points = {}      -- npc_id → 关系分（0..100）
local reputation = {}  -- faction → 声望值（faction.min..faction.max）
local rewarded = {}    -- key → true（幂等奖励锚点，随存档持久化）
local gifted_day = {}  -- npc_id → 最后一次有效送礼的游戏日（每日限次，docs/33 §8.1）
local events = {}      -- 有界变更流水（关系/声望共用）

local function record_event(kind, id, delta, before, after, source, event)
  events[#events + 1] = {
    kind = kind,
    id = id,
    delta = delta,
    before = before,
    after = after,
    source = tostring(source or "unknown"),
    event = tostring(event or "")
  }
  while #events > EVENT_LIMIT do
    table.remove(events, 1)
  end
end

function M.reset()
  points = {}
  reputation = {}
  rewarded = {}
  gifted_day = {}
  events = {}
end

-- 关系分（0..100，默认 0）；未知 NPC 返回 nil。
function M.points(npc_id)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return nil
  end
  return tonumber(points[npc_id]) or 0
end

-- 关系档位（数据 tiers 推导）；未知 NPC 返回 nil。
function M.tier(npc_id)
  local value = M.points(npc_id)
  if value == nil then
    return nil
  end
  return data.tier_for(value)
end

-- 加关系分：未知 NPC → false,"unknown_npc"；delta 非数值 → false,"bad_delta"；
-- clamp 到 0..100；来源/事件入有界流水。返回 true,新分数。
function M.add(npc_id, delta, source, event)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return false, "unknown_npc"
  end
  local step = tonumber(delta)
  if not step then
    return false, "bad_delta"
  end
  local before = tonumber(points[npc_id]) or 0
  local after = math.max(0, math.min(100, before + step))
  points[npc_id] = after
  record_event("relationship", npc_id, step, before, after, source, event)
  return true, after
end

-- 声望值（faction.min..max，默认 0）；未知阵营返回 nil。
function M.reputation(faction_id)
  local def = data.faction(faction_id)
  if not def then
    return nil
  end
  return tonumber(reputation[faction_id]) or 0
end

-- 加声望：未知阵营 → false,"unknown_faction"；delta 非数值 → false,"bad_delta"；
-- clamp 到 [min, max]。返回 true,新值。
function M.add_reputation(faction_id, delta, source, event)
  local def = data.faction(faction_id)
  if not def then
    return false, "unknown_faction"
  end
  local step = tonumber(delta)
  if not step then
    return false, "bad_delta"
  end
  local before = tonumber(reputation[faction_id]) or 0
  local after = math.max(def.min, math.min(def.max, before + step))
  reputation[faction_id] = after
  record_event("reputation", faction_id, step, before, after, source, event)
  return true, after
end

-- 幂等奖励：key 非字符串 → false；已完成 → false,"already_rewarded" 且不执行
-- fn；首次执行 fn() 并记录 key → true。keys 随存档持久化。
function M.reward_once(key, fn)
  if type(key) ~= "string" or key == "" then
    return false, "bad_key"
  end
  if rewarded[key] then
    return false, "already_rewarded"
  end
  if type(fn) ~= "function" then
    return false, "bad_fn"
  end
  fn()
  rewarded[key] = true
  return true
end

-- 送礼：NPC 存在且配置了该礼物（否则 not_giftable）、背包持有（否则
-- not_enough）、当日未送过（否则 gift_today，不扣物不加分）；成功 =
-- 消耗 1 件 + gift_points 加分（来源 gift，事件 = 物品 id）。
-- 返回 true,新分数。
function M.gift(npc_id, item_id)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return false, "unknown_npc"
  end
  if type(item_id) ~= "string" or not data.gift_points(npc_id, item_id) then
    return false, "not_giftable"
  end
  if inv.count(state.inventory, item_id) <= 0 then
    return false, "not_enough"
  end
  -- 每日限次（docs/33 §8.1）：同一 NPC 每日有效礼物最多 1 次。
  local day = clock.now().day
  if gifted_day[npc_id] == day then
    return false, "gift_today"
  end
  inv.remove(state.inventory, item_id, 1)
  local ok, after = M.add(npc_id, data.gift_points(npc_id, item_id), "gift", item_id)
  if not ok then
    -- NPC 已在上面的入口校验，正常不可达；防御性回滚避免「扣物未加分」。
    inv.add(state.inventory, item_id, 1)
    log.warn("[relationship] gift add failed after consume: " .. tostring(npc_id) .. "/" .. tostring(item_id))
    return false, after
  end
  gifted_day[npc_id] = day
  return true, after
end

-- 面前格（含相邻 1 格）找 NPC 并按背包顺序送出第一件其可收的礼物。
-- 定位只读（npc.query），不改 npc.lua。返回 true,送出的物品 id；
-- 无玩家 → no_player；范围内无 NPC → no_npc_near；无可礼物 → no_gift。
function M.gift_facing(facing)
  local pid = player.ensure()
  if not pid then
    return false, "no_player"
  end
  local ok, pos = pcall(transform.get, pid)
  if not ok or type(pos) ~= "table" then
    return false, "no_player"
  end
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  local cx, cy = cell[1], cell[2]
  local map_id = map.current() or scene.reload()

  -- 两轮匹配：先面前格精确命中，再相邻 1 格（Chebyshev 距离 <= 1）；
  -- 同轮按 data.npcs 的稳定 persist_id 排序取第一个。
  local target = nil
  for _, exact_only in ipairs({ true, false }) do
    for _, def in ipairs(npcs.list()) do
      local info = npc.query(def.persist_id)
      if info and info.live and info.map == map_id and info.cell_x and info.cell_y then
        local dx = math.abs(info.cell_x - cx)
        local dy = math.abs(info.cell_y - cy)
        local matched = exact_only and (dx == 0 and dy == 0) or (dx <= 1 and dy <= 1)
        if matched then
          target = def.persist_id
          break
        end
      end
    end
    if target then
      break
    end
  end
  if not target then
    return false, "no_npc_near"
  end

  -- 每日限次（docs/33 §8.1）：目标今日已收过礼 → 直接提示 gift_today，
  -- 不再扫背包（否则会误报 no_gift）。
  local today = clock.now().day
  if gifted_day[target] == today then
    return false, "gift_today"
  end

  -- 背包顺序扫描：第一件该 NPC 可收的礼物。
  local inventory = inv.get(state.inventory)
  local capacity = tonumber(inventory and inventory.capacity) or 0
  for index = 1, capacity do
    local stack = inventory.slots and inventory.slots[index]
    if stack and stack.item and stack.item ~= "" and (tonumber(stack.count) or 0) > 0 then
      if data.gift_points(target, stack.item) then
        local gifted, result = M.gift(target, stack.item)
        if gifted then
          return true, stack.item
        end
      end
    end
  end
  return false, "no_gift"
end

-- 只读变更流水副本（headless 验收 / 调试用）。
function M.events()
  local out = {}
  for index, entry in ipairs(events) do
    out[index] = {
      kind = entry.kind,
      id = entry.id,
      delta = entry.delta,
      before = entry.before,
      after = entry.after,
      source = entry.source,
      event = entry.event
    }
  end
  return out
end

-- 存档：关系/声望/已发奖励/每日送礼记录/流水全量快照（基础默认值由 restore 兜底）。
function M.capture()
  local points_data = {}
  for npc_id, value in pairs(points) do
    if type(npc_id) == "string" then
      points_data[npc_id] = value
    end
  end
  local reputation_data = {}
  for faction_id, value in pairs(reputation) do
    if type(faction_id) == "string" then
      reputation_data[faction_id] = value
    end
  end
  local rewarded_data = {}
  for key, flag in pairs(rewarded) do
    if type(key) == "string" and flag == true then
      rewarded_data[key] = true
    end
  end
  local gifted_day_data = {}
  for npc_id, day in pairs(gifted_day) do
    if type(npc_id) == "string" and type(day) == "number" then
      gifted_day_data[npc_id] = day
    end
  end
  local events_data = {}
  for index, entry in ipairs(events) do
    events_data[index] = {
      kind = entry.kind,
      id = entry.id,
      delta = entry.delta,
      before = entry.before,
      after = entry.after,
      source = entry.source,
      event = entry.event
    }
  end
  return {
    points = points_data,
    reputation = reputation_data,
    rewarded = rewarded_data,
    gifted_day = gifted_day_data,
    events = events_data
  }
end

-- 读档：旧档缺字段（data 非 table / 缺子表）用默认；points/reputation 重新
-- clamp 到范围，未知 id/非法条目丢弃。
function M.restore(data_in)
  points = {}
  reputation = {}
  rewarded = {}
  gifted_day = {}
  events = {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.points) == "table" then
    for npc_id, value in pairs(data_in.points) do
      if type(npc_id) == "string" and npcs.get(npc_id) then
        points[npc_id] = math.max(0, math.min(100, tonumber(value) or 0))
      end
    end
  end
  if type(data_in.reputation) == "table" then
    for faction_id, value in pairs(data_in.reputation) do
      local def = data.faction(faction_id)
      if def then
        local restored = tonumber(value) or 0
        reputation[faction_id] = math.max(def.min, math.min(def.max, restored))
      end
    end
  end
  if type(data_in.rewarded) == "table" then
    for key, flag in pairs(data_in.rewarded) do
      if type(key) == "string" and key ~= "" and flag == true then
        rewarded[key] = true
      end
    end
  end
  if type(data_in.gifted_day) == "table" then
    for npc_id, day in pairs(data_in.gifted_day) do
      if type(npc_id) == "string" and npcs.get(npc_id) and type(day) == "number" then
        gifted_day[npc_id] = day
      end
    end
  end
  if type(data_in.events) == "table" then
    for _, entry in ipairs(data_in.events) do
      if type(entry) == "table" then
        record_event(
          tostring(entry.kind or ""),
          tostring(entry.id or ""),
          tonumber(entry.delta) or 0,
          tonumber(entry.before) or 0,
          tonumber(entry.after) or 0,
          entry.source,
          entry.event
        )
        if #events >= EVENT_LIMIT then
          break
        end
      end
    end
  end
end

return M
