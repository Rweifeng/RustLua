-- data/black_market.lua：《江湖有田》黑市抉择循环配置（docs/54 阶段3，2026-10-06 2.2）。
--
-- 纯数据注册表：新增黑市商品/抉择事件只改本文件，不改 systems/black_market.lua 与
-- Rust 核心。运行时只做校验与执行（与 data/shops.lua 同约定）。
--
-- 字段：
--   hours       营业窗口 { open, close }，单位＝当日分钟数（0..1440），左闭右开。
--               黑市深夜出没：1320..1440（22:00–24:00）。跨午夜窗口不支持
--               （close <= open 校验失败，与正规商店同口径）。
--   risk        风险档 →「东窗事发」概率（0..1）。低/中/高三档。
--   caught_penalty 高风险被抓后的统一惩罚：favor 跌落 + rep 声望罚（village/sect）。
--   favor_tiers 黑市声望档位（派生值，存档只存 favor 分数）：favor >= min 即落档，
--               越高档解锁越好的货品。
--   stock       黑市**卖给**玩家的违禁品/稀有货（玩家「买入」）：
--                 item       物品 id（引用 data.items，含 items_stage3 扩展）
--                 price      黑市价（文），必须 > 0
--                 risk       风险档 low/mid/high
--                 min_favor  解锁所需黑市声望分（档位门槛；0 = 生客即可买）
--                 favor_gain 成交（未被抓）为黑市声望加的分
--                 rep        成交对 village/sect 阵营声望的影响（deal，负分为主）
--   buys        黑市**从玩家收购**的稀有货（玩家「卖出」）：
--                 item / price / risk / favor_gain / rep 同上；price 为黑市收购价
--                 （通常高于正规货摊 sell，体现「来路不明的高价收」）。
--   events      抉择事件池（夜半来客三选：接受 / 拒绝 / 报官）：
--                 id / title / text
--                 choices.accept  接受：钱、给物、黑市声望、阵营声望各后果
--                 choices.refuse  拒绝：无事发生（空表）
--                 choices.report  报官：黑市声望大跌、village 声望上涨
--
-- 引用完整性由 validate_all 在加载期校验（硬风险：运行时对未知物品/阵营静默兜底会
-- 让「买了个不存在的东西」「罚了个不存在的阵营」变成无声 bug）。
local M = {}

M.version = 1

-- 深夜营业：22:00–24:00（左闭右开）。
M.hours = { open = 1320, close = 1440 }

-- 风险档 → 东窗事发概率。
M.risk = { low = 0.05, mid = 0.25, high = 0.5 }

-- 被抓统一惩罚：黑市声望 -5，云泽村声望 -4（官面追查，村里风评变差）。
M.caught_penalty = { favor = 5, rep = { village = -4 } }

-- 黑市声望档位（favor 0..100，派生）。
M.favor_tiers = {
  { min = 0, key = "stranger", name = "生客" },
  { min = 10, key = "acquaintance", name = "熟客" },
  { min = 30, key = "regular", name = "常客" },
  { min = 60, key = "patron", name = "金主" }
}

-- 黑市卖给玩家（买入）。越稀有越贵、风险越高、档位门槛越高。
M.stock = {
  { item = "smuggled_salt", price = 20, risk = "low", min_favor = 0, favor_gain = 1, rep = { village = -1 } },
  { item = "smuggled_liquor", price = 38, risk = "low", min_favor = 0, favor_gain = 1, rep = { village = -1 } },
  { item = "tiger_bone", price = 85, risk = "low", min_favor = 0, favor_gain = 1, rep = { village = -1, sect = -1 } },
  { item = "poison_powder", price = 55, risk = "mid", min_favor = 0, favor_gain = 2, rep = { village = -2 } },
  { item = "demon_mask", price = 140, risk = "mid", min_favor = 10, favor_gain = 2, rep = { village = -2 } },
  { item = "poison_dagger", price = 170, risk = "high", min_favor = 10, favor_gain = 3, rep = { village = -3, sect = -1 } },
  { item = "blood_jade", price = 280, risk = "mid", min_favor = 30, favor_gain = 3, rep = { village = -2 } },
  { item = "silk_armor", price = 300, risk = "high", min_favor = 30, favor_gain = 3, rep = { village = -3, sect = -1 } },
  { item = "forbidden_manual", price = 250, risk = "high", min_favor = 60, favor_gain = 4, rep = { village = -2, sect = -3 } },
  { item = "revival_pill", price = 480, risk = "high", min_favor = 60, favor_gain = 4, rep = { village = -3 } }
}

-- 黑市从玩家收购（卖出）：稀有药材/宝石出高价，风险随货值升高。
M.buys = {
  { item = "ginseng", price = 120, risk = "low", favor_gain = 1, rep = { village = -1 } },
  { item = "polygonum", price = 90, risk = "low", favor_gain = 1, rep = { village = -1 } },
  { item = "winter_shen", price = 110, risk = "mid", favor_gain = 2, rep = { village = -2 } },
  { item = "emerald", price = 300, risk = "mid", favor_gain = 2, rep = { village = -2, sect = -1 } },
  { item = "luminous_pearl", price = 1000, risk = "high", favor_gain = 3, rep = { village = -3, sect = -1 } },
  { item = "diamond", price = 560, risk = "high", favor_gain = 3, rep = { village = -3, sect = -1 } }
}

-- 抉择事件池：夜半来客，三选。
M.events = {
  {
    id = "midnight_herb",
    title = "夜半来客求购药材",
    text = "深夜有人叩窗，愿出三倍价钱收走你手头的一株老山参，只问货不问来路。",
    choices = {
      accept = { money = 150, favor = 2, rep = { village = -3 } },
      refuse = {},
      report = { favor = -5, rep = { village = 6 } }
    }
  },
  {
    id = "night_peddler",
    title = "私盐夜贩",
    text = "墙角黑影兜售一批官盐，说能便宜匀你几斗，只等你点头。",
    choices = {
      accept = { money = 80, favor = 1, rep = { village = -2, sect = -1 } },
      refuse = {},
      report = { favor = -3, rep = { village = 4 } }
    }
  },
  {
    id = "found_bundle",
    title = "来路不明的包袱",
    text = "草丛里拾到一只封口包袱，沉甸甸像是货商失物，打开还是送官？",
    choices = {
      accept = { give_item = "demon_mask", give_count = 1, favor = 3, rep = { village = -2 } },
      refuse = {},
      report = { favor = -2, rep = { village = 3, sect = 1 } }
    }
  }
}

-- ---------- 加载期校验（与 systems/shop.lua 同约定：返回 true, issues） ----------

local RISK_GRADES = { low = true, mid = true, high = true }
local FACTIONS = { village = true, sect = true }

function M.validate_all()
  local issues = {}
  local items = require("data.items")
  local tools = require("data.tools")
  local function known_item(id)
    return type(id) == "string" and (items[id] ~= nil or tools[id] ~= nil)
  end

  if M.version ~= 1 then
    issues[#issues + 1] = "data/black_market.lua:version: unsupported version " .. tostring(M.version)
  end

  -- 营业窗口：与正规商店同口径（左闭右开、不跨午夜）。
  if type(M.hours) ~= "table" then
    issues[#issues + 1] = "data/black_market.lua:hours: expected object { open, close }"
  else
    local open = tonumber(M.hours.open)
    local close = tonumber(M.hours.close)
    if not open or not close then
      issues[#issues + 1] = "data/black_market.lua:hours.open/close: expected numbers"
    elseif open < 0 or close > 1440 or close <= open then
      issues[#issues + 1] = "data/black_market.lua:hours: illegal window " .. tostring(open) .. ".." .. tostring(close)
    end
  end

  -- 风险档概率。
  if type(M.risk) ~= "table" then
    issues[#issues + 1] = "data/black_market.lua:risk: expected object"
  else
    for grade, chance in pairs(M.risk) do
      if not RISK_GRADES[grade] then
        issues[#issues + 1] = "data/black_market.lua:risk." .. tostring(grade) .. ": unknown grade (low/mid/high)"
      end
      local c = tonumber(chance)
      if not c or c < 0 or c > 1 then
        issues[#issues + 1] = "data/black_market.lua:risk." .. tostring(grade) .. ": expected number in [0,1]"
      end
    end
  end

  -- 档位表：min 升序、首档从 0 起。
  if type(M.favor_tiers) ~= "table" or #M.favor_tiers == 0 then
    issues[#issues + 1] = "data/black_market.lua:favor_tiers: expected non-empty array"
  else
    local prev = nil
    for i, tier in ipairs(M.favor_tiers) do
      local path = "data/black_market.lua:favor_tiers[" .. tostring(i) .. "]"
      if type(tier) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(tier.key) ~= "string" or tier.key == "" then
          issues[#issues + 1] = path .. ".key: required non-empty string"
        end
        local min = tonumber(tier.min)
        if not min then
          issues[#issues + 1] = path .. ".min: expected number"
        elseif prev ~= nil and min <= prev then
          issues[#issues + 1] = path .. ".min: expected strictly increasing"
        end
        prev = min or prev
      end
    end
  end

  -- 商品条目通用校验。
  local function check_entry(where, entry)
    local path = "data/black_market.lua:" .. where
    if type(entry) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
      return
    end
    if not known_item(entry.item) then
      issues[#issues + 1] = path .. ": unknown item " .. tostring(entry.item)
    end
    local price = tonumber(entry.price)
    if not price or price <= 0 then
      issues[#issues + 1] = path .. ".price: expected positive number"
    end
    if entry.risk ~= nil and not RISK_GRADES[entry.risk] then
      issues[#issues + 1] = path .. ".risk: unknown grade " .. tostring(entry.risk)
    end
    if entry.min_favor ~= nil and (type(entry.min_favor) ~= "number" or entry.min_favor < 0) then
      issues[#issues + 1] = path .. ".min_favor: expected non-negative number"
    end
    if entry.rep ~= nil then
      if type(entry.rep) ~= "table" then
        issues[#issues + 1] = path .. ".rep: expected object"
      else
        for faction in pairs(entry.rep) do
          if not FACTIONS[faction] then
            issues[#issues + 1] = path .. ".rep: unknown faction " .. tostring(faction)
          end
        end
      end
    end
  end

  local seen = {}
  if type(M.stock) ~= "table" or #M.stock == 0 then
    issues[#issues + 1] = "data/black_market.lua:stock: expected non-empty array"
  else
    for i, entry in ipairs(M.stock) do
      check_entry("stock[" .. tostring(i) .. "]", entry)
      if type(entry) == "table" then
        if seen[entry.item] then
          issues[#issues + 1] = "data/black_market.lua:stock: duplicate item " .. tostring(entry.item)
        end
        seen[entry.item] = true
      end
    end
  end
  if type(M.buys) ~= "table" or #M.buys == 0 then
    issues[#issues + 1] = "data/black_market.lua:buys: expected non-empty array"
  else
    for i, entry in ipairs(M.buys) do
      check_entry("buys[" .. tostring(i) .. "]", entry)
      if type(entry) == "table" then
        if seen[entry.item] then
          issues[#issues + 1] = "data/black_market.lua:buys: item also in stock (arbitrage) " .. tostring(entry.item)
        end
        seen[entry.item] = true
      end
    end
  end

  -- 抉择事件：三选齐全、give_item 存在、rep 阵营合法。
  if type(M.events) ~= "table" or #M.events == 0 then
    issues[#issues + 1] = "data/black_market.lua:events: expected non-empty array"
  else
    local seen_ids = {}
    for i, ev in ipairs(M.events) do
      local path = "data/black_market.lua:events[" .. tostring(i) .. "]"
      if type(ev) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(ev.id) ~= "string" or ev.id == "" then
          issues[#issues + 1] = path .. ".id: required non-empty string"
        elseif seen_ids[ev.id] then
          issues[#issues + 1] = path .. ".id: duplicate " .. tostring(ev.id)
        end
        seen_ids[ev.id] = true
        local choices = ev.choices
        if type(choices) ~= "table" then
          issues[#issues + 1] = path .. ".choices: expected object"
        else
          for _, c in ipairs({ "accept", "refuse", "report" }) do
            local out = choices[c]
            if type(out) ~= "table" then
              issues[#issues + 1] = path .. ".choices." .. c .. ": expected object"
            else
              if out.give_item ~= nil and not known_item(out.give_item) then
                issues[#issues + 1] = path .. ".choices." .. c .. ".give_item: unknown item " .. tostring(out.give_item)
              end
              if out.rep ~= nil and type(out.rep) == "table" then
                for faction in pairs(out.rep) do
                  if not FACTIONS[faction] then
                    issues[#issues + 1] = path .. ".choices." .. c .. ".rep: unknown faction " .. tostring(faction)
                  end
                end
              end
            end
          end
        end
      end
    end
  end

  return #issues == 0, issues
end

return M
