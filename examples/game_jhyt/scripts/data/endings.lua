-- data/endings.lua：《江湖有田》多结局注册表（docs/54 §7.6，2026-10-06 2.4）。
--
-- docs/54 §7.6：4 结局 + 结局后自由模式；结局条件由"山庄声望 + 阵营声望 + 主线抉择"
-- 三线计分，纯数据配置（本文件）。本注册表是**查询面声明层**：每条结局只引用既有可查询量
-- （阵营声望 / 熟练度 / 天数），不新增系统、不接管 quest 五态机——既有任务驱动的结局
-- （data/quests/ending_*.lua：归田/承业/雪恨/断念）保持原样，这里把同一批门槛
-- （docs/33 §8.2：归田=村熟识 20、承业=派重视 60）落成可由 systems/endings.lua
-- 自动求值、once 解锁的"结局图鉴"记录。
--
-- 5 条结局各占一条独立轴，互不覆盖，便于逐档边界验收：
--   归田 village 声望轴 / 入仕 sect 声望轴 / 神医 制药熟练度轴 /
--   武林 武学熟练度轴 / 隐世 天数轴（四季一轮）。
--
-- 字段：
--   id          稳定标识（全局唯一；与 data/quests/ending_*.lua 的 id 对齐两条主线结局）
--   name        玩家可见结局名
--   description 玩家可见结局描述
--   cond        条件表达式——**走"可查询谓词"闭集**（M.predicate_types 白名单；
--               求值在 systems/endings.lua，依赖模块缺失时 pcall 容错按 false 处理）：
--                 reputation    { type="reputation", faction="village", min=20 }
--                 mastery_level { type="mastery_level", line="alchemy", min=5 }
--                 day           { type="day", min=112 }
--
-- validate_all 守结构不变量：id 唯一、谓词类型白名单、阵营/线条目引用存在、门槛落在可达区间。
local M = {}

M.version = 1

-- 谓词类型闭集（validate_all 只认这张白名单；systems/endings.lua 的 PREDICATES
-- 求值表必须与之一一对应）。
M.predicate_types = {
  reputation = true,
  mastery_level = true,
  day = true
}

M.endings = {
  -- ── 归田：云泽村声望到熟识（20）——与 docs/33 §8.2「归田结局的门槛」逐格对齐 ──
  {
    id = "ending_field",
    name = "归田",
    description = "云泽村声望达到熟识（20）。放下武名，回山庄把这一亩药田种成模样。",
    cond = { type = "reputation", faction = "village", min = 20 }
  },
  -- ── 入仕（承业）：青云门派声望到重视（60）——与 docs/33 §8.2「承业结局的门槛」对齐 ──
  {
    id = "ending_sect",
    name = "入仕",
    description = "青云门派声望达到重视（60）。接过掌门信物，从此执掌山庄、入世行道。",
    cond = { type = "reputation", faction = "sect", min = 60 }
  },
  -- ── 神医：制药熟练度 5 级——以医术闻名乡里（熟练度轴，docs/33 §7.1）─────────
  {
    id = "ending_healer",
    name = "神医",
    description = "制药熟练度达到 5 级。悬壶济世，一方百姓称你一声神医。",
    cond = { type = "mastery_level", line = "alchemy", min = 5 }
  },
  -- ── 武林：武学熟练度 5 级——武道小成（熟练度轴，docs/33 §7.1）──────────────
  {
    id = "ending_martial",
    name = "武林",
    description = "武学熟练度达到 5 级。拳脚兵刃小成，闯出一号江湖名声。",
    cond = { type = "mastery_level", line = "combat", min = 5 }
  },
  -- ── 隐世：度过 112 天（四季一轮）——长线时间结局 ─────────────────────────
  {
    id = "ending_hermit",
    name = "隐世",
    description = "在江湖中度过 112 天（四季一轮）。寒来暑往，挂冠归隐，不问是非。",
    cond = { type = "day", min = 112 }
  }
}

--- 按 id 索引（只读访问器）；未知 id 返回 nil。
function M.get(id)
  for _, entry in ipairs(M.endings) do
    if entry.id == id then
      return entry
    end
  end
  return nil
end

function M.count()
  return #M.endings
end

-- 阵营 id → 是否存在（data/relationships.factions）；pcall 容错：关系数据模块缺失时
-- 返回 nil，引用校验跳过交叉比对（避免加载顺序把启动打崩）。
local function known_faction(faction_id)
  local ok, rel = pcall(require, "data.relationships")
  if not ok or type(rel) ~= "table" or type(rel.factions) ~= "table" then
    return nil
  end
  return rel.factions[faction_id]
end

-- 熟练度线条目 → 是否存在（data/mastery.lines）；pcall 容错。
local function known_line(line_id)
  local ok, m = pcall(require, "data.mastery")
  if not ok or type(m) ~= "table" or type(m.lines) ~= "table" then
    return nil
  end
  return m.lines[line_id]
end

--- 加载期校验：结构不变量——id 唯一、谓词类型在白名单、必填字段齐全、
-- 阵营/线条目引用存在、门槛落在可达区间。返回 "" = 通过。
function M.validate_all()
  local issues = {}
  local function is_integer(v)
    return type(v) == "number" and v == math.floor(v)
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/endings.lua:version: unsupported version " .. tostring(M.version)
  end
  local seen = {}
  for index, entry in ipairs(M.endings) do
    local path = "data/endings.lua:[" .. tostring(index) .. "]"
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
      local cond = entry.cond
      if type(cond) ~= "table" or type(cond.type) ~= "string" then
        issues[#issues + 1] = path .. ".cond: expected table with string .type"
      else
        if not M.predicate_types[cond.type] then
          issues[#issues + 1] = path .. ".cond.type: unknown predicate " .. tostring(cond.type)
        end
        if cond.type == "reputation" then
          if type(cond.faction) ~= "string" then
            issues[#issues + 1] = path .. ".cond.faction: required string"
          else
            local faction = known_faction(cond.faction)
            if faction == nil then
              issues[#issues + 1] = path .. ".cond.faction: unknown faction " .. tostring(cond.faction)
            elseif not is_integer(cond.min) or cond.min < 0 then
              issues[#issues + 1] = path .. ".cond.min: expected integer >= 0"
            elseif type(faction.max) == "number" and cond.min > faction.max then
              issues[#issues + 1] = path .. ".cond.min: exceeds faction max " .. tostring(faction.max)
            end
          end
        elseif cond.type == "mastery_level" then
          if type(cond.line) ~= "string" then
            issues[#issues + 1] = path .. ".cond.line: required string"
          else
            if known_line(cond.line) == nil then
              issues[#issues + 1] = path .. ".cond.line: unknown mastery line " .. tostring(cond.line)
            end
          end
          if not is_integer(cond.min) or cond.min < 1 or cond.min > 10 then
            issues[#issues + 1] = path .. ".cond.min: expected integer 1..10"
          end
        elseif cond.type == "day" then
          if not is_integer(cond.min) or cond.min < 1 then
            issues[#issues + 1] = path .. ".cond.min: expected integer >= 1"
          end
        end
      end
    end
  end
  -- 固定 5 条结局（docs/54 §7.6 起步规模 4–6，本版落 5 条独立轴）。
  if #M.endings ~= 5 then
    issues[#issues + 1] = "data/endings.lua:endings: expected 5 entries, got " .. tostring(#M.endings)
  end
  return table.concat(issues, "\n")
end

return M
