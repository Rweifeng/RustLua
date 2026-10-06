-- data/achievements.lua：《江湖有田》成就注册表（docs/54 §4.12/§9，2026-10-06 1.8）。
--
-- 30 个成就（含 5 个隐藏），覆盖务农/出货/熟练度/好感/图鉴/时间/战斗/节日八类。
--
-- 字段：
--   id          稳定标识（全局唯一）
--   name        玩家可见称号（隐藏成就解锁前也显示此名？——不：隐藏成就未解锁时
--               name/description 都由 systems/achievement.lua 兜底为 "???"）
--   description 玩家可见描述（隐藏成就未解锁时系统返回 "???"）
--   hidden      true = 隐藏成就（条件满足前 description 显示 "???"）
--   condition   条件表达式——**走"可查询谓词"**：type 字段 + 参数，谓词类型闭集
--               （见 systems/achievement.lua 的 PREDICATES 白名单）：
--                 shipping_total    { type="shipping_total", item="rice", min=100 }
--                 money             { type="money", min=1000 }
--                 mastery_level     { type="mastery_level", line="farming", min=5 }
--                 heart             { type="heart", npc="npc_shen_qingluo", min=5 }
--                 codex_progress    { type="codex_progress", min_pct=50 }
--                 day               { type="day", min=112 }
--                 festivals_attended{ type="festivals_attended", min=4 }
--                 kills             { type="kills", min=20 }
--   reward      可选：{ money=N } 或 { item="rice", count=N }（只引用既有物品）
--
-- 谓词求值在 systems/achievement.lua：依赖模块未落地时 pcall 容错按 false 处理，
-- 避免启动崩。validate_all 只认白名单谓词类型。
local M = {}

M.version = 1

-- 谓词类型闭集（validate_all 校验只认这张白名单；systems/achievement.lua 的
-- PREDICATES 求值表必须与之一一对应）。
M.predicate_types = {
  shipping_total = true,
  money = true,
  mastery_level = true,
  heart = true,
  codex_progress = true,
  day = true,
  festivals_attended = true,
  kills = true
}

M.achievements = {
  -- ── 出货（镖局收单累计）────────────────────────────────────────
  {
    id = "first_harvest",
    name = "第一桶金",
    description = "累计出货稻谷 10 份。",
    hidden = false,
    condition = { type = "shipping_total", item = "rice", min = 10 },
    reward = { money = 50 }
  },
  {
    id = "rice_100",
    name = "稻丰",
    description = "累计出货稻谷 100 份。",
    hidden = false,
    condition = { type = "shipping_total", item = "rice", min = 100 },
    reward = { money = 200 }
  },
  {
    id = "rice_500",
    name = "谷满仓",
    description = "累计出货稻谷 500 份。",
    hidden = false,
    condition = { type = "shipping_total", item = "rice", min = 500 },
    reward = { money = 1000 }
  },
  {
    id = "lingzhi_50",
    name = "药香远播",
    description = "累计出货灵芝 50 份。",
    hidden = false,
    condition = { type = "shipping_total", item = "lingzhi", min = 50 },
    reward = { item = "lingzhi", count = 5 }
  },
  {
    id = "rice_1000",
    name = "稻谷满仓",
    description = "累计出货稻谷 1000 份。",
    hidden = true,
    condition = { type = "shipping_total", item = "rice", min = 1000 },
    reward = { money = 2000 }
  },

  -- ── 金钱（state.money 实时余额）──────────────────────────────
  {
    id = "money_500",
    name = "小富即安",
    description = "持有金币达到 500 文。",
    hidden = false,
    condition = { type = "money", min = 500 },
    reward = { item = "rice", count = 5 }
  },
  {
    id = "money_2000",
    name = "小康之家",
    description = "持有金币达到 2000 文。",
    hidden = false,
    condition = { type = "money", min = 2000 },
    reward = { money = 500 }
  },
  {
    id = "money_10000",
    name = "腰缠万贯",
    description = "持有金币达到 10000 文。",
    hidden = false,
    condition = { type = "money", min = 10000 },
    reward = { money = 2000 }
  },
  {
    id = "money_5000",
    name = "一夜暴富",
    description = "持有金币达到 5000 文。",
    hidden = true,
    condition = { type = "money", min = 5000 },
    reward = { item = "lingzhi", count = 3 }
  },

  -- ── 熟练度（5 线 0–10 级）────────────────────────────────────
  {
    id = "farm_1",
    name = "初执锄犁",
    description = "务农熟练度达到 1 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "farming", min = 1 }
  },
  {
    id = "farm_3",
    name = "田间好手",
    description = "务农熟练度达到 3 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "farming", min = 3 },
    reward = { money = 100 }
  },
  {
    id = "farm_5",
    name = "精耕细作",
    description = "务农熟练度达到 5 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "farming", min = 5 },
    reward = { item = "seed_rice", count = 5 }
  },
  {
    id = "herb_3",
    name = "采药入门",
    description = "采药熟练度达到 3 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "herbalism", min = 3 }
  },
  {
    id = "alchemy_3",
    name = "丹房初成",
    description = "制药熟练度达到 3 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "alchemy", min = 3 }
  },
  {
    id = "combat_3",
    name = "拳脚小成",
    description = "武学熟练度达到 3 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "combat", min = 3 }
  },
  {
    id = "jianghu_3",
    name = "行商初成",
    description = "江湖熟练度达到 3 级。",
    hidden = false,
    condition = { type = "mastery_level", line = "jianghu", min = 3 }
  },

  -- ── 好感（relationship.points）───────────────────────────────
  {
    id = "heart_shen_5",
    name = "初结善缘",
    description = "与沈青萝的好感达到 5 点。",
    hidden = false,
    condition = { type = "heart", npc = "npc_shen_qingluo", min = 5 },
    reward = { money = 50 }
  },
  {
    id = "heart_shen_20",
    name = "青萝之友",
    description = "与沈青萝的好感达到 20 点。",
    hidden = false,
    condition = { type = "heart", npc = "npc_shen_qingluo", min = 20 }
  },
  {
    id = "heart_zhao_10",
    name = "掌柜青眼",
    description = "与赵铁算盘的好感达到 10 点。",
    hidden = false,
    condition = { type = "heart", npc = "npc_zhao_tiesuanpan", min = 10 }
  },
  {
    id = "heart_shen_50",
    name = "生死之交",
    description = "与沈青萝的好感达到 50 点。",
    hidden = true,
    condition = { type = "heart", npc = "npc_shen_qingluo", min = 50 },
    reward = { money = 500 }
  },

  -- ── 图鉴（已出货物品种类占比）────────────────────────────────
  {
    id = "codex_10",
    name = "初见天地",
    description = "图鉴收录达到 10%。",
    hidden = false,
    condition = { type = "codex_progress", min_pct = 10 }
  },
  {
    id = "codex_50",
    name = "博闻广识",
    description = "图鉴收录达到 50%。",
    hidden = false,
    condition = { type = "codex_progress", min_pct = 50 },
    reward = { money = 500 }
  },
  {
    id = "codex_90",
    name = "博物君子",
    description = "图鉴收录达到 90%。",
    hidden = true,
    condition = { type = "codex_progress", min_pct = 90 },
    reward = { money = 2000 }
  },

  -- ── 时间（clock.now().day）──────────────────────────────────
  {
    id = "day_7",
    name = "安顿下来",
    description = "在江湖中度过 7 天。",
    hidden = false,
    condition = { type = "day", min = 7 }
  },
  {
    id = "day_28",
    name = "一季收成",
    description = "在江湖中度过 28 天。",
    hidden = false,
    condition = { type = "day", min = 28 },
    reward = { money = 100 }
  },
  {
    id = "day_112",
    name = "寒来暑往",
    description = "在江湖中度过 112 天（四季一轮）。",
    hidden = false,
    condition = { type = "day", min = 112 },
    reward = { money = 1000 }
  },

  -- ── 战斗（combat.stats().kills）────────────────────────────
  {
    id = "kills_5",
    name = "初试拳脚",
    description = "累计击倒敌人 5 次。",
    hidden = false,
    condition = { type = "kills", min = 5 }
  },
  {
    id = "kills_20",
    name = "小有名气",
    description = "累计击倒敌人 20 次。",
    hidden = false,
    condition = { type = "kills", min = 20 },
    reward = { money = 200 }
  },
  {
    id = "kills_100",
    name = "百战余身",
    description = "累计击倒敌人 100 次。",
    hidden = true,
    condition = { type = "kills", min = 100 },
    reward = { money = 1000 }
  },

  -- ── 节日（经历过的不同节气数）────────────────────────────────
  {
    id = "fest_4",
    name = "四季遍历",
    description = "亲历过 4 个不同节气。",
    hidden = false,
    condition = { type = "festivals_attended", min = 4 },
    reward = { money = 300 }
  }
}

--- 按 id 索引（只读访问器）。
function M.get(id)
  for _, entry in ipairs(M.achievements) do
    if entry.id == id then
      return entry
    end
  end
  return nil
end

function M.count()
  return #M.achievements
end

--- 加载期校验：结构不变量——id 唯一、谓词类型在白名单、必填字段齐全、
-- 奖励只引用既有物品（pcall 容错：物品模块缺失时跳过交叉校验）。
function M.validate_all()
  local issues = {}
  local function is_integer(v)
    return type(v) == "number" and v == math.floor(v)
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/achievements.lua:version: unsupported version " .. tostring(M.version)
  end
  local seen = {}
  for index, entry in ipairs(M.achievements) do
    local path = "data/achievements.lua:[" .. tostring(index) .. "]"
    if type(entry) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      local id = entry.id
      if type(id) ~= "string" or id == "" then
        issues[#issues + 1] = path .. ".id: required non-empty string"
      elseif seen[id] then
        issues[#issues + 1] = path .. ".id: duplicate id " .. tostring(id)
      else
        seen[id] = true
      end
      if type(entry.name) ~= "string" or entry.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if type(entry.description) ~= "string" or entry.description == "" then
        issues[#issues + 1] = path .. ".description: required non-empty string"
      end
      -- condition 谓词白名单校验
      local cond = entry.condition
      if type(cond) ~= "table" or type(cond.type) ~= "string" then
        issues[#issues + 1] = path .. ".condition: expected table with string .type"
      else
        if not M.predicate_types[cond.type] then
          issues[#issues + 1] = path .. ".condition.type: unknown predicate " .. tostring(cond.type)
        end
        if cond.type == "shipping_total" then
          if type(cond.item) ~= "string" then
            issues[#issues + 1] = path .. ".condition.item: required string"
          end
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "money" then
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "mastery_level" then
          if type(cond.line) ~= "string" then
            issues[#issues + 1] = path .. ".condition.line: required string"
          end
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "heart" then
          if type(cond.npc) ~= "string" then
            issues[#issues + 1] = path .. ".condition.npc: required string"
          end
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "codex_progress" then
          local pct = tonumber(cond.min_pct)
          if not pct or pct < 1 or pct > 100 then
            issues[#issues + 1] = path .. ".condition.min_pct: expected number 1..100"
          end
        elseif cond.type == "day" then
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "festivals_attended" then
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        elseif cond.type == "kills" then
          if not is_integer(cond.min) or (cond.min or 0) < 1 then
            issues[#issues + 1] = path .. ".condition.min: expected integer >= 1"
          end
        end
      end
      -- reward 只引用既有物品（pcall 容错：物品模块缺失时跳过交叉校验）
      if entry.reward ~= nil then
        local reward = entry.reward
        if type(reward) ~= "table" then
          issues[#issues + 1] = path .. ".reward: expected table"
        else
          if reward.item ~= nil then
            if type(reward.item) ~= "string" then
              issues[#issues + 1] = path .. ".reward.item: expected string"
            else
              local ok, items = pcall(require, "data.items")
              local ok2, tools = pcall(require, "data.tools")
              local known = false
              if ok and type(items) == "table" and items[reward.item] then
                known = true
              end
              if not known and ok2 and type(tools) == "table" and tools[reward.item] then
                known = true
              end
              if not known then
                issues[#issues + 1] = path .. ".reward.item: unknown item " .. tostring(reward.item)
              end
            end
            if not is_integer(reward.count) or (reward.count or 0) < 1 then
              issues[#issues + 1] = path .. ".reward.count: expected integer >= 1"
            end
          end
          if reward.money ~= nil and (not is_integer(reward.money) or reward.money < 1) then
            issues[#issues + 1] = path .. ".reward.money: expected integer >= 1"
          end
        end
      end
    end
  end
  if #M.achievements ~= 30 then
    issues[#issues + 1] = "data/achievements.lua:achievements: expected 30 entries, got " .. tostring(#M.achievements)
  end
  return table.concat(issues, "\n")
end

return M
