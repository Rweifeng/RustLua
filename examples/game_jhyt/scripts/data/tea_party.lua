-- data/tea_party.lua：《江湖有田》山庄茶会社交数据注册表
-- （docs/54 §7.5 / docs/33 §8.3，2026-10-06 2.3，纯数据）。
--
-- 与 data/npcs.lua / data/relationships.lua 同约定：新增话题/调参 = 本文件加记录，
-- 无需改动 systems/tea_party.lua / Rust 核心。
--
-- 字段口径：
--   min_hearts      可邀门槛心数（2/5/8 心分档中的最低档；心 ×10 = 所需关系分）。
--                   5 心（50 分）/8 心（80 分）为「深交档」，本期不做行为分支，只在
--                   话题好感系数上用匹配度体现（见 topics.affinity）。
--   min_guests / max_guests
--                   每场最少/最多客人（2..4）。
--   cooldown_days   同一 NPC 茶会后 N 天内不可再邀（冷却按日期懒推导，无每日结算）。
--   tea_items       可用茶品白名单（必须是 data.items 里 kind=dish/consumable 的
--                   茶类物品）；每场消耗恰好 1 件（systems/tea_party.prepare）。
--   topics          话题池（≥10）：{ id, name, factions?, npcs?, affinity }。
--                   命中（该客阵营 ∈ factions 或该客 ∈ npcs）时在基础好感上叠加
--                   affinity 分，并记一条情报（intel）；都不命中只拿基础好感。
--   outcome         结算常数：base_affinity 每客基础好感、per_guest_reputation
--                   每客对其阵营的声望贡献（无阵营 NPC 不贡献声望）。
--
-- 引用完整性（NPC/阵营/物品存在性）由 validate_all 在加载期校验。
local M = {}

M.version = 1

-- 邀请门槛与场次规模（docs/33 §8.3）。
M.min_hearts = 2
M.min_guests = 2
M.max_guests = 4
-- 同人茶会冷却天数：第 D 天办过，第 D+cooldown_days 天起可再邀（含当天）。
M.cooldown_days = 7

-- 茶品白名单：现有山泉清茶（data/items.lua 的 tea_brew，consumable）。
-- 阶段 4 泡茶技艺（水质/水温/器具 5 档）落地后在此追加茶类物品 id。
M.tea_items = { "tea_brew" }

-- 话题池（10 条）。affinity 为「话题命中」时在基础好感上叠加的分（正整数）。
M.topics = {
  { id = "tp_martial",     name = "武学切磋",  factions = { "sect" },    affinity = 6,
    description = "演武心得与招式得失，青云门弟子最受用。" },
  { id = "tp_anecdote",    name = "江湖轶事",  affinity = 4,
    description = "走南闯北的奇闻异事，人人都爱听一段。" },
  { id = "tp_farming",     name = "农桑禾稼",  factions = { "village" }, affinity = 6,
    description = "时令耕织与年成丰歉，庄户人家聊得最投机。" },
  { id = "tp_poetry",      name = "诗词歌赋",  npcs = { "npc_su_wanqing", "npc_shen_qingluo" }, affinity = 7,
    description = "风花雪月的吟咏，苏晚晴与沈青萝最有兴致。" },
  { id = "tp_herbal",      name = "药膳养生",  npcs = { "npc_a_yao", "npc_shen_qingluo" }, affinity = 7,
    description = "药食同源的门道，阿药与医馆掌柜听得眼睛发亮。" },
  { id = "tp_sect_feud",   name = "门派恩怨",  npcs = { "npc_xuanhe", "npc_lu_guizang", "npc_gu_changfeng" }, affinity = 7,
    description = "江湖门派的旧怨新仇，玄鹤、陆归藏与顾长风各有话说。" },
  { id = "tp_tea_craft",   name = "煮茶论道",  npcs = { "npc_lin_xiaoman" }, affinity = 8,
    description = "火候与水品的讲究，种茶的林小满是这席话的知己。" },
  { id = "tp_trade",       name = "商贾行情",  npcs = { "npc_zhao_tiesuanpan", "npc_qianduobao" }, affinity = 7,
    description = "粮价与货路的起落，算盘与钱串子最熟稔。" },
  { id = "tp_escort",      name = "镖行走镖",  npcs = { "npc_shi_gandang", "npc_han_tieshan" }, affinity = 7,
    description = "押镖路上的风风雨雨，石敢当与韩铁山拍案称快。" },
  { id = "tp_travel",      name = "云游风物",  npcs = { "npc_xuanhe" }, affinity = 6,
    description = "名山大川的见闻，云游四方的玄鹤如数家珍。" }
}

-- 结算常数（每客）。
M.outcome = {
  base_affinity = 3,          -- 茶会基础好感（话题不命中也给）
  per_guest_reputation = 2    -- 每客对其所属阵营的声望贡献
}

-- 心数 → 所需关系分（与 data/relationship_events.heart_points 同口径：心 ×10）。
function M.heart_points(heart)
  local h = math.floor(tonumber(heart) or 0)
  return h * 10
end

-- 某话题是否命中该 NPC（阵营命中或指名命中）。
function M.topic_matches(topic, npc_id, faction_id)
  if type(topic) ~= "table" then
    return false
  end
  if type(topic.factions) == "table" then
    for _, f in ipairs(topic.factions) do
      if f == faction_id then
        return true
      end
    end
  end
  if type(topic.npcs) == "table" then
    for _, id in ipairs(topic.npcs) do
      if id == npc_id then
        return true
      end
    end
  end
  return false
end

function M.get_topic(id)
  for _, topic in ipairs(M.topics) do
    if topic.id == id then
      return topic
    end
  end
  return nil
end

-- 茶品白名单查询（id ∈ tea_items）。
function M.is_tea_item(item_id)
  if type(item_id) ~= "string" then
    return false
  end
  for _, id in ipairs(M.tea_items) do
    if id == item_id then
      return true
    end
  end
  return false
end

function M.validate_all()
  local issues = {}
  local npcs = require("data.npcs")
  local rel = require("data.relationships")
  local items = require("data.items")

  local function is_integer(v)
    return type(v) == "number" and v == math.floor(v)
  end

  if M.version ~= 1 then
    issues[#issues + 1] = "data/tea_party.lua:version: unsupported version " .. tostring(M.version)
  end

  -- 门槛与场次规模：整数、2..4 人、min <= max、心数 1..10。
  if not is_integer(M.min_hearts) or M.min_hearts < 1 or M.min_hearts > 10 then
    issues[#issues + 1] = "data/tea_party.lua:min_hearts: expected integer 1..10, got " .. tostring(M.min_hearts)
  end
  if not is_integer(M.min_guests) or not is_integer(M.max_guests)
    or M.min_guests < 2 or M.max_guests > 4 or M.min_guests > M.max_guests then
    issues[#issues + 1] = "data/tea_party.lua: min_guests/max_guests: expected 2..4 with min<=max, got "
      .. tostring(M.min_guests) .. "/" .. tostring(M.max_guests)
  end
  if not is_integer(M.cooldown_days) or M.cooldown_days <= 0 then
    issues[#issues + 1] = "data/tea_party.lua:cooldown_days: expected positive integer, got " .. tostring(M.cooldown_days)
  end

  -- 茶品白名单：非空、物品存在、kind ∈ dish/consumable。
  if type(M.tea_items) ~= "table" or #M.tea_items == 0 then
    issues[#issues + 1] = "data/tea_party.lua:tea_items: expected non-empty array"
  else
    for _, item_id in ipairs(M.tea_items) do
      local def = type(item_id) == "string" and items[item_id] or nil
      if not def then
        issues[#issues + 1] = "data/tea_party.lua:tea_items." .. tostring(item_id)
          .. ": unknown item (not in data.items)"
      elseif def.kind ~= "dish" and def.kind ~= "consumable" then
        issues[#issues + 1] = "data/tea_party.lua:tea_items." .. tostring(item_id)
          .. ": expected dish/consumable kind, got " .. tostring(def.kind)
      end
    end
  end

  -- 话题池：≥10 条、id 唯一且非空、affinity 正整数、factions/npcs 引用真实存在。
  if type(M.topics) ~= "table" or #M.topics < 10 then
    issues[#issues + 1] = "data/tea_party.lua:topics: expected >= 10 entries"
  else
    local seen = {}
    for index, topic in ipairs(M.topics) do
      local path = "data/tea_party.lua:topics[" .. tostring(index) .. "]"
      if type(topic) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if type(topic.id) ~= "string" or topic.id == "" then
          issues[#issues + 1] = path .. ".id: required non-empty string"
        elseif seen[topic.id] then
          issues[#issues + 1] = path .. ".id: duplicate topic id " .. topic.id
        else
          seen[topic.id] = true
        end
        if type(topic.name) ~= "string" or topic.name == "" then
          issues[#issues + 1] = path .. ".name: required non-empty string"
        end
        if not is_integer(topic.affinity) or topic.affinity <= 0 then
          issues[#issues + 1] = path .. ".affinity: expected positive integer, got " .. tostring(topic.affinity)
        end
        if topic.factions ~= nil then
          if type(topic.factions) ~= "table" then
            issues[#issues + 1] = path .. ".factions: expected array (may be omitted)"
          else
            for _, f in ipairs(topic.factions) do
              if not rel.faction(f) then
                issues[#issues + 1] = path .. ".factions." .. tostring(f) .. ": unknown faction " .. tostring(f)
              end
            end
          end
        end
        if topic.npcs ~= nil then
          if type(topic.npcs) ~= "table" then
            issues[#issues + 1] = path .. ".npcs: expected array (may be omitted)"
          else
            for _, npc_id in ipairs(topic.npcs) do
              if not npcs.get(npc_id) then
                issues[#issues + 1] = path .. ".npcs." .. tostring(npc_id) .. ": unknown npc " .. tostring(npc_id)
              end
            end
          end
        end
      end
    end
  end

  -- 结算常数：正整数。
  if type(M.outcome) ~= "table" then
    issues[#issues + 1] = "data/tea_party.lua:outcome: expected object"
  else
    if not is_integer(M.outcome.base_affinity) or M.outcome.base_affinity <= 0 then
      issues[#issues + 1] = "data/tea_party.lua:outcome.base_affinity: expected positive integer"
    end
    if not is_integer(M.outcome.per_guest_reputation) or M.outcome.per_guest_reputation <= 0 then
      issues[#issues + 1] = "data/tea_party.lua:outcome.per_guest_reputation: expected positive integer"
    end
  end

  return #issues == 0, issues
end

return M
