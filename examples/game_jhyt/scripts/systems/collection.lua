-- systems/collection.lua：《江湖有田》图鉴 6 类（收集）系统
-- （docs/54 §4.12 / docs/33 §4.12，2026-10-06 1.7）。
--
-- 发现记录 + 进度 + 里程碑奖励，是"出货记录/熟练度"之外的第三条**收集向**成长线。
--
-- 设计要点（与 mastery.lua 同构：后台记录、不写 last_action、capture/restore/reset）：
--   * notify(category, id)：各玩法系统（farm 收获 / fishing 钓获 / mine 挖矿 /
--     gather 采药 / cooking 制菜 / combat 击敌）在产出/击杀成功后调用，登记标本。
--     未知 category / 未知 id → (false, reason)，不写脏数据；已见过 → (true,"already")
--     且不重复触发奖励。本阶段样例以"样例显式 notify"模拟各触发点；真系统钩子走
--     补丁文本交 Organizer 集成（见交付说明）。
--   * 进度：每类 seen/total、总进度 %；50% / 100% 两个里程碑各发一次奖励
--     （data.collections.rewards，金币，只引用既有货币）。
--   * 存档：seen 与已发里程碑随 save_manager 持久（capture/restore）；reset 清空。
--   * 不写 state.last_action——图鉴是后台收集日志，不干扰耕作/出货等动作断言。
local state = require("core.state")
local data = require("data.collections")

local M = {}

-- 运行时状态（capture/restore 随存档持久；reset 清空）
local seen = {} -- category → { [id] = true }
local granted = {} -- category → { [50] = true, [100] = true }

local function entry_list(category)
  return data.entries[category]
end

local function is_valid_id(category, id)
  local list = entry_list(category)
  if not list then
    return false
  end
  for _, candidate in ipairs(list) do
    if candidate == id then
      return true
    end
  end
  return false
end

-- 6 类稳定顺序（UI/验收用）。
function M.categories()
  return data.order
end

function M.category_name(category)
  return data.name[category] or category
end

function M.total(category)
  local list = entry_list(category)
  return list and #list or 0
end

function M.seen_count(category)
  local n = 0
  local bag = seen[category]
  if bag then
    for _ in pairs(bag) do
      n = n + 1
    end
  end
  return n
end

function M.seen(category, id)
  return seen[category] ~= nil and seen[category][id] == true
end

-- 每类进度：{ seen, total, pct(0..100 整数) }。
function M.progress(category)
  local total = M.total(category)
  local n = M.seen_count(category)
  local pct = total > 0 and math.floor((n / total) * 100 + 0.5) or 0
  return { seen = n, total = total, pct = pct }
end

-- 全局总进度（跨 6 类）。
function M.overall()
  local seen_all, total_all = 0, 0
  for _, category in ipairs(data.order) do
    local p = M.progress(category)
    seen_all = seen_all + p.seen
    total_all = total_all + p.total
  end
  local pct = total_all > 0 and math.floor((seen_all / total_all) * 100 + 0.5) or 0
  return { seen = seen_all, total = total_all, pct = pct }
end

-- 某类已发的里程碑（只读）：{ [50]=bool, [100]=bool }。
function M.granted(category)
  local out = {}
  local bag = granted[category]
  if bag then
    for tier, v in pairs(bag) do
      out[tier] = v == true
    end
  end
  return out
end

-- 发放里程碑奖励（金币入账；item 字段预留）。幂等：已发不重发。
local function grant_once(category, tier)
  if granted[category] and granted[category][tier] then
    return nil
  end
  local cfg = data.rewards[tier]
  if not cfg then
    return nil
  end
  granted[category] = granted[category] or {}
  granted[category][tier] = true
  if tonumber(cfg.gold) and cfg.gold > 0 then
    state.money = (tonumber(state.money) or 0) + cfg.gold
  end
  return { tier = tier, gold = cfg.gold or 0, item = cfg.item, count = cfg.count }
end

-- 登记一条标本。返回 (true, "ok"/"already") 或 (false, reason)。
-- reason ∈ unknown_category / unknown_id。新标本若让该类跨过 50%/100% 里程碑，
-- 立即发奖（金币入账），每档只发一次。
function M.notify(category, id)
  if type(category) ~= "string" or not entry_list(category) then
    return false, "unknown_category"
  end
  if type(id) ~= "string" or not is_valid_id(category, id) then
    return false, "unknown_id"
  end
  if M.seen(category, id) then
    return true, "already"
  end
  seen[category] = seen[category] or {}
  seen[category][id] = true
  -- 里程碑判定：新标本入列后按 seen 计数跨过阈值即发。
  local total = M.total(category)
  local n = M.seen_count(category)
  local half = math.ceil(total / 2)
  if n >= half then
    grant_once(category, 50)
  end
  if n >= total then
    grant_once(category, 100)
  end
  return true, "ok"
end

-- ---------- 存档 ----------

function M.capture()
  local seen_data, granted_data = {}, {}
  for category, bag in pairs(seen) do
    if entry_list(category) and type(bag) == "table" then
      local safe = {}
      for id in pairs(bag) do
        safe[id] = true
      end
      seen_data[category] = safe
    end
  end
  for category, tiers in pairs(granted) do
    if entry_list(category) and type(tiers) == "table" then
      local safe = {}
      for tier, v in pairs(tiers) do
        if (tier == 50 or tier == 100) and v then
          safe[tier] = true
        end
      end
      granted_data[category] = safe
    end
  end
  return { seen = seen_data, granted = granted_data }
end

function M.restore(data_in)
  seen, granted = {}, {}
  if type(data_in) ~= "table" then
    return
  end
  if type(data_in.seen) == "table" then
    for category, bag in pairs(data_in.seen) do
      local list = entry_list(category)
      if list and type(bag) == "table" then
        local safe = {}
        for id in pairs(bag) do
          if is_valid_id(category, id) then
            safe[id] = true
          end
        end
        seen[category] = safe
      end
    end
  end
  if type(data_in.granted) == "table" then
    for category, tiers in pairs(data_in.granted) do
      if entry_list(category) and type(tiers) == "table" then
        local safe = {}
        for tier, v in pairs(tiers) do
          if (tier == 50 or tier == 100) and v then
            safe[tier] = true
          end
        end
        granted[category] = safe
      end
    end
  end
end

function M.reset()
  seen, granted = {}, {}
end

-- ---------- 加载期校验 ----------

-- 结构不变量：6 类齐全非空、类内 id 无重复、引用的 id 在源头注册表真实存在、
-- 奖励档 50/100 齐全。返回 ""（通过）或换行分隔的问题列表（与 shipping/mastery 同约）。
function M.validate_all()
  local report = {}
  if #data.order ~= 6 then
    report[#report + 1] = "collection: 类别数须为 6，现有 " .. tostring(#data.order)
  end
  local in_base = require("data.items")
  for _, category in ipairs(data.order) do
    local list = entry_list(category)
    if not list or #list == 0 then
      report[#report + 1] = "collection: 类别 " .. tostring(category) .. " 条目为空"
    else
      local dup = {}
      for _, id in ipairs(list) do
        if type(id) ~= "string" or id == "" then
          report[#report + 1] = "collection: " .. category .. " 含空 id"
        end
        if dup[id] then
          report[#report + 1] = "collection: " .. category .. " id 重复 " .. tostring(id)
        end
        dup[id] = true
      end
      -- 作物/药材/菜肴引用物品注册表：id 必须真实存在（防止图鉴登记了不存在的标本）。
      if category == "crop" or category == "herb" or category == "dish" then
        for _, id in ipairs(list) do
          if not in_base[id] then
            report[#report + 1] = "collection: " .. category .. " 引用了不存在的物品 " .. tostring(id)
          end
        end
      end
    end
  end
  if not data.rewards[50] or not data.rewards[100] then
    report[#report + 1] = "collection: 奖励档须含 50 与 100"
  end
  return table.concat(report, "\n")
end

return M
