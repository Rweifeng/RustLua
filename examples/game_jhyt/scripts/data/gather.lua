-- data/gather.lua：《江湖有田》野外采集点注册表（M19-01 纯数据）。
--
-- 与 data/maps.lua / data/enemies.lua 同约定：新增采集点 = 本文件加一条记录，
-- 无需改动 gather.lua / interaction.lua / Rust 核心。
--
-- 字段：
--   id            稳定标识（全局唯一；map.state("gather") 的持久键）
--   map           所属地图 id（引用 data/maps.lua；采集只在该图生效）
--   cell          采集点所在格 {x, y}（cell 0-index、y 朝上；玩家需站在其相邻格
--                 并面向它才能采集。该格由 headless 验收校验为可行走格）
--   item          产出物品 id（引用 data/items.lua 或 data/tools.lua）
--   count         单次采集数量（正整数）
--   minutes       本次采集计入 state.labor_minutes 的劳作分钟（>= 0；
--                 与农田 MVP 取舍一致：**不推进 clock**）
--   requires      可选：需要的工具/物品 id（引用 data.items / data.tools）。
--                 玩家 tool/weapon 装备槽里没有该物时不可采集（文案给出原因）。
--                 例：矿镐未实装，故矿石点用技能分支 requires = "hoe" 表达
--                 「需要趁手器具」的占位 —— 数据驱动，改数据即可换条件。
--   respawn_days  采集后多少天可再采（>= 0）。0 或缺省 = 一次性（采完永久消失）。
--                 1 = 次日刷新（与 7 天循环/睡觉日结算配合）。
--
-- 刷新语义（gather.lua 实现，逐条确定）：
--   * 节点记录 { day = 采集发生的游戏日 }；query/act 时若
--     clock.now().day - day >= respawn_days 则视为已刷新（惰性清除，幂等）。
--   * respawn_days = 0 的节点永不刷新（一次性资源）。
--
-- 数值口径（M19-01 校核）：灵芝 crop.growth_days = 7、稻谷 4，农田当日不可收，
-- 因此**单日通关路线的药材/稻谷必须由山道采集供给**：
--   * 灵芝：wild_herb_1（2，日刷）+ wild_poison_herb_1（3，日刷，需锄头）= 5/日
--     + manor_wild_herb_1（1，一次性）= 单日 6，覆盖
--     「药庐订单 2 + 主线交药材 2 + 炼回复药 2」。
--   * 稻谷：wild_berry_1（2，一次性）+ 新档自带 2 = 4，覆盖
--     「市集订单 2 + 主线二段 2」。
--   多日路线则由农田补足（各作物 1 个有效成长日 = 1 天）。
local M = {}

M.version = 1

M.nodes = {
  -- ---- 青云山道 mountain_path（M19-01 垂直切片：进山采集）----
  -- 药草丛 (12,11)：对应 scenes/mountain_path.scene 的 HerbCluster 道具格。
  -- 玩家可站 (12,10)/(12,12)/(13,11) 面向它；次日刷新。
  {
    id = "wild_herb_1",
    map = "mountain_path",
    cell = { x = 12, y = 11 },
    item = "lingzhi",
    count = 2,
    minutes = 20,
    respawn_days = 1
  },
  -- 野果丛 (9,16)：对应 Berry 道具格；一次性（尝过就没了），2 斗野谷。
  {
    id = "wild_berry_1",
    map = "mountain_path",
    cell = { x = 9, y = 16 },
    item = "rice",
    count = 2,
    minutes = 15,
    respawn_days = 0
  },
  -- 柴堆 (15,3)：对应 Firewood 道具格；一次性（木料只能捡一回）。
  {
    id = "wild_firewood_1",
    map = "mountain_path",
    cell = { x = 15, y = 3 },
    item = "timber",
    count = 1,
    minutes = 25,
    respawn_days = 0
  },
  -- 矿石堆 (28,17)：需要趁手器具（requires 分支）；一次性。
  -- 山道暂无矿镐道具，故用数据驱动的 requires = "hoe" 表达门槛，
  -- 缺前置时 prompt 给出明确原因（不静默失败）。
  {
    id = "wild_ore_1",
    map = "mountain_path",
    cell = { x = 28, y = 17 },
    item = "timber",
    count = 1,
    minutes = 30,
    requires = "hoe",
    respawn_days = 0
  },
  -- 毒草丛 (29,15)：对应 PoisonHerb 道具格；次日刷新，同样需要器具（3 株，成片）。
  {
    id = "wild_poison_herb_1",
    map = "mountain_path",
    cell = { x = 29, y = 15 },
    item = "lingzhi",
    count = 3,
    minutes = 20,
    requires = "hoe",
    respawn_days = 1
  },
  -- ---- 山庄 manor（药田外围的一次性野采，补足主线药材）----
  -- 山庄药田西侧石路旁 (12,3)：与 places.manor_gate(16,2) 同一片可行走区。
  {
    id = "manor_wild_herb_1",
    map = "manor",
    cell = { x = 12, y = 3 },
    item = "lingzhi",
    count = 1,
    minutes = 15,
    respawn_days = 0
  }
}

function M.list(map_id)
  local out = {}
  for _, node in ipairs(M.nodes) do
    if map_id == nil or node.map == map_id then
      out[#out + 1] = node
    end
  end
  return out
end

function M.get(id)
  for _, node in ipairs(M.nodes) do
    if node.id == id then
      return node
    end
  end
  return nil
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/gather.lua: version: unsupported version " .. tostring(M.version)
  end
  local maps = require("data.maps")
  local items = require("data.items")
  local tools = require("data.tools")
  if type(M.nodes) ~= "table" then
    issues[#issues + 1] = "data/gather.lua:nodes: expected array"
    return false, issues
  end
  local seen = {}
  for index, node in ipairs(M.nodes) do
    local path = "data/gather.lua:nodes[" .. tostring(index) .. "]"
    if type(node) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      local id = node.id
      if type(id) ~= "string" or id == "" then
        issues[#issues + 1] = path .. ".id: required non-empty string"
      elseif seen[id] then
        issues[#issues + 1] = path .. ".id: duplicate object id " .. id
      else
        seen[id] = true
      end
      if type(node.map) ~= "string" or not maps.get(node.map) then
        issues[#issues + 1] = path .. ".map: unknown map " .. tostring(node.map)
      end
      local cell = node.cell
      if type(cell) ~= "table" or type(cell.x) ~= "number" or type(cell.y) ~= "number" then
        issues[#issues + 1] = path .. ".cell: expected {x, y} numbers"
      elseif cell.x < 0 or cell.x >= 40 or cell.y < 0 or cell.y >= 22 then
        issues[#issues + 1] = path .. ".cell: out of map bounds"
      end
      if items[node.item] == nil and tools[node.item] == nil then
        issues[#issues + 1] = path .. ".item: unknown item " .. tostring(node.item)
      end
      if type(node.count) ~= "number" or node.count <= 0 or node.count ~= math.floor(node.count) then
        issues[#issues + 1] = path .. ".count: expected positive integer"
      end
      if type(node.minutes) ~= "number" or node.minutes < 0 then
        issues[#issues + 1] = path .. ".minutes: expected non-negative number"
      end
      if node.requires ~= nil then
        if type(node.requires) ~= "string" or node.requires == "" then
          issues[#issues + 1] = path .. ".requires: expected non-empty string when present"
        elseif items[node.requires] == nil and tools[node.requires] == nil then
          issues[#issues + 1] = path .. ".requires: unknown item/tool " .. tostring(node.requires)
        end
      end
      if node.respawn_days ~= nil then
        local days = node.respawn_days
        if type(days) ~= "number" or days < 0 or days ~= math.floor(days) then
          issues[#issues + 1] = path .. ".respawn_days: expected non-negative integer"
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
