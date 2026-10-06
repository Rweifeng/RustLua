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
--   * 社交乘子（M34-05，docs/33 §8.1）：add 的正增量统一乘 `social_multiplier()`
--     = 1 + 建筑档位加成（茶棚 2 级 +10%）+ 饮茶加成（当日喝过茶 +10%）。
--     乘子是**只读查询**，来源分别是 data.buildings 的等级行与 data.relationships 的
--     social.tea_bonus；当日锚点走 core.state 的标量 var（随 state provider 进存档）。
--
-- 数据定义见 data/relationships.lua（纯数据 + validate_all）；引用完整性在
-- 加载期校验（main.validate_content），运行时对未知 NPC/阵营返回明确 reason，
-- 不静默失败。
local state = require("core.state")
local data = require("data.relationships")
local building_data = require("data.buildings")
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

-- NPC 所属声望阵营（data.npcs.lua 的 faction 字段）；无阵营/未知 NPC → nil。
function M.faction_of(npc_id)
  local def = type(npc_id) == "string" and npcs.get(npc_id) or nil
  return def and def.faction or nil
end

-- 关系分是否达到某个档位（对话条件用；tier_key 见 data.tiers 的 key）。
-- 未知 NPC/tier_key → false（宁可挡住，也不让写错的档位名静默放行）。
function M.affinity_met(npc_id, tier_key)
  local required = data.tier_by_key(tier_key)
  local value = M.points(npc_id)
  if not required or value == nil then
    return false
  end
  return value >= required.min
end

-- 声望档位条目（docs/33 §8.2 的五档）；未知阵营返回 nil。
-- 负声望落在第一档（陌生，无任何效果）——查表规则见 data.reputation_tier_for。
function M.reputation_tier(faction_id)
  local def = data.faction(faction_id)
  if not def then
    return nil
  end
  return data.reputation_tier_for(M.reputation(faction_id))
end

-- 声望是否达到某个档位（任务目标用；tier_key 见 data.reputation_tiers 的 key）。
function M.reputation_met(faction_id, tier_key)
  local required = data.reputation_tier_by_key(tier_key)
  if not required or not data.faction(faction_id) then
    return false
  end
  return M.reputation(faction_id) >= required.min
end

-- 该阵营当前给的商店折扣百分数（整数）；无阵营/无档位 → 0（原价）。
function M.reputation_price_off(faction_id)
  local tier = M.reputation_tier(faction_id)
  local effects = tier and tier.effects
  return effects and tonumber(effects.price_off) or 0
end

-- 加关系分：未知 NPC → false,"unknown_npc"；delta 非数值 → false,"bad_delta"；
-- 正增量先乘当日社交乘子（M34-05，见下方 social_multiplier），clamp 到 0..100；
-- 来源/事件入有界流水。返回 true,新分数。
-- 流水里记的 `delta` 是**乘完之后真正加上去的那一份**：乘子改的是到账数字，
-- 诊断要看的也是"这一笔实际加了几分"。
function M.add(npc_id, delta, source, event)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return false, "unknown_npc"
  end
  local step = tonumber(delta)
  if not step then
    return false, "bad_delta"
  end
  if step > 0 then
    step = math.floor(step * M.social_multiplier() + 0.5)
  end
  local before = tonumber(points[npc_id]) or 0
  local after = math.max(0, math.min(100, before + step))
  points[npc_id] = after
  record_event("relationship", npc_id, step, before, after, source, event)
  return true, after
end

-- ---- 当日社交乘子（M34-05，docs/33 §8.1）----
-- 这是一条**只读查询**：`add` 里不判断建筑、不判断节气，只问一句"今天乘多少"，
-- 于是"社交收益"这条循环只有一个乘子来源，不会在三个调用点各写一套条件。
-- 两份数字各有归属，都不在这里重复写一遍：
--   建筑档位加成 = data/buildings.lua 的 levels[i].social_bonus（茶棚 2 级 +10%，
--                  按"建到过几级"累加；等级读 systems/building.lua 记进 state.vars 的历史最高值）
--   饮茶加成     = data/relationships.lua 的 social.tea_bonus（今天喝过茶再 +10%）
-- 两项**相加**（不是相乘），今天最高 1 + 0.1 + 0.1 = 1.2；取整用四舍五入，
-- 因为 +10% 落在 5 分礼上如果向下取整就还是 5 分——玩家看不出任何变化的一条加成等于没做。

--- 饮茶锚点的 var 名：记的是"哪一天喝过"（标量，与 weather_day 同族做法），
--- 因此随既有 state provider 自动进出存档，不需要新 provider。
local TEA_VAR = "tea_day"

--- 今天喝过茶吗（跨天自动失效：日期不等就是没喝过，不需要每日重置钩子）。
function M.tea_today()
  local day = tonumber(state.get_var(TEA_VAR))
  return day ~= nil and day == math.floor(tonumber(clock.now().day) or 0)
end

--- 记下"今天喝过茶"（由 player/inventory.lua 在 use 一件 `use.social_day` 的物品后调用）。
function M.mark_tea()
  local day = math.floor(tonumber(clock.now().day) or 0)
  return state.set_var(TEA_VAR, day, "tea", "drink")
end

--- 当前社交乘子（1 = 没有任何加成）。
function M.social_multiplier()
  local total = 1
  for _, id in ipairs(building_data.ids()) do
    local reached = tonumber(state.get_var(building_data.level_var(id))) or 0
    total = total + building_data.social_bonus_at(building_data.get(id), reached)
  end
  if M.tea_today() then
    total = total + (tonumber(data.social and data.social.tea_bonus) or 0)
  end
  return total
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

-- 礼物基础分（docs/54 §4.8，2026-10-06 1.4）：偏好档（爱40/喜20/厌-10）优先，
-- 未命中回落到既有 gifts 小分制（阶段 1 锚点逐字不动）。都不命中返回 nil = 不收礼。
local function gift_base_points(npc_id, item_id)
  local pref = data.gift_preference and data.gift_preference(npc_id, item_id)
  if pref then
    local pts = data.pref_points and data.pref_points[pref]
    if type(pts) == "number" then
      return pts
    end
  end
  return data.gift_points(npc_id, item_id)
end

-- 今天是该 NPC 生日吗（112 天循环，纯查 clock，不写状态）。
function M.is_birthday(npc_id)
  local def = type(npc_id) == "string" and npcs.get(npc_id) or nil
  if not def or type(def.birthday) ~= "number" then
    return false
  end
  local day = math.floor(tonumber(clock.now().day) or 1)
  local within = ((day - 1) % npcs.cycle_days) + 1
  return within == def.birthday
end

-- 送礼：NPC 存在且收该礼（偏好档或 gifts 命中，否则 not_giftable）、背包持有（否则
-- not_enough）、当日未送过（否则 gift_today，不扣物不加分）；成功 =
-- 消耗 1 件 + 加分（来源 gift，事件 = 物品 id）。
-- 生日 ×3（docs/54 §4.8）：只放大正向礼（爱/喜/小分正礼），厌礼负向不放大。
-- 返回 true,新分数。
function M.gift(npc_id, item_id)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return false, "unknown_npc"
  end
  local base = gift_base_points(npc_id, item_id)
  if type(base) ~= "number" then
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
  local step = base
  if step > 0 and M.is_birthday(npc_id) then
    step = step * (tonumber(data.birthday_gift_multiplier) or 3)
  end
  local ok, after = M.add(npc_id, step, "gift", item_id)
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
