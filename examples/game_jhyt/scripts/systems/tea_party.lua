-- systems/tea_party.lua：《江湖有田》山庄茶会社交运行时
-- （docs/54 §7.5 / docs/33 §8.3，2026-10-06 2.3）。
--
-- 职责（深交系统的「聚会化出口」）：
--   * 邀请 invite(npc)：按好感门槛（data.min_hearts 心）+ 冷却 + 名单规模筛人；
--     成功写 state.last_action = "tea_invite_<npc>"。
--   * 筹备 prepare(tea_item)：校验茶品白名单与背包持有，消耗 1 件。
--   * 开场 begin(topic_a, topic_b)：复核客人 2..4 人、已备茶、两个合法且不同话题。
--   * 结算 settle()：逐客按「基础好感 + 命中话题系数」走 relationship.add 入账
--     （正增量统一乘当日社交乘子，与送礼/对话同一条入口）；有阵营的客人给其阵营
--     加声望（relationship.add_reputation，不乘社交乘子）；话题命中记一条情报
--     （intel，解锁对话/成就条件）；写 state.last_action = "tea_party_<id>"。
--   * 冷却 cooldown：茶会当天记 cooldown[npc]=day，N 天内不可再邀；按日期懒推导
--     （与 gifted_day / tea_day 同族），on_day_start 只派生「今日解除冷却」名单。
--   * 存档：cooldown / intel / parties / last_id 随 capture/restore 持久；reset 清空。
--
-- 数据定义见 data/tea_party.lua（纯数据 + validate_all）。
local state = require("core.state")
local data = require("data.tea_party")
local npcs = require("data.npcs")
local relationship = require("player.relationship")

local M = {}

-- ---- 运行态（随存档持久的 = cooldown/intel/parties/last_id；瞬态 = 邀请/筹备会话）----
local invited = {}    -- npc_id → true（待入席名单）
local guest_order = {} -- npc_id 数组（邀请顺序稳定）
local prepared_tea = nil -- 已备茶品 item id
local chosen = {}     -- 本场两个话题 id
local in_session = false
local cooldown = {}    -- npc_id → 最后一次办茶会的游戏日
local intel = {}      -- "npc_id|topic_id" → true（已解锁情报）
local parties = 0     -- 已办茶会场次
local last_id = nil   -- 最近一场茶会 id

local function today()
  return math.floor(tonumber(clock.now().day) or 1)
end

-- 该 NPC 是否在冷却期内（day - 上次茶会 < cooldown_days）。
local function cooling(npc_id, day)
  local last = cooldown[npc_id]
  if type(last) ~= "number" then
    return false
  end
  return (day or today()) - last < data.cooldown_days
end

-- 只读：当前可邀名单（门槛 + 冷却 + 已在席筛完，按 persist_id 稳定序）。
function M.list_eligible()
  local out = {}
  for _, def in ipairs(npcs.list()) do
    local id = def.persist_id
    local ok = M.eligible(id)
    if ok then
      out[#out + 1] = id
    end
  end
  return out
end

-- 只读：某 NPC 当前是否可邀（true / false, reason）。
function M.eligible(npc_id)
  if type(npc_id) ~= "string" or not npcs.get(npc_id) then
    return false, "unknown_npc"
  end
  local need = data.heart_points(data.min_hearts)
  local have = relationship.points(npc_id) or 0
  if have < need then
    return false, "threshold_not_met"
  end
  if cooling(npc_id) then
    return false, "cooldown"
  end
  if invited[npc_id] then
    return false, "already_invited"
  end
  return true
end

-- 邀请：门槛/冷却/名单满校验；成功入席并写 last_action。
function M.invite(npc_id)
  if in_session then
    return false, "in_session"
  end
  local ok, reason = M.eligible(npc_id)
  if not ok then
    return false, reason
  end
  if #guest_order >= data.max_guests then
    return false, "list_full"
  end
  invited[npc_id] = true
  guest_order[#guest_order + 1] = npc_id
  state.last_action = "tea_invite_" .. npc_id
  return true, npc_id
end

-- 当前待入席名单（只读副本，稳定序）。
function M.guests()
  local out = {}
  for index, id in ipairs(guest_order) do
    out[index] = id
  end
  return out
end

-- 已备茶品（item id 或 nil）。
function M.prepared_tea()
  return prepared_tea
end

-- 筹备：校验白名单与背包持有，消耗 1 件。已备过茶不重复消耗（already_prepared）。
function M.prepare(tea_item_id)
  if in_session then
    return false, "in_session"
  end
  if prepared_tea ~= nil then
    return false, "already_prepared"
  end
  if not data.is_tea_item(tea_item_id) then
    return false, "unknown_tea"
  end
  if inv.count(state.inventory, tea_item_id) <= 0 then
    return false, "not_enough"
  end
  inv.remove(state.inventory, tea_item_id, 1)
  prepared_tea = tea_item_id
  return true, tea_item_id
end

-- 话题列表（只读，数据序）。
function M.topics()
  return data.topics
end

-- 开场：客人 2..4、已备茶、两个话题合法且不同。
function M.begin(topic_a, topic_b)
  if #guest_order < data.min_guests then
    return false, "too_few_guests"
  end
  if #guest_order > data.max_guests then
    return false, "too_many_guests"
  end
  if prepared_tea == nil then
    return false, "no_tea"
  end
  if type(topic_a) ~= "string" or type(topic_b) ~= "string"
    or not data.get_topic(topic_a) or not data.get_topic(topic_b) then
    return false, "bad_topic"
  end
  if topic_a == topic_b then
    return false, "duplicate_topic"
  end
  chosen = { topic_a, topic_b }
  in_session = true
  return true, chosen
end

-- 情报是否已解锁（npc_id|topic_id）。
function M.intel_known(npc_id, topic_id)
  if type(npc_id) ~= "string" or type(topic_id) ~= "string" then
    return false
  end
  return intel[npc_id .. "|" .. topic_id] == true
end

-- 已办茶会场次。
function M.party_count()
  return parties
end

-- 最近一场茶会 id。
function M.last_party_id()
  return last_id
end

-- 结算：逐客入账好感/声望、记情报与冷却、写 last_action；瞬态会话清空。
function M.settle()
  if not in_session then
    return false, "not_in_session"
  end
  local id = "tp_" .. tostring(parties + 1)
  local day = today()
  local tea_served = prepared_tea
  local affinity_out = {}
  local reputation_out = {}
  local intel_count = 0
  local seen_factions = {}

  for _, npc_id in ipairs(guest_order) do
    local def = npcs.get(npc_id)
    local faction = def and def.faction or nil
    -- 基础好感 + 命中话题系数（两个话题都算）。
    local total = data.outcome.base_affinity
    for _, topic_id in ipairs(chosen) do
      local topic = data.get_topic(topic_id)
      if topic and data.topic_matches(topic, npc_id, faction) then
        total = total + (tonumber(topic.affinity) or 0)
        intel[npc_id .. "|" .. topic_id] = true
        intel_count = intel_count + 1
      end
    end
    local _, after = relationship.add(npc_id, total, "tea_party", id)
    affinity_out[npc_id] = after
    -- 声望：同一阵营多位客人各贡献一份（按阵营累加，不重复加同一人）。
    if faction then
      local _, rep = relationship.add_reputation(faction, data.outcome.per_guest_reputation, "tea_party", id)
      reputation_out[faction] = rep
      seen_factions[faction] = true
    end
    cooldown[npc_id] = day
  end

  -- 清空瞬态会话（邀请名单/茶/话题），持久态（cooldown/intel）保留。
  invited = {}
  guest_order = {}
  prepared_tea = nil
  chosen = {}
  in_session = false
  parties = parties + 1
  last_id = id
  state.last_action = "tea_party_" .. id

  return true, {
    id = id,
    day = day,
    tea = tea_served,
    guests = affinity_out,
    reputation = reputation_out,
    intel_count = intel_count
  }
end

-- 每日挂点（main.lua on_day_start 接线，样例内显式调用断言）：派生「今日解除冷却」
-- 的 NPC 名单（按 id 稳定序），并把人数写进 state var（随存档）。纯日期推导，不发奖。
function M.on_day_start()
  local day = today()
  local ready = {}
  for npc_id, last in pairs(cooldown) do
    if type(last) == "number" and (day - last) >= data.cooldown_days then
      ready[#ready + 1] = npc_id
    end
  end
  table.sort(ready)
  state.set_var("tea_cooldown_ready", #ready, "tea_party", "cooldown")
  return ready
end

-- ---------- 存档 ----------

function M.capture()
  local cooldown_data = {}
  for npc_id, day in pairs(cooldown) do
    if type(npc_id) == "string" and type(day) == "number" then
      cooldown_data[npc_id] = day
    end
  end
  local intel_data = {}
  for key, flag in pairs(intel) do
    if type(key) == "string" and flag == true then
      intel_data[key] = true
    end
  end
  return {
    cooldown = cooldown_data,
    intel = intel_data,
    parties = parties,
    last_id = last_id
  }
end

function M.restore(data_in)
  cooldown = {}
  intel = {}
  parties = 0
  last_id = nil
  -- 读档同时丢弃未完成的临时会话（读档点不该残留半程茶会）。
  invited = {}
  guest_order = {}
  prepared_tea = nil
  chosen = {}
  in_session = false
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.cooldown) == "table" then
    for npc_id, day in pairs(data_in.cooldown) do
      if npcs.get(npc_id) and type(day) == "number" then
        cooldown[npc_id] = day
      end
    end
  end
  if type(data_in.intel) == "table" then
    for key, flag in pairs(data_in.intel) do
      if type(key) == "string" and flag == true then
        -- 只保留当前话题池里真实存在的情报键（话题删了则对应情报随之失效）。
        local npc_id, topic_id = key:match("^(.-)|(.-)$")
        if npc_id and topic_id and npcs.get(npc_id) and data.get_topic(topic_id) then
          intel[key] = true
        end
      end
    end
  end
  if type(data_in.parties) == "number" then
    parties = math.max(0, math.floor(data_in.parties))
  end
  if type(data_in.last_id) == "string" then
    last_id = data_in.last_id
  end
end

function M.reset()
  invited = {}
  guest_order = {}
  prepared_tea = nil
  chosen = {}
  in_session = false
  cooldown = {}
  intel = {}
  parties = 0
  last_id = nil
end

-- ---------- 加载期校验（委托数据表；系统自身无独立常量需自检） ----------

function M.validate_all()
  return data.validate_all()
end

return M
