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

function M.faction(id)
  if type(id) ~= "string" then
    return nil
  end
  return M.factions[id]
end

function M.validate_all()
  local issues = {}
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

  local npcs = require("data.npcs")
  local items = require("data.items")
  local tools = require("data.tools")
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
