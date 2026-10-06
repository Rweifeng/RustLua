-- systems/questboard.lua：《江湖有田》任务板 3 层运行时（docs/54 §4.7，2026-10-06 1.6）。
--
-- 与 world/quest.lua 的关系：**独立闭环**——任务板自己持有"当前挂出的单子 + 接单表"，
-- 不接入 quest 五态状态机；奖励发放走既有通路（state.money / inv.add /
-- relationship.add_reputation），与 shipping/goals 共存不冲突。
--
-- 板子状态（capture/restore 随存档持久；reset 清空）：
--   board[layer]      当前挂出的单子数组：{ tpl=<模板id>, accepted=<bool>, done=<bool> }
--   seeded_day[layer]  该层最近一次刷新落在第几天（刷新锚点）
--
-- 刷新规则（on_day_start，main.lua 日结算挂点；headless 样例显式调用）：
--   daily     每个游戏日重抽 3 单；
--   weekly    day % 7 == 0（第 7/14/21... 天）重抽 2 单；
--   quarterly (day-1) % 28 == 0（第 1/29/57/85... 天，每季 28 天）重抽 1 单。
-- 重抽即整体替换该层挂单：昨天已交/已接未交的单子一律不残留（与"每天换榜"语义一致）。
--
-- 选单确定性：按 (day, layer) 哈希取起点、在候选池里顺序步进去重——同一天同一层
-- 永远抽到同一组（headless 可断言数量合法、不断言具体哪几张）。
local state = require("core.state")
-- inv / clock 是引擎注入的全局模块（与 systems/shipping.lua 同一用法）。
local inv = inv
local items = require("data.items")
local tools = require("data.tools")
local relationship = require("player.relationship")
local relationships_data = require("data.relationships")
local audio = require("core.audio")

local M = {}

-- 阶段 2 分系统扩展物品（与 systems/shipping.lua 同一批可选模块）：纯追加回退，
-- 既有物品仍优先命中 data.items/data.tools。任务板模板目前只引用基础物品，
-- 这里照抄同一套兜底以便将来扩展模板时不用改本文件。
local ITEM_EXT_MODULES = {
  "data.items_animal",
  "data.items_fish",
  "data.items_mine",
  "data.items_cooking",
  "data.items_stage3",
}

local function board_data()
  local ok, mod = pcall(require, "data.questboard")
  if ok and type(mod) == "table" then
    return mod
  end
  return nil
end

-- 运行时状态。
local board = { daily = {}, weekly = {}, quarterly = {} }
local seeded_day = { daily = 0, weekly = 0, quarterly = 0 }

local function item_def(item_id)
  if type(item_id) ~= "string" then
    return nil
  end
  if items[item_id] then
    return items[item_id]
  end
  if tools[item_id] then
    return tools[item_id]
  end
  for _, module_name in ipairs(ITEM_EXT_MODULES) do
    local ok, ext = pcall(require, module_name)
    if ok and type(ext) == "table" and ext.items and ext.items[item_id] then
      return ext.items[item_id]
    end
  end
  return nil
end

local function valid_layer(layer)
  local data = board_data()
  if not data then
    return false
  end
  for _, name in ipairs(data.LAYERS) do
    if name == layer then
      return true
    end
  end
  return false
end

-- 确定性选单：从 pool 里挑 count 张，按 (day, layer_seed) 取起点顺序步进去重。
local function pick_pool(pool, count, day, layer_seed)
  local picked = {}
  if type(pool) ~= "table" or #pool == 0 or count <= 0 then
    return picked
  end
  local used = {}
  local n = #pool
  local cursor = ((day * 13 + layer_seed * 17) % n) + 1
  while #picked < count and #picked < n do
    local idx = ((cursor - 1) % n) + 1
    if not used[idx] then
      used[idx] = true
      picked[#picked + 1] = pool[idx]
    end
    cursor = cursor + 1
  end
  return picked
end

local LAYER_SEED = { daily = 1, weekly = 101, quarterly = 211 }

-- 重抽某一层挂单（整体替换）。day 为落榜的游戏日。
local function redraw(layer, day)
  local data = board_data()
  if not data or not valid_layer(layer) then
    return
  end
  local rule = data.layer_rules[layer]
  local count = rule and tonumber(rule.offers) or 0
  local pool = data.pool(layer, day)
  local chosen = pick_pool(pool, count, day, LAYER_SEED[layer] or 1)
  local next_board = {}
  for _, tpl in ipairs(chosen) do
    next_board[#next_board + 1] = { tpl = tpl.id, accepted = false, done = false }
  end
  board[layer] = next_board
  seeded_day[layer] = day
end

-- ---------- 只读查询 ----------

-- 某层当前挂单视图（验收/UI 用，调用方只读）。
-- 每项：{ id, tpl, title, accepted, done, item, count }。
function M.offers(layer)
  local data = board_data()
  local out = {}
  if not data or not valid_layer(layer) then
    return out
  end
  for index, entry in ipairs(board[layer] or {}) do
    local tpl = data.template(entry.tpl)
    out[index] = {
      id = layer .. ":" .. tostring(seeded_day[layer]) .. ":" .. tostring(entry.tpl),
      tpl = entry.tpl,
      title = tpl and tpl.title or entry.tpl,
      accepted = entry.accepted,
      done = entry.done,
      item = tpl and tpl.objective and tpl.objective.item or nil,
      count = tpl and tpl.objective and tpl.objective.count or 0,
    }
  end
  return out
end

-- 某层挂单数量（验收用）。
function M.count(layer)
  return #(M.offers(layer))
end

-- 取某层第 index 张挂单的模板定义。
local function offer_entry(layer, index)
  if not valid_layer(layer) then
    return nil
  end
  local list = board[layer]
  if type(index) ~= "number" or index < 1 or index > #list then
    return nil
  end
  return list[index]
end

-- ---------- 接单 / 交单 ----------

-- 接下某层第 index 张挂单。返回 true,"ok" 或 false,reason。
-- 成功写 last_action=questboard_accept；失败不写 last_action（与商店/出货同语义）。
function M.accept(layer, index)
  local entry = offer_entry(layer, index)
  if not entry then
    return false, "no_such_offer"
  end
  if entry.accepted then
    return false, "already_accepted"
  end
  entry.accepted = true
  state.last_action = "questboard_accept"
  audio.ui("ui_confirm")
  return true, "ok"
end

-- 交付某层第 index 张挂单：必须已接、目标物齐 → 消耗 → 发奖 → 标记完成。
-- 成功写 last_action=questboard_deliver；失败不写。奖励幂等（done 后再交返回 already_done）。
function M.deliver(layer, index)
  local data = board_data()
  local entry = offer_entry(layer, index)
  if not data or not entry then
    return false, "no_such_offer"
  end
  local tpl = data.template(entry.tpl)
  if not tpl then
    return false, "unknown_template"
  end
  if not entry.accepted then
    return false, "not_accepted"
  end
  if entry.done then
    return false, "already_done"
  end
  local objective = tpl.objective or {}
  local need = math.floor(tonumber(objective.count) or 0)
  if objective.kind == nil and type(objective.item) == "string" then
    -- 物品类目标：持有足够才放行（与 order_hunt 同一口径）。
    if (tonumber(inv.count(state.inventory, objective.item)) or 0) < need then
      return false, "objectives_unmet"
    end
  else
    return false, "bad_objective"
  end

  -- 落定完成态先于消耗/发奖：重复交付在入口即被拦，奖励绝不重复发（硬风险②）。
  entry.done = true
  inv.remove(state.inventory, objective.item, need)

  local reward = tpl.reward or {}
  local money = tonumber(reward.money)
  if money then
    state.money = (tonumber(state.money) or 0) + money
  end
  if type(reward.item) == "table" and type(reward.item.id) == "string" then
    local gain = math.floor(tonumber(reward.item.count) or 0)
    if gain > 0 then
      inv.add(state.inventory, reward.item.id, gain)
    end
  end
  if type(reward.rep) == "table" and type(reward.rep.faction) == "string" then
    local amount = tonumber(reward.rep.amount)
    if amount then
      relationship.add_reputation(reward.rep.faction, amount, "questboard", tpl.id)
    end
  end

  state.last_action = "questboard_deliver"
  audio.sfx("sfx_quest")
  return true, "ok"
end

-- ---------- 刷新挂点 ----------

-- 日结算挂点（main.on_day_start 调用；headless 样例显式调用）。
-- 按当前 clock 天决定哪些层到期重抽。幂等：同一天重复调用结果一致（重抽同组）。
function M.on_day_start()
  local day = math.floor(tonumber(clock.now().day) or 1)
  redraw("daily", day)
  if day % 7 == 0 then
    redraw("weekly", day)
  end
  if (day - 1) % 28 == 0 then
    redraw("quarterly", day)
  end
  return true
end

-- ---------- 存档 ----------

function M.capture()
  local board_data_out = {}
  for layer, list in pairs(board) do
    local entries = {}
    for index, entry in ipairs(list) do
      entries[index] = { tpl = entry.tpl, accepted = entry.accepted and true or false, done = entry.done and true or false }
    end
    board_data_out[layer] = entries
  end
  local seeded = {}
  for layer, day in pairs(seeded_day) do
    seeded[layer] = math.floor(tonumber(day) or 0)
  end
  return { board = board_data_out, seeded_day = seeded }
end

function M.restore(data_in)
  board = { daily = {}, weekly = {}, quarterly = {} }
  seeded_day = { daily = 0, weekly = 0, quarterly = 0 }
  if type(data_in) ~= "table" then
    return
  end
  local data = board_data()
  if type(data_in.board) == "table" then
    for layer, entries in pairs(data_in.board) do
      if valid_layer(layer) and type(entries) == "table" then
        local restored = {}
        for _, entry in ipairs(entries) do
          if type(entry) == "table" and type(entry.tpl) == "string" and data and data.template(entry.tpl) then
            restored[#restored + 1] = {
              tpl = entry.tpl,
              accepted = entry.accepted == true,
              done = entry.done == true,
            }
          end
        end
        board[layer] = restored
      end
    end
  end
  if type(data_in.seeded_day) == "table" then
    for layer, day in pairs(data_in.seeded_day) do
      if valid_layer(layer) then
        seeded_day[layer] = math.floor(tonumber(day) or 0)
      end
    end
  end
end

function M.reset()
  board = { daily = {}, weekly = {}, quarterly = {} }
  seeded_day = { daily = 0, weekly = 0, quarterly = 0 }
end

-- ---------- 加载期校验 ----------

-- 返回 (true, {}) 通过；否则 (false, issues)。登记在 validate_data.lua（补丁文本）。
function M.validate_all()
  local issues = {}
  local data = board_data()
  if not data then
    issues[#issues + 1] = "data/questboard.lua: unable to load"
    return false, issues
  end
  if data.version ~= 1 then
    issues[#issues + 1] = "data/questboard.lua:version: unsupported version " .. tostring(data.version)
  end

  -- 层规则：offers 正整数、money_min < money_max、候选池足够挂出。
  if type(data.layer_rules) ~= "table" then
    issues[#issues + 1] = "data/questboard.lua:layer_rules: expected object"
  else
    for _, layer in ipairs(data.LAYERS or {}) do
      local rule = data.layer_rules[layer]
      local path = "layer_rules." .. layer
      if type(rule) ~= "table" then
        issues[#issues + 1] = "data/questboard.lua:" .. path .. ": missing rule"
      else
        local offers = tonumber(rule.offers)
        if not offers or offers < 1 or offers ~= math.floor(offers) then
          issues[#issues + 1] = "data/questboard.lua:" .. path .. ".offers: expected positive integer"
        else
          local pool_size = #(data.pool(layer, 0))
          if pool_size < offers then
            issues[#issues + 1] = "data/questboard.lua:" .. path
              .. ": 候选池只有 " .. tostring(pool_size) .. " 张，不够挂出 " .. tostring(offers)
          end
        end
        local mn, mx = tonumber(rule.money_min), tonumber(rule.money_max)
        if not mn or not mx or mn <= 0 or mx <= mn then
          issues[#issues + 1] = "data/questboard.lua:" .. path .. ".money_min/.money_max: expected 0 < min < max"
        end
      end
    end
  end

  -- 模板：id 唯一、layer 合法、目标物存在且 count 正、奖励落在层区间、声望阵营存在。
  local seen = {}
  for index, tpl in ipairs(data.templates or {}) do
    local where = "templates[" .. tostring(index) .. "]"
    if type(tpl) ~= "table" then
      issues[#issues + 1] = where .. ": expected object"
    else
      if type(tpl.id) ~= "string" or tpl.id == "" then
        issues[#issues + 1] = where .. ".id: required non-empty string"
      elseif seen[tpl.id] then
        issues[#issues + 1] = where .. ".id: duplicate " .. tostring(tpl.id)
      else
        seen[tpl.id] = true
      end
      if type(tpl.title) ~= "string" or tpl.title == "" then
        issues[#issues + 1] = where .. ".title: required non-empty string"
      end
      local layer_ok = false
      for _, name in ipairs(data.LAYERS) do
        if name == tpl.layer then
          layer_ok = true
        end
      end
      if not layer_ok then
        issues[#issues + 1] = where .. ".layer: illegal " .. tostring(tpl.layer)
      end

      local objective = tpl.objective
      if type(objective) ~= "table" or type(objective.item) ~= "string" or objective.item == "" then
        issues[#issues + 1] = where .. ".objective: expected { item=<id>, count=N }"
      else
        if not item_def(objective.item) then
          issues[#issues + 1] = where .. ".objective.item: unknown item " .. tostring(objective.item)
        end
        local count = tonumber(objective.count)
        if not count or count < 1 or count ~= math.floor(count) then
          issues[#issues + 1] = where .. ".objective.count: expected positive integer"
        end
      end

      local reward = tpl.reward
      if type(reward) ~= "table" then
        issues[#issues + 1] = where .. ".reward: expected object"
      else
        local money = tonumber(reward.money)
        local rule = type(tpl.layer) == "string" and data.layer_rules[tpl.layer] or nil
        if not money or money <= 0 then
          issues[#issues + 1] = where .. ".reward.money: expected positive number"
        elseif rule then
          local mn, mx = tonumber(rule.money_min), tonumber(rule.money_max)
          if mn and mx and (money < mn or money > mx) then
            issues[#issues + 1] = where .. ".reward.money: " .. tostring(money)
              .. " 不在 " .. tostring(tpl.layer) .. " 层区间 [" .. tostring(mn) .. "," .. tostring(mx) .. "]"
          end
        end
        if reward.item ~= nil then
          if type(reward.item) ~= "table" or type(reward.item.id) ~= "string" or not item_def(reward.item.id) then
            issues[#issues + 1] = where .. ".reward.item: unknown item " .. tostring(reward.item and reward.item.id)
          elseif not tonumber(reward.item.count) or tonumber(reward.item.count) < 1 then
            issues[#issues + 1] = where .. ".reward.item.count: expected positive integer"
          end
        end
        if reward.rep ~= nil then
          if type(reward.rep) ~= "table" or not relationships_data.faction(reward.rep.faction) then
            issues[#issues + 1] = where .. ".reward.rep.faction: unknown faction " .. tostring(reward.rep and reward.rep.faction)
          elseif not tonumber(reward.rep.amount) then
            issues[#issues + 1] = where .. ".reward.rep.amount: expected number"
          end
        end
      end

      if tpl.min_day ~= nil then
        local min_day = tonumber(tpl.min_day)
        if not min_day or min_day < 1 or min_day ~= math.floor(min_day) then
          issues[#issues + 1] = where .. ".min_day: expected positive integer"
        end
      end
    end
  end

  return #issues == 0, issues
end

return M
