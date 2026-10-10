-- data/tutorial.lua：《江湖有田》新手引导步骤表（M19-02 纯数据）。
--
-- 新增/调整引导步骤只改本文件，不改 tutorial.lua 与 Rust 核心。
--
-- 字段：
--   version  数据版本（必须为 1）
--   id       步骤 id（唯一，写入 state.vars["tutorial_step"] 的事件来源）
--   prompt   HUD 提示区单行中文文案（≤ 约 40 个显示宽度）
--   detail   文档/调试用详细说明（HUD 不显示）
--   match    完成条件：
--     kind = "action"           state.last_action 精确匹配（value 为字符串或数组）
--     kind = "action_prefix"    state.last_action 前缀匹配（value 为字符串或数组）
--     kind = "day_start"        跨过一天（on_day_start）
--     kind = "enter_map"        进入指定地图（value = data/maps.lua 的键）
--     kind = "quest_accepted"   任务已接取（value = data/quests/index.lua 的任务 id）
--     kind = "quest_delivered"  任务已交付（同上）
--     kind = "finish"           终止标记：进入该步即视为引导完成（发奖）
--
-- 步骤顺序即玩家操作顺序；tutorial.lua 每次事件只推进至多一步（幂等）。
local M = {}

M.version = 1
M.title = "新手引导"
-- 完成全部步骤的一次性奖励（幂等：state.vars["tutorial_rewarded"] 锚点）。
M.reward = { item = "heal_pill", count = 1 }

M.steps = {
  {
    id = "till",
    prompt = "① 翻土：WASD 走到药田，面向空地按 E（锄头已装备）",
    detail = "山庄药田矩形 14..33 × 4..13；面向 DIRT 地块按 E 翻土，体力 -8。",
    match = { kind = "action", value = "till" }
  },
  {
    id = "plant",
    prompt = "② 播种：再按一次 E 播下灵芝菌种",
    detail = "已垦地块按 E 播种（默认选中灵芝菌种，可用 I 背包切换）；体力 -3。",
    match = { kind = "action_prefix", value = "plant_" }
  },
  {
    id = "water",
    prompt = "③ 浇水：按 I 打开背包，选「水壶」按 E 装备，再面向药田按 E",
    detail = "浇水需要 tool 槽装备水壶；也可用背包数字键选中后按 E 装备。体力 -4。",
    match = { kind = "action_prefix", value = { "water_", "rain_watered" } }
  },
  {
    id = "sleep",
    prompt = "④ 睡觉：按 T 睡到第二天（作物每天浇水才会成长）",
    detail = "T 触发 day.sleep → on_day_start：作物成长 +1 天、体力回满、自动存档。",
    match = { kind = "day_start" }
  },
  {
    id = "goto_village",
    prompt = "⑤ 出庄：走到山庄南侧石路出口按 E 前往云泽村",
    detail = "出口格 (16,1)，按 E 交互即切图；云泽村有药庐主人沈青萝与货摊。",
    match = { kind = "enter_map", value = "village" }
  },
  {
    id = "talk_npc",
    prompt = "⑥ 交谈：走到 NPC 面前按 E 打开对话",
    detail = "对话面板：数字键选择、Esc 结束；与沈青萝对话可接「为医馆采药」。",
    match = { kind = "action", value = "dialogue" }
  },
  {
    id = "accept_quest",
    prompt = "⑦ 接任务：在对话里选「愿为医馆采三株灵芝」",
    detail = "任务状态从 not_started 变为 in_progress；按 L 可打开任务日志查看进度。",
    match = { kind = "quest_accepted", value = "quest_herb" }
  },
  {
    id = "deliver",
    prompt = "⑧ 交付：攒 5 株灵芝（3 交付 + 2 制药），回药庐选「三株灵芝已备齐」",
    detail = "交付需 3 株灵芝，另留 2 株炼药。灵芝种植需 7 个有效成长日；不必等成熟，可去青云山道采集药草，毒草点需要装备锄头。",
    match = { kind = "quest_delivered", value = "quest_herb" }
  },
  {
    id = "craft",
    prompt = "⑨ 制作：按 B 开建造模式 → 按 Q 选到「药庐」→ F 放置 → 面向它按 E",
    detail = "建造模式默认选中的是字典序第一座（畜棚），必须按 Q 换到药庐；药庐 station=herb，"
      .. "炼「回复药」需 2 株灵芝、120 游戏分钟；到点再面向它按 E 收取。",
    match = { kind = "action_prefix", value = "craft_collect_" }
  },
  -- ---- 种植复习段（购种/备种→翻土→播种→浇水→入睡跨天→收获）----
  -- 本段在制作之后，不能作为 docs/54「首次种植完成 ≤5 分钟」的达标证据。
  -- 保留步骤 id/顺序以兼容 tutorial_step 数字索引存档；首局重排须另做迁移。
  -- 仅追加在既有链尾（终态 done 之前），既有步骤 id/顺序不动；新步骤只在玩家走完整条
  -- 既有链（craft 完成）后才出现，新档第 0 步仍是 till，不抢占既有 headless 样例。
  {
    id = "claim_seed",
    prompt = "备种：去云泽村货摊买萝卜种，再用 I 选种、E 备种",
    detail = "萝卜种不在新档背包中：货摊每日 08:00–18:00 售卖，4 文/粒。买种后在背包选种触发 select_seed_<seed>；萝卜需普通耕地，种植季节以作物表为准。",
    match = { kind = "action_prefix", value = "select_seed_" }
  },
  {
    id = "till_loop",
    prompt = "翻土：在普通耕地用锄头按 E（药田不能种萝卜）",
    detail = "耕地区 x24..33（避开温室 x28..33,y8..13）；面向 DIRT 按 E 翻土，锄头须装备，体力 -8。",
    match = { kind = "action", value = "till" }
  },
  {
    id = "plant_loop",
    prompt = "播种：面向刚翻好的地再按 E，播下萝卜种",
    detail = "已垦空地按 E 播种（读 state.selected_seed=seed_radish）；体力 -3。",
    match = { kind = "action_prefix", value = "plant_" }
  },
  {
    id = "water_loop",
    prompt = "浇水：切出水壶（I→选中水壶→E 装备），面向苗按 E 浇透",
    detail = "浇水需 tool 槽装备水壶；雨天自动浇（rain_watered）。体力 -4。",
    match = { kind = "action_prefix", value = { "water_", "rain_watered" } }
  },
  {
    id = "sleep_loop",
    prompt = "入睡：按 T 睡到第二天（萝卜需连浇两晚才成熟）",
    detail = "T 触发 day.sleep → on_day_start：作物按已浇水天数成长 +1、体力回满、自动存档。",
    match = { kind = "day_start" }
  },
  {
    id = "harvest_loop",
    prompt = "收获：再浇一次水、睡一觉，萝卜成熟后按 E 采收",
    detail = "萝卜 growth_days=2：跨天后若未熟，再浇水+入睡一次；成熟按 E 收获（harvest_<item>），首次种植闭环完成。",
    match = { kind = "action_prefix", value = "harvest_" }
  },
  {
    id = "done",
    prompt = "引导完成：江湖路远，慢慢来。",
    detail = "终止标记：进入此步即完成引导并发放一次性奖励（回复药 ×1）。",
    match = { kind = "finish" }
  }
}

local VALID_KIND = {
  action = true,
  action_prefix = true,
  day_start = true,
  enter_map = true,
  quest_accepted = true,
  quest_delivered = true,
  finish = true
}

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/tutorial.lua: version: unsupported version " .. tostring(M.version)
  end
  if type(M.steps) ~= "table" or #M.steps == 0 then
    issues[#issues + 1] = "data/tutorial.lua: steps: expected non-empty array"
    return #issues == 0, issues
  end

  local maps = require("data.maps")
  local quest = require("world.quest")
  local items = require("data.items")
  local tools = require("data.tools")

  local seen = {}
  local endings = 0
  for index, step in ipairs(M.steps) do
    local path = "data/tutorial.lua: steps[" .. tostring(index) .. "]"
    if type(step) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if type(step.id) ~= "string" or step.id == "" then
        issues[#issues + 1] = path .. ".id: required non-empty string"
      elseif seen[step.id] then
        issues[#issues + 1] = path .. ".id: duplicate step id " .. step.id
      else
        seen[step.id] = true
      end
      if type(step.prompt) ~= "string" or step.prompt == "" then
        issues[#issues + 1] = path .. ".prompt: required non-empty string"
      end
      local match = step.match
      if type(match) ~= "table" then
        issues[#issues + 1] = path .. ".match: expected object"
      elseif not VALID_KIND[match.kind] then
        issues[#issues + 1] = path .. ".match.kind: invalid kind " .. tostring(match.kind)
      else
        local kind = match.kind
        if kind == "action" or kind == "action_prefix" then
          local value = match.value
          local values = type(value) == "table" and value or { value }
          if #values == 0 then
            issues[#issues + 1] = path .. ".match.value: required for " .. kind
          end
          for position, entry in ipairs(values) do
            if type(entry) ~= "string" or entry == "" then
              issues[#issues + 1] = path .. ".match.value[" .. tostring(position) .. "]: expected non-empty string"
            end
          end
        elseif kind == "enter_map" then
          if type(match.value) ~= "string" or not maps.get(match.value) then
            issues[#issues + 1] = path .. ".match.value: unknown map " .. tostring(match.value)
          end
        elseif kind == "quest_accepted" or kind == "quest_delivered" then
          if type(match.value) ~= "string" or not quest.get(match.value) then
            issues[#issues + 1] = path .. ".match.value: unknown quest " .. tostring(match.value)
          end
        elseif kind == "finish" then
          endings = endings + 1
        end
      end
    end
  end

  if endings ~= 1 then
    issues[#issues + 1] = "data/tutorial.lua: 终止标记（match.kind = \"finish\"）必须恰好一个，当前 "
      .. tostring(endings)
  elseif M.steps[#M.steps].match == nil or M.steps[#M.steps].match.kind ~= "finish" then
    issues[#issues + 1] = "data/tutorial.lua: 终止标记必须是最后一步"
  end

  if M.reward ~= nil then
    if type(M.reward) ~= "table" then
      issues[#issues + 1] = "data/tutorial.lua: reward: expected object"
    else
      if items[M.reward.item] == nil and tools[M.reward.item] == nil then
        issues[#issues + 1] = "data/tutorial.lua: reward.item: unknown item " .. tostring(M.reward.item)
      end
      local count = tonumber(M.reward.count)
      if count == nil or count <= 0 or count ~= math.floor(count) then
        issues[#issues + 1] = "data/tutorial.lua: reward.count: expected positive integer"
      end
    end
  end

  return #issues == 0, issues
end

return M
