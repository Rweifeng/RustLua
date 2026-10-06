-- systems/relationship_events.lua：《江湖有田》好感事件运行时（docs/54 §4.8，2026-10-06 1.4）。
--
-- 职责：
--   * 查询可触发事件：list_for(npc) 返回该 NPC「心数门槛已达 + 未 once」的事件列表。
--   * 触发：trigger(id) 复核门槛/未完成 → 标记 once → 一次性发奖励（关系分/金币/物品）。
--   * 生日标注：on_day_start 读 data.npcs.birthday_of(今天)，把今日寿星写进
--     state var "birthday_today"（随存档）；送礼 ×3 在 player/relationship.lua 内
--     直接读 clock，与本模块解耦。
--   * 存档：completed（事件完成表）随 capture/restore 持久；reset 清空。
--
-- 数据定义见 data/relationship_events.lua（纯数据 + validate_all）。
local state = require("core.state")
local data = require("data.relationship_events")
local npcs = require("data.npcs")
local relationship = require("player.relationship")

local M = {}

local completed = {} -- event_id → true（once 完成表，随存档持久）

-- 某 NPC 当前可触发的事件（心数门槛已达 + 未完成）。只读、稳定序（数据序）。
function M.list_for(npc_id)
  local out = {}
  for _, ev in ipairs(data.for_npc(npc_id)) do
    if not completed[ev.id] then
      local need = data.heart_points(ev.heart)
      local have = relationship.points(npc_id) or 0
      if have >= need then
        out[#out + 1] = ev
      end
    end
  end
  return out
end

-- 是否已完成某事件。
function M.done(event_id)
  return completed[event_id] == true
end

-- 触发事件：未知事件/门槛未达/已完成 → false,reason；成功 → true,{rewards=...}。
-- 发奖励走既有入口（relationship.add / state.money / inv.add），与对话/任务同链路。
function M.trigger(event_id)
  local ev = type(event_id) == "string" and data.get(event_id) or nil
  if not ev then
    return false, "unknown_event"
  end
  if completed[ev.id] then
    return false, "already_done"
  end
  local need = data.heart_points(ev.heart)
  local have = relationship.points(ev.npc) or 0
  if have < need then
    return false, "threshold_not_met"
  end
  completed[ev.id] = true
  local rewards = type(ev.rewards) == "table" and ev.rewards or {}
  if type(rewards.affinity) == "number" and rewards.affinity ~= 0 then
    relationship.add(ev.npc, rewards.affinity, "event", ev.id)
  end
  if type(rewards.gold) == "number" and rewards.gold ~= 0 then
    state.money = (tonumber(state.money) or 0) + rewards.gold
  end
  if type(rewards.item) == "string" and rewards.item ~= "" then
    inv.add(state.inventory, rewards.item, 1)
  end
  return true, { rewards = rewards }
end

-- 今日寿星（只读；on_day_start 刷新后写入 state var，读档随 clock 回来）。
function M.birthday_today()
  return state.get_var("birthday_today")
end

-- 每日挂点（main.lua on_day_start 接线，样例内显式调用断言）：按今天日期查寿星，
-- 写进 state var。纯日期推导，不发奖（生日 ×3 在送礼时即时生效）。
function M.on_day_start()
  local day = math.floor(tonumber(clock.now().day) or 1)
  local bday = npcs.birthday_of(day)
  local id = bday and bday.persist_id or nil
  state.set_var("birthday_today", id, "relationship_events", "birthday")
  return id
end

-- ---------- 存档 ----------

function M.capture()
  local done_data = {}
  for id, flag in pairs(completed) do
    if type(id) == "string" and flag == true then
      done_data[id] = true
    end
  end
  return { completed = done_data }
end

function M.restore(data_in)
  completed = {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.completed) == "table" then
    for id, flag in pairs(data_in.completed) do
      if type(id) == "string" and flag == true and data.get(id) then
        completed[id] = true
      end
    end
  end
end

function M.reset()
  completed = {}
end

-- ---------- 加载期校验（委托数据表；系统自身无独立常量需自检） ----------

function M.validate_all()
  return data.validate_all()
end

return M
