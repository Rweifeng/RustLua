-- data/relationships.lua：《江湖有田》关系与声望数据注册表（M18-04 纯数据）。
--
-- 与 data/npcs.lua / data/maps.lua 同约定：新增关系/礼物配置 = 本文件加记录，
-- 无需改动 relationship.lua / quest.lua / Rust 核心。
--
-- 字段：
--   version   数据版本（加载期校验）
--   factions  声望阵营：id → { name, min, max }（声望值 clamp 到 [min, max]）
--   tiers     关系档位表：{ min, key, name } 升序排列；tier_for(points) 取
--             最后一个 min <= points 的档位（0..100 关系分 → 陌生..莫逆）
--   reputation_tiers
--             阵营声望档位表（docs/33 §8.2 逐格对应：区间与称号由
--             jhyt_m24_numeric_alignment 比对，改数值要同时改文档）：
--             { min, max, key, name, effects }，左闭右闭、逐档连续；
--             effects 是**只读的效果预算表**，消费者每次交易/交付自取，
--             档位自身不写任何状态（存档只存分数，档位是派生值）。
--             已实现效果：price_off = 店主阵营的收购折扣百分数（整数，
--             0 或字段缺省 = 原价）；新增效果键要同时改 EFFECT_KEYS 白名单
--             与消费方，否则校验直接报错。
--   npcs      npc_id（引用 data/npcs.lua 的 persist_id）→ { gifts = { item_id = 分值 } }
--             gifts 可为空表（该 NPC 不收礼）；item_id 必须存在于
--             data.items / data.tools；分值必须为正数
--
-- 引用的完整性由 validate_all 在加载期校验（错误含文件/对象/字段路径）。
local M = {}

M.version = 1

M.factions = {
  village = { name = "云泽村", min = -100, max = 100 },
  sect = { name = "青云门派", min = -100, max = 100 }
}

M.tiers = {
  { min = 0, key = "stranger", name = "陌生" },
  { min = 20, key = "acquainted", name = "相识" },
  { min = 45, key = "friendly", name = "友善" },
  { min = 70, key = "close", name = "亲厚" },
  { min = 90, key = "devoted", name = "莫逆" }
}

-- 阵营声望五档（docs/33 §8.2）。声望可为负（factions 的 min 下界是 -100），
-- 低于第一档 min 一律按第一档算（无任何正向效果），所以档位查表没有空洞。
M.reputation_tiers = {
  { min = 0, max = 19, key = "stranger", name = "陌生", effects = {} },
  { min = 20, max = 39, key = "known", name = "熟识", effects = { price_off = 5 } },
  { min = 40, max = 59, key = "trusted", name = "受信", effects = { price_off = 8 } },
  { min = 60, max = 79, key = "esteemed", name = "重视", effects = { price_off = 12 } },
  { min = 80, max = 100, key = "core", name = "核心", effects = { price_off = 15 } }
}

-- effects 允许的效果键（新增键必须同时有消费方，否则校验报错）。
M.effect_keys = { price_off = true }

M.npcs = {
  npc_shen_qingluo = {
    gifts = { lingzhi = 8, rice = 3, heal_pill = 5 }
  },
  npc_gu_changfeng = {
    gifts = { heal_pill = 8, lingzhi = 4 }
  },
  npc_lin_xiaoman = {
    gifts = { rice = 2, lingzhi = 2 }
  },
  npc_han_tieshan = {
    gifts = { heal_pill = 3 }
  },
  npc_su_wanqing = {
    gifts = { lingzhi = 3, heal_pill = 2 }
  },
  -- 赵铁算盘只认钱不收礼；玄鹤云游四方无所需（空表 = not_giftable）。
  npc_zhao_tiesuanpan = {
    gifts = {}
  },
  npc_xuanhe = {
    gifts = {}
  },
  -- v0.2 主线四五段与结局新引用的三位 NPC：陆归藏（书房长老，收茶与行气散）、
  -- 阿七（村里少年，收零食野果）、司徒魁（玄冥会执事，敌对立场、不收礼）。
  npc_lu_guizang = {
    gifts = { tea_brew = 5, herb_tonic = 4 }
  },
  npc_a_qi = {
    gifts = { rice_ball = 3, wild_fruit = 2 }
  },
  npc_situ_kui = {
    gifts = {}
  }
}

function M.npc_ids()
  local out = {}
  for id, _ in pairs(M.npcs) do
    out[#out + 1] = id
  end
  table.sort(out)
  return out
end

-- NPC 收某件礼物的关系分；未配置（不收该物/NPC 未登记）返回 nil。
function M.gift_points(npc_id, item_id)
  local entry = type(npc_id) == "string" and M.npcs[npc_id] or nil
  if not entry or type(entry.gifts) ~= "table" then
    return nil
  end
  local points = entry.gifts[item_id]
  if type(points) ~= "number" then
    return nil
  end
  return points
end

-- 关系分 → 档位表（最后一个 min <= points 的档位；无匹配返回 nil）。
function M.tier_for(points)
  local value = tonumber(points)
  if not value then
    return nil
  end
  local tier = nil
  for _, entry in ipairs(M.tiers) do
    if value >= entry.min then
      tier = entry
    end
  end
  return tier
end

-- 关系档位 key → 档位表条目（"friendly" → { min = 45, ... }）；未知 key 返回 nil。
-- 数据里的对话条件（cond.tier_gte）用它把"档位名"翻成可比较的下限。
function M.tier_by_key(key)
  if type(key) ~= "string" then
    return nil
  end
  for _, entry in ipairs(M.tiers) do
    if entry.key == key then
      return entry
    end
  end
  return nil
end

-- 关系档位 key 的升序列（校验器报错文案里的"已知档位"提示）。
function M.tier_keys()
  local out = {}
  for _, entry in ipairs(M.tiers) do
    if type(entry.key) == "string" then
      out[#out + 1] = entry.key
    end
  end
  table.sort(out)
  return out
end

-- 声望值 → 档位表条目：取最后一个 min <= value 的档位；低于第一档 min（负声望）
-- 按第一档算——阵营下界是 -100，档位表只覆盖 0..100，不补空洞就是"负声望无任何效果"。
function M.reputation_tier_for(value)
  local points = tonumber(value)
  if not points then
    return nil
  end
  local tier = nil
  for _, entry in ipairs(M.reputation_tiers) do
    if points >= entry.min then
      tier = entry
    end
  end
  return tier or M.reputation_tiers[1]
end

-- 声望档位 key → 档位表条目；未知 key 返回 nil（quest 目标的 tier 字段用它校验）。
function M.reputation_tier_by_key(key)
  if type(key) ~= "string" then
    return nil
  end
  for _, entry in ipairs(M.reputation_tiers) do
    if entry.key == key then
      return entry
    end
  end
  return nil
end

-- 声望档位 key 的升序列（诊断文案与校验器的"已知档位"提示用）。
function M.reputation_tier_keys()
  local out = {}
  for _, entry in ipairs(M.reputation_tiers) do
    if type(entry.key) == "string" then
      out[#out + 1] = entry.key
    end
  end
  table.sort(out)
  return out
end

function M.faction(id)
  if type(id) ~= "string" then
    return nil
  end
  return M.factions[id]
end

function M.validate_all()
  local issues = {}
  local function is_integer(value)
    return type(value) == "number" and value == math.floor(value)
  end
  local effect_names = {}
  for key in pairs(M.effect_keys) do
    effect_names[#effect_names + 1] = key
  end
  table.sort(effect_names)
  if M.version ~= 1 then
    issues[#issues + 1] = "data/relationships.lua:version: unsupported version " .. tostring(M.version)
  end

  if type(M.factions) ~= "table" or next(M.factions) == nil then
    issues[#issues + 1] = "data/relationships.lua:factions: expected non-empty table"
  else
    for id, faction in pairs(M.factions) do
      local path = "data/relationships.lua:factions." .. tostring(id)
      if type(faction) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(faction.name) ~= "string" or faction.name == "" then
          issues[#issues + 1] = path .. ".name: required non-empty string"
        end
        if type(faction.min) ~= "number" or type(faction.max) ~= "number" then
          issues[#issues + 1] = path .. ".min/.max: expected numbers"
        elseif faction.min >= faction.max then
          issues[#issues + 1] = path
            .. ".min: expected min < max, got "
            .. tostring(faction.min) .. " >= " .. tostring(faction.max)
        end
      end
    end
  end

  if type(M.tiers) ~= "table" or #M.tiers == 0 then
    issues[#issues + 1] = "data/relationships.lua:tiers: expected non-empty array"
  else
    local previous = nil
    for index, tier in ipairs(M.tiers) do
      local path = "data/relationships.lua:tiers[" .. tostring(index) .. "]"
      if type(tier) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(tier.key) ~= "string" or tier.key == "" then
          issues[#issues + 1] = path .. ".key: required non-empty string"
        end
        if type(tier.name) ~= "string" or tier.name == "" then
          issues[#issues + 1] = path .. ".name: required non-empty string"
        end
        if type(tier.min) ~= "number" then
          issues[#issues + 1] = path .. ".min: expected number"
        elseif previous ~= nil and tier.min <= previous then
          issues[#issues + 1] = path
            .. ".min: expected strictly increasing, got "
            .. tostring(tier.min) .. " <= " .. tostring(previous)
        end
        previous = type(tier.min) == "number" and tier.min or previous
      end
    end
    -- 默认 0 分必须落在第一档；最高档必须覆盖到声望上限（全值域有档可查）。
    if previous ~= nil then
      local first = M.tiers[1]
      if type(first) == "table" and type(first.min) == "number" and first.min > 0 then
        issues[#issues + 1] = "data/relationships.lua:tiers[1].min: first tier must start at or below 0"
      end
      local max_reputation = 0
      for _, faction in pairs(type(M.factions) == "table" and M.factions or {}) do
        if type(faction) == "table" and type(faction.max) == "number" then
          max_reputation = math.max(max_reputation, faction.max)
        end
      end
      if previous > max_reputation then
        issues[#issues + 1] = "data/relationships.lua:tiers[#].min: last tier must cover max reputation "
          .. tostring(max_reputation)
      end
    end
  end

  -- 声望档位表（docs/33 §8.2）：区间必须从 0 起、逐档连续、末档覆盖到声望上限；
  -- effects 只认白名单键，price_off 必须是 0..100 的整数且随档位单调不减
  -- （"声望越高折扣越大"是 §8.2 的表义，写反了就是数值 bug）。
  if type(M.reputation_tiers) ~= "table" or #M.reputation_tiers == 0 then
    issues[#issues + 1] = "data/relationships.lua:reputation_tiers: expected non-empty array"
  else
    local seen_keys = {}
    local previous_max = -1
    local previous_off = 0
    for index, tier in ipairs(M.reputation_tiers) do
      local path = "data/relationships.lua:reputation_tiers[" .. tostring(index) .. "]"
      if type(tier) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(tier.key) ~= "string" or tier.key == "" then
          issues[#issues + 1] = path .. ".key: required non-empty string"
        elseif seen_keys[tier.key] then
          issues[#issues + 1] = path .. ".key: duplicate tier key " .. tier.key
        end
        seen_keys[tier.key] = true
        if type(tier.name) ~= "string" or tier.name == "" then
          issues[#issues + 1] = path .. ".name: required non-empty string"
        end
        if not is_integer(tier.min) or not is_integer(tier.max) then
          issues[#issues + 1] = path .. ".min/.max: expected integers"
        elseif tier.min > tier.max then
          issues[#issues + 1] = path
            .. ".min: expected min <= max, got "
            .. tostring(tier.min) .. " > " .. tostring(tier.max)
        elseif tier.min ~= previous_max + 1 then
          issues[#issues + 1] = path
            .. ".min: tiers must be contiguous, expected "
            .. tostring(previous_max + 1) .. ", got " .. tostring(tier.min)
        else
          previous_max = tier.max
        end
        if tier.effects ~= nil and type(tier.effects) ~= "table" then
          issues[#issues + 1] = path .. ".effects: expected table (may be empty)"
        elseif type(tier.effects) == "table" then
          for key, value in pairs(tier.effects) do
            if not M.effect_keys[key] then
              issues[#issues + 1] = path .. ".effects." .. tostring(key)
                .. ": unknown effect key (known: " .. table.concat(effect_names, "/") .. ")"
            end
          end
          local off = tier.effects.price_off
          if off ~= nil then
            if not is_integer(off) or off < 0 or off > 100 then
              issues[#issues + 1] = path .. ".effects.price_off: expected integer 0..100"
            elseif off < previous_off then
              issues[#issues + 1] = path
                .. ".effects.price_off: expected non-decreasing, got "
                .. tostring(off) .. " < " .. tostring(previous_off)
            else
              previous_off = off
            end
          end
        end
      end
    end
    if type(M.reputation_tiers[1]) == "table" and M.reputation_tiers[1].min ~= 0 then
      issues[#issues + 1] = "data/relationships.lua:reputation_tiers[1].min: must start at 0"
    end
    local max_reputation = 0
    for _, faction in pairs(type(M.factions) == "table" and M.factions or {}) do
      if type(faction) == "table" and type(faction.max) == "number" then
        max_reputation = math.max(max_reputation, faction.max)
      end
    end
    if previous_max ~= max_reputation then
      issues[#issues + 1] = "data/relationships.lua:reputation_tiers[#].max: last tier must end at max reputation "
        .. tostring(max_reputation) .. ", got " .. tostring(previous_max)
    end
  end

  local npcs = require("data.npcs")
  local items = require("data.items")
  local tools = require("data.tools")

  -- 每个 NPC 的阵营（data/npcs.lua 的 faction 字段，商店折扣按店主阵营声望算）：
  -- 可为 nil（无阵营，不参与折扣），但不能写未知阵营。
  for _, npc in ipairs(npcs.list()) do
    if npc.faction ~= nil and not M.faction(npc.faction) then
      issues[#issues + 1] = "data/npcs.lua:" .. tostring(npc.persist_id)
        .. ".faction: unknown faction " .. tostring(npc.faction)
    end
  end

  if type(M.npcs) ~= "table" or next(M.npcs) == nil then
    issues[#issues + 1] = "data/relationships.lua:npcs: expected non-empty table"
  else
    for npc_id, entry in pairs(M.npcs) do
      local path = "data/relationships.lua:npcs." .. tostring(npc_id)
      if not npcs.get(npc_id) then
        issues[#issues + 1] = path .. ": unknown npc (not in data/npcs.lua)"
      elseif type(entry) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(entry.gifts) ~= "table" then
          issues[#issues + 1] = path .. ".gifts: expected table (may be empty)"
        else
          for item_id, points in pairs(entry.gifts) do
            local gift_path = path .. ".gifts." .. tostring(item_id)
            if items[item_id] == nil and tools[item_id] == nil then
              issues[#issues + 1] = gift_path .. ": unknown item (not in data/items.lua or data/tools.lua)"
            end
            if type(points) ~= "number" or points <= 0 then
              issues[#issues + 1] = gift_path .. ": expected positive number"
            end
          end
        end
      end
    end
  end

  return #issues == 0, issues
end

return M
