-- data/clinic.lua：《江湖有田》医馆经营与疫情事件 纯数据
-- （docs/54 阶段3「医馆经营与疫情事件」，2026-10-06 2.0）。
--
-- 两张表：
--   diseases   病种 → 处方（药材配方 + 治疗分钟 + 收费 + 声望）。
--   epidemics  疫情事件 → 季节/季内触发日/持续天数/关联病种/症状描述/
--              被动惩罚（村民好感下降、医馆收益乘子）与三档决策效果。
--
-- 运行期逻辑在 systems/clinic.lua；本表只放数值与文案，并自带 validate_all
-- （登记进 validate_data.lua，由 main.validate_content 与 --check 共用同一道门）。
--
-- 药材一律引用既有物品（data.items：lingzhi/herb/prunella/mung_bean/gouqi/reed/
-- chrysanthemum/ginseng/polygonum…），不新增药品类物品——阶段 3 不需要
-- data/items_stage3.lua（该扩展槽留待后续波次，pcall require 缺失即跳过）。
local M = {}

M.version = 1

-- 季节白名单（与 world/calendar.lua 的 SEASONS 同序）。
local SEASONS = { spring = true, summer = true, autumn = true, winter = true }

-- 病种：id → { name, symptoms, recipe={ {item,count}... }, minutes, fee, rep }
--   recipe  处方所需药材（既有物品 id + 数量），接诊时逐项扣减；
--   minutes 单次接诊耗时（分钟，预留 UI/排队用，当前接诊即时结算）；
--   fee     基础诊费（文），疫情期间按 epidemics.fee_mult 上浮；
--   rep     每次接诊给医馆声望的增量。
M.diseases = {
  -- 风寒：春日常见，轻剂即可。
  wind_cold = {
    name = "风寒",
    symptoms = "恶寒发热，鼻塞声重",
    recipe = { { item = "herb", count = 2 }, { item = "mint", count = 1 } },
    minutes = 20,
    fee = 25,
    rep = 1
  },
  -- 湿热：暑季缠绵，需清热化湿。
  damp_heat = {
    name = "湿热",
    symptoms = "身重困倦，午后潮热",
    recipe = { { item = "prunella", count = 2 }, { item = "mung_bean", count = 1 } },
    minutes = 30,
    fee = 40,
    rep = 2
  },
  -- 跌打：筋骨外伤，外敷活血。
  trauma = {
    name = "跌打",
    symptoms = "筋骨肿痛，屈伸不利",
    recipe = { { item = "gouqi", count = 1 }, { item = "reed", count = 2 } },
    minutes = 25,
    fee = 50,
    rep = 2
  },
  -- 疫病：疫气横行，须重剂解毒。
  plague = {
    name = "疫病",
    symptoms = "高热神昏，疫气蔓延",
    recipe = { { item = "lingzhi", count = 1 }, { item = "chrysanthemum", count = 1 } },
    minutes = 40,
    fee = 80,
    rep = 4
  },
  -- 内伤：虚损久疾，贵重药引缓补。
  internal_injury = {
    name = "内伤",
    symptoms = "气滞血瘀，久咳虚损",
    recipe = { { item = "ginseng", count = 1 }, { item = "polygonum", count = 1 } },
    minutes = 60,
    fee = 120,
    rep = 5
  }
}

-- 疫情事件：id → {
--   season, start_day   触发季节 + 季内第几天（与 calendar.day_in_season 对齐，
--                        纯日期映射，确定可复现——同日同档必触发，不掷随机）；
--   duration            持续天数（on_day_start 每日递减，归 0 自然消退）；
--   disease            本疫情主要病种（文案/诊断用）；
--   symptom            症状描述（HUD/图鉴文案）；
--   probability        设计概率（当前固定 1.0 = 到点必发；预留未来随机化）；
--   favor_penalty      疫情激活当日一次性扣的村民好感；
--   fee_mult           疫情期间接诊诊费乘子（夏疫 1.5 = 药材需求暴涨、收益上浮）；
--   herbs              「治疫」决策一次性消耗的药材；
--   treat              「治疫」效果：声望大涨 / 好感回补；
--   stockpile          「囤药」效果：一次性现金收益（囤积居奇）/ 好感小罚；
-- }
-- 三决策（systems/clinic.decide）：
--   treat      治疫：消耗 herbs → 声望+treat.rep、好感+treat.favor，疫情提前平息；
--   stockpile  囤药：不动药材，立即兑现 stockpile.income，好感 stockpile.favor（负）；
--   close      闭馆：期间停止接诊（treat 拒 clinic_closed），无收益亦无额外损失，
--              疫情自然消退后复馆。
M.epidemics = {
  -- 夏疫：docs/54 点名的夏季高温瘟疫。
  summer_plague = {
    name = "夏疫",
    season = "summer",
    start_day = 1,
    duration = 5,
    disease = "plague",
    symptom = "暑湿蒸腾，村中热病横行",
    probability = 1.0,
    favor_penalty = 5,
    fee_mult = 1.5,
    herbs = { { item = "lingzhi", count = 3 }, { item = "chrysanthemum", count = 2 } },
    treat = { rep = 15, favor = 12 },
    stockpile = { income = 80, favor = -6 }
  },
  autumn_fever = {
    name = "秋瘟",
    season = "autumn",
    start_day = 3,
    duration = 4,
    disease = "damp_heat",
    symptom = "秋燥伤肺，咳嗽者众",
    probability = 1.0,
    favor_penalty = 4,
    fee_mult = 1.3,
    herbs = { { item = "prunella", count = 3 }, { item = "herb", count = 2 } },
    treat = { rep = 12, favor = 10 },
    stockpile = { income = 60, favor = -5 }
  },
  winter_cold = {
    name = "冬瘟",
    season = "winter",
    start_day = 2,
    duration = 6,
    disease = "wind_cold",
    symptom = "寒风刺骨，冻倒一片",
    probability = 1.0,
    favor_penalty = 6,
    fee_mult = 1.2,
    herbs = { { item = "herb", count = 4 }, { item = "mint", count = 2 } },
    treat = { rep = 14, favor = 11 },
    stockpile = { income = 70, favor = -6 }
  }
}

function M.disease(id)
  return M.diseases[id]
end

function M.epidemic(id)
  return M.epidemics[id]
end

-- 病种 id 稳定排序（UI/遍历确定性）。
function M.ids()
  local ids = {}
  for id in pairs(M.diseases) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

function M.epidemic_ids()
  local ids = {}
  for id in pairs(M.epidemics) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

-- 季节 + 季内第几天 → 该日该触发的疫情条目（无则 nil）。纯映射、不掷随机。
function M.trigger_for(season, day_in_season)
  for id, def in pairs(M.epidemics) do
    if def.season == season and def.start_day == day_in_season then
      return id, def
    end
  end
  return nil
end

-- ---------- 加载期校验（登记进 validate_data.lua） ----------
function M.validate_all()
  local issues = {}
  local items = require("data.items")
  local tools = require("data.tools")
  local function item_exists(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/clinic.lua: version: unsupported " .. tostring(M.version)
  end
  -- 病种
  for id, def in pairs(M.diseases) do
    local path = "data/clinic.lua:diseases." .. tostring(id)
    if type(def) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if type(def.name) ~= "string" or def.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if type(def.recipe) ~= "table" or #def.recipe == 0 then
        issues[#issues + 1] = path .. ".recipe: expected non-empty array"
      else
        for index, cost in ipairs(def.recipe) do
          local cpath = path .. ".recipe[" .. tostring(index) .. "]"
          if type(cost) ~= "table" or not item_exists(cost.item) then
            issues[#issues + 1] = cpath .. ": unknown item " .. tostring(cost and cost.item)
          elseif type(cost.count) ~= "number" or cost.count <= 0 or cost.count ~= math.floor(cost.count) then
            issues[#issues + 1] = cpath .. ".count: expected positive integer"
          end
        end
      end
      for _, field in ipairs({ "minutes", "fee", "rep" }) do
        local value = def[field]
        if type(value) ~= "number" or value < 0 then
          issues[#issues + 1] = path .. "." .. field .. ": expected non-negative number"
        end
      end
    end
  end
  -- 疫情
  for id, def in pairs(M.epidemics) do
    local path = "data/clinic.lua:epidemics." .. tostring(id)
    if type(def) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if not SEASONS[def.season] then
        issues[#issues + 1] = path .. ".season: invalid season " .. tostring(def.season)
      end
      if type(def.start_day) ~= "number" or def.start_day < 1 or def.start_day ~= math.floor(def.start_day) then
        issues[#issues + 1] = path .. ".start_day: expected positive integer"
      end
      if type(def.duration) ~= "number" or def.duration < 1 or def.duration ~= math.floor(def.duration) then
        issues[#issues + 1] = path .. ".duration: expected positive integer"
      end
      if not M.diseases[def.disease] then
        issues[#issues + 1] = path .. ".disease: unknown disease " .. tostring(def.disease)
      end
      if type(def.fee_mult) ~= "number" or def.fee_mult <= 0 then
        issues[#issues + 1] = path .. ".fee_mult: expected positive number"
      end
      for index, cost in ipairs(def.herbs or {}) do
        local cpath = path .. ".herbs[" .. tostring(index) .. "]"
        if type(cost) ~= "table" or not item_exists(cost.item) then
          issues[#issues + 1] = cpath .. ": unknown item " .. tostring(cost and cost.item)
        elseif type(cost.count) ~= "number" or cost.count <= 0 or cost.count ~= math.floor(cost.count) then
          issues[#issues + 1] = cpath .. ".count: expected positive integer"
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
