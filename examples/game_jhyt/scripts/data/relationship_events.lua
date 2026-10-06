-- data/relationship_events.lua：《江湖有田》好感事件注册表（docs/54 §4.8，2026-10-06 1.4，纯数据）。
--
-- 与 data/npcs.lua / data/relationships.lua 同约定：新增好感事件 = 本文件加一条记录，
-- 无需改动 systems/relationship_events.lua / Rust 核心。
--
-- 字段：
--   id       稳定事件 id（全局唯一，once 完成表的键，随存档持久）
--   npc      触发该事件的 NPC（引用 data/npcs.lua 的 persist_id）
--   heart    门槛心数（2|5|8 → 关系分 heart*10：20 / 50 / 80）
--   rewards  触发后一次性发放：{ affinity=关系分增量, gold=金币, item=物品 id }（均可缺省）
--   once     true = 一生只触发一次（默认 true；完成后不再出现在可触发列表）
--
-- 深交 6 位（沈青萝/顾长风/苏晚晴/玄鹤/林小满/韩铁山）每人 4–5 个（2/5/8 心分布），
-- 其余 10 人每人 1–2 个，合计 43（≥30）。触发判定与发放在 systems/relationship_events.lua。
local M = {}

M.version = 1

-- 心数 → 所需关系分（0..100）。2 心=20、5 心=50、8 心=80。
function M.heart_points(heart)
  local h = math.floor(tonumber(heart) or 0)
  return h * 10
end

M.events = {
  -- ============ 深交 6 位 ============
  -- 沈青萝（医馆掌柜）
  { id = "ev_shen_1", npc = "npc_shen_qingluo", heart = 2, rewards = { affinity = 10, gold = 30 }, once = true },
  { id = "ev_shen_2", npc = "npc_shen_qingluo", heart = 2, rewards = { affinity = 10, item = "herb" }, once = true },
  { id = "ev_shen_3", npc = "npc_shen_qingluo", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_shen_4", npc = "npc_shen_qingluo", heart = 5, rewards = { affinity = 15, item = "heal_pill" }, once = true },
  { id = "ev_shen_5", npc = "npc_shen_qingluo", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  -- 顾长风（山庄掌门）
  { id = "ev_gu_1", npc = "npc_gu_changfeng", heart = 2, rewards = { affinity = 10, gold = 30 }, once = true },
  { id = "ev_gu_2", npc = "npc_gu_changfeng", heart = 2, rewards = { affinity = 10, item = "herb_tonic" }, once = true },
  { id = "ev_gu_3", npc = "npc_gu_changfeng", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_gu_4", npc = "npc_gu_changfeng", heart = 5, rewards = { affinity = 15, item = "iron_ingot" }, once = true },
  { id = "ev_gu_5", npc = "npc_gu_changfeng", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  -- 苏晚晴
  { id = "ev_su_1", npc = "npc_su_wanqing", heart = 2, rewards = { affinity = 10, gold = 30 }, once = true },
  { id = "ev_su_2", npc = "npc_su_wanqing", heart = 5, rewards = { affinity = 15, item = "chrysanthemum" }, once = true },
  { id = "ev_su_3", npc = "npc_su_wanqing", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_su_4", npc = "npc_su_wanqing", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  -- 玄鹤
  { id = "ev_xuanhe_1", npc = "npc_xuanhe", heart = 2, rewards = { affinity = 10, item = "tea_brew" }, once = true },
  { id = "ev_xuanhe_2", npc = "npc_xuanhe", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_xuanhe_3", npc = "npc_xuanhe", heart = 5, rewards = { affinity = 15, item = "lotus_seed" }, once = true },
  { id = "ev_xuanhe_4", npc = "npc_xuanhe", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  -- 林小满
  { id = "ev_lin_1", npc = "npc_lin_xiaoman", heart = 2, rewards = { affinity = 10, item = "peach" }, once = true },
  { id = "ev_lin_2", npc = "npc_lin_xiaoman", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_lin_3", npc = "npc_lin_xiaoman", heart = 5, rewards = { affinity = 15, item = "mume" }, once = true },
  { id = "ev_lin_4", npc = "npc_lin_xiaoman", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  -- 韩铁山
  { id = "ev_han_1", npc = "npc_han_tieshan", heart = 2, rewards = { affinity = 10, item = "iron_ore" }, once = true },
  { id = "ev_han_2", npc = "npc_han_tieshan", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_han_3", npc = "npc_han_tieshan", heart = 8, rewards = { affinity = 20, item = "iron_ingot" }, once = true },
  { id = "ev_han_4", npc = "npc_han_tieshan", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },

  -- ============ 其余 10 人 ============
  { id = "ev_zhao_1", npc = "npc_zhao_tiesuanpan", heart = 5, rewards = { affinity = 10, gold = 150 }, once = true },
  { id = "ev_aqi_1", npc = "npc_a_qi", heart = 2, rewards = { affinity = 10, item = "rice_ball" }, once = true },
  { id = "ev_aqi_2", npc = "npc_a_qi", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true },
  { id = "ev_situ_1", npc = "npc_situ_kui", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  { id = "ev_lu_1", npc = "npc_lu_guizang", heart = 2, rewards = { affinity = 10, item = "tea_brew" }, once = true },
  { id = "ev_lu_2", npc = "npc_lu_guizang", heart = 5, rewards = { affinity = 15, gold = 100 }, once = true },
  -- 阶段 2 新增 6 人
  { id = "ev_niu_1", npc = "npc_niu_dazhuang", heart = 2, rewards = { affinity = 10, gold = 50 }, once = true },
  { id = "ev_niu_2", npc = "npc_niu_dazhuang", heart = 5, rewards = { affinity = 15, item = "rice" }, once = true },
  { id = "ev_ayao_1", npc = "npc_a_yao", heart = 2, rewards = { affinity = 10, item = "herb" }, once = true },
  { id = "ev_ayao_2", npc = "npc_a_yao", heart = 5, rewards = { affinity = 15, gold = 60 }, once = true },
  { id = "ev_shi_1", npc = "npc_shi_gandang", heart = 2, rewards = { affinity = 10, gold = 50, item = "rice" }, once = true },
  { id = "ev_shi_2", npc = "npc_shi_gandang", heart = 5, rewards = { affinity = 15, item = "iron_ingot" }, once = true },
  { id = "ev_qdb_1", npc = "npc_qianduobao", heart = 2, rewards = { affinity = 10, gold = 100 }, once = true },
  { id = "ev_qdb_2", npc = "npc_qianduobao", heart = 5, rewards = { affinity = 15, item = "ginkgo_fruit" }, once = true },
  { id = "ev_gm_1", npc = "npc_guimian", heart = 8, rewards = { affinity = 20, gold = 200 }, once = true },
  { id = "ev_lqf_1", npc = "npc_lu_qingfeng", heart = 2, rewards = { affinity = 10, item = "boar_meat" }, once = true },
  { id = "ev_lqf_2", npc = "npc_lu_qingfeng", heart = 5, rewards = { affinity = 15, gold = 80 }, once = true }
}

function M.get(id)
  for _, ev in ipairs(M.events) do
    if ev.id == id then
      return ev
    end
  end
  return nil
end

-- 某 NPC 的全部事件（只读）。
function M.for_npc(npc_id)
  local out = {}
  for _, ev in ipairs(M.events) do
    if ev.npc == npc_id then
      out[#out + 1] = ev
    end
  end
  return out
end

function M.validate_all()
  local issues = {}
  local npcs = require("data.npcs")
  local items = require("data.items")
  local tools = require("data.tools")
  local seen = {}
  local function is_integer(v)
    return type(v) == "number" and v == math.floor(v)
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/relationship_events.lua:version: unsupported version " .. tostring(M.version)
  end
  if type(M.events) ~= "table" or #M.events < 30 then
    issues[#issues + 1] = "data/relationship_events.lua:events: expected >= 30 entries"
  else
    for index, ev in ipairs(M.events) do
      local path = "data/relationship_events.lua:events[" .. tostring(index) .. "]"
      if type(ev) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(ev.id) ~= "string" or ev.id == "" then
          issues[#issues + 1] = path .. ".id: required non-empty string"
        elseif seen[ev.id] then
          issues[#issues + 1] = path .. ".id: duplicate event id " .. ev.id
        else
          seen[ev.id] = true
        end
        if not npcs.get(ev.npc) then
          issues[#issues + 1] = path .. ".npc: unknown npc " .. tostring(ev.npc)
        end
        if ev.heart ~= 2 and ev.heart ~= 5 and ev.heart ~= 8 then
          issues[#issues + 1] = path .. ".heart: expected 2/5/8, got " .. tostring(ev.heart)
        end
        if type(ev.rewards) ~= "table" then
          issues[#issues + 1] = path .. ".rewards: expected table (may be empty)"
        else
          if ev.rewards.affinity ~= nil and (not is_integer(ev.rewards.affinity) or ev.rewards.affinity <= 0) then
            issues[#issues + 1] = path .. ".rewards.affinity: expected positive integer"
          end
          if ev.rewards.gold ~= nil and (not is_integer(ev.rewards.gold) or ev.rewards.gold < 0) then
            issues[#issues + 1] = path .. ".rewards.gold: expected nonnegative integer"
          end
          if ev.rewards.item ~= nil then
            if type(ev.rewards.item) ~= "string" then
              issues[#issues + 1] = path .. ".rewards.item: expected string id"
            elseif items[ev.rewards.item] == nil and tools[ev.rewards.item] == nil then
              issues[#issues + 1] = path .. ".rewards.item: unknown item " .. tostring(ev.rewards.item)
            end
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
