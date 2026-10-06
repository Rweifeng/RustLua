-- data/mine.lua：《江湖有田》青云矿脉 30 层数据（阶段2，docs/33 §4.6 / docs/54 §4.5，
-- 2026-10-06 1.2）。
--
-- 纯数据注册表（与 data/crops.lua / data/farm.lua 同约定）：systems/mine.lua 只消费本表，
-- 新增/调层只改这里，不改系统与 Rust 核心。
--
-- 每层字段（闭集）：
--   floor        层号 1..30（1=最浅，30=最深）
--   name         显示名（如"青云一层·青石阶"）
--   ore_pool     本层矿石掉落池：{ { item = 矿石id, chance = 权重 } }。
--                systems/mine.lua 按权重加权抽取，一次挖矿恒出 1 块矿石。
--   gem_pool     稀有宝石池：{ { item = 宝石id, chance = 独立概率(0<chance<=1) } }。
--                一次挖矿按每条独立掷骰命中即额外入包（稀有加成，与矿石同次）。
--   difficulty   1..10，随层深递增；descend 下到该层的体力消耗 = difficulty。
--   energy_cost  挖矿体力消耗，随层深递增（一次 mine() 扣这么多体力）。
--   elevator     true = 电梯层（5/10/15/20/25/30）：解锁后可直达。
--
-- 矿石分级深度口径：
--   * 前 10 层（1–10）：铜(copper_ore)/铁(iron_ore 复用主表)；
--   * 11–20 层：银(silver_ore)/金(gold_ore)；
--   * 21–30 层：金(gold_ore)/寒铁(cold_iron_ore)，宝石池出现高阶宝石。
-- 宝石池 11 层起才出现（浅层只出矿石），越深宝石池越稀有越好。
local M = {}

M.version = 1
M.FLOOR_COUNT = 30

-- 电梯层（每 5 层一停）：解锁后 systems/mine.lua 的 elevator() 可直达。
local ELEVATORS = { [5] = true, [10] = true, [15] = true, [20] = true, [25] = true, [30] = true }

-- 每层的风味名（手工写 30 个，按深度分区）。
local NAMES = {
  "青石阶", "碎石道", "积砂巷", "锈铁廊", "铜绿台",
  "方竖井", "滴水窟", "断矿脉", "黑渣坡", "废采洞",
  "银霜壁", "寒泉眼", "回响井", "萤火窟", "金砂坳",
  "暗河湾", "钟乳厅", "风啸峡", "碎晶隙", "沉银底",
  "金髓洞", "玄冰缝", "锁龙栈", "孤星厅", "冻玉渊",
  "寒光壁", "陨铁隙", "九幽井", "翡翠窟", "天外层"
}

-- 难度/挖矿消耗随层深线性递增：difficulty = ceil(f/3)（1..10），
-- energy_cost = 2 + difficulty（3..12）。下潜越深，下一层与挖一镐都越费体力。
local function difficulty_of(f)
  return math.min(10, math.max(1, math.ceil(f / 3)))
end

-- 矿石池按深度区域：浅(铜铁) → 中(银金) → 深(金寒铁)。chance 为加权权重。
local function ore_pool_of(f)
  if f <= 5 then
    -- 浅层：铜多铁少
    return { { item = "copper_ore", chance = 60 }, { item = "iron_ore", chance = 30 } }
  elseif f <= 10 then
    -- 前 10 层后段：铁反超铜
    return { { item = "copper_ore", chance = 40 }, { item = "iron_ore", chance = 50 } }
  elseif f <= 15 then
    -- 中层前段：银主金辅
    return { { item = "silver_ore", chance = 50 }, { item = "gold_ore", chance = 20 } }
  elseif f <= 20 then
    -- 中层后段：金权重上升
    return { { item = "silver_ore", chance = 40 }, { item = "gold_ore", chance = 35 } }
  elseif f <= 25 then
    -- 深层前段：金主寒铁辅
    return { { item = "gold_ore", chance = 40 }, { item = "cold_iron_ore", chance = 30 } }
  else
    -- 最深处：寒铁为主、金为辅
    return { { item = "gold_ore", chance = 30 }, { item = "cold_iron_ore", chance = 50 } }
  end
end

-- 宝石池：1–10 层空（只出矿石）；11 层起出现，越深越稀有越好。
-- chance 为独立概率（0<chance<=1），命中即额外入包。
local function gem_pool_of(f)
  if f <= 10 then
    return {}
  elseif f <= 15 then
    return { { item = "amber", chance = 0.05 }, { item = "agate", chance = 0.04 } }
  elseif f <= 20 then
    return {
      { item = "agate", chance = 0.04 },
      { item = "jade", chance = 0.03 },
      { item = "amethyst", chance = 0.015 }
    }
  elseif f <= 25 then
    return {
      { item = "jade", chance = 0.03 },
      { item = "amethyst", chance = 0.02 },
      { item = "emerald", chance = 0.01 }
    }
  else
    return {
      { item = "emerald", chance = 0.015 },
      { item = "diamond", chance = 0.008 },
      { item = "luminous_pearl", chance = 0.004 }
    }
  end
end

-- 构建 30 层（索引即层号）。
M.floors = {}
for f = 1, M.FLOOR_COUNT do
  local diff = difficulty_of(f)
  M.floors[f] = {
    floor = f,
    name = "青云" .. f .. "层·" .. (NAMES[f] or "深穴"),
    ore_pool = ore_pool_of(f),
    gem_pool = gem_pool_of(f),
    difficulty = diff,
    energy_cost = 2 + diff,
    elevator = ELEVATORS[f] == true
  }
end

--- 取某层定义（越界返回 nil）。
function M.get(f)
  f = tonumber(f)
  if not f or f < 1 or f > M.FLOOR_COUNT then
    return nil
  end
  return M.floors[f]
end

--- 是否电梯层。
function M.is_elevator(f)
  local def = M.get(f)
  return def ~= nil and def.elevator == true
end

--- 全部电梯层（升序）。
function M.elevator_floors()
  local out = {}
  for f = 1, M.FLOOR_COUNT do
    if M.floors[f].elevator then
      out[#out + 1] = f
    end
  end
  return out
end

-- ---------- 加载期校验 ----------

local function is_non_empty_string(v)
  return type(v) == "string" and v ~= ""
end

-- 已知矿石/宝石 id（矿石含主表复用的 iron_ore；宝石全在 items_mine）。
local KNOWN_ORES = {
  copper_ore = true, iron_ore = true,
  silver_ore = true, gold_ore = true, cold_iron_ore = true
}
local KNOWN_GEMS = {
  amber = true, agate = true, jade = true,
  amethyst = true, emerald = true, diamond = true, luminous_pearl = true
}

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/mine.lua: version: unsupported " .. tostring(M.version)
  end
  if #M.floors ~= M.FLOOR_COUNT then
    issues[#issues + 1] = "data/mine.lua: 层数须为 " .. M.FLOOR_COUNT .. "，现有 " .. #M.floors
  end
  for f = 1, M.FLOOR_COUNT do
    local def = M.floors[f]
    local path = "data/mine.lua:floors[" .. tostring(f) .. "]"
    if type(def) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if def.floor ~= f then
        issues[#issues + 1] = path .. ".floor: expected " .. tostring(f)
      end
      if not is_non_empty_string(def.name) then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      -- ore_pool：非空数组，每项 { item=已知矿石, chance>0 }
      if type(def.ore_pool) ~= "table" or #def.ore_pool == 0 then
        issues[#issues + 1] = path .. ".ore_pool: expected non-empty array"
      else
        for i, entry in ipairs(def.ore_pool) do
          local p = path .. ".ore_pool[" .. tostring(i) .. "]"
          if not KNOWN_ORES[entry.item] then
            issues[#issues + 1] = p .. ".item: unknown ore " .. tostring(entry.item)
          end
          local chance = tonumber(entry.chance)
          if not chance or chance <= 0 then
            issues[#issues + 1] = p .. ".chance: expected positive number"
          end
        end
      end
      -- gem_pool：可为空；非空时每项 { item=已知宝石, 0<chance<=1 }
      if type(def.gem_pool) ~= "table" then
        issues[#issues + 1] = path .. ".gem_pool: expected array"
      else
        for i, entry in ipairs(def.gem_pool) do
          local p = path .. ".gem_pool[" .. tostring(i) .. "]"
          if not KNOWN_GEMS[entry.item] then
            issues[#issues + 1] = p .. ".item: unknown gem " .. tostring(entry.item)
          end
          local chance = tonumber(entry.chance)
          if not chance or chance <= 0 or chance > 1 then
            issues[#issues + 1] = p .. ".chance: expected number in (0,1]"
          end
        end
      end
      local diff = tonumber(def.difficulty)
      if not diff or diff ~= math.floor(diff) or diff < 1 or diff > 10 then
        issues[#issues + 1] = path .. ".difficulty: expected integer 1..10"
      end
      local cost = tonumber(def.energy_cost)
      if not cost or cost ~= math.floor(cost) or cost < 1 then
        issues[#issues + 1] = path .. ".energy_cost: expected positive integer"
      end
      -- 电梯层必须恰好落在 5/10/15/20/25/30。
      local expect_elevator = (f % 5 == 0)
      if (def.elevator == true) ~= expect_elevator then
        issues[#issues + 1] = path .. ".elevator: floor " .. tostring(f)
          .. " 应" .. (expect_elevator and "为" or "不为") .. "电梯层"
      end
    end
  end
  if #issues == 0 then
    return ""
  end
  return table.concat(issues, "\n")
end

return M
