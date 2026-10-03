-- data/buildings.lua：《江湖有田》建筑注册表（M18-02 纯数据）。
--
-- 新增一个建筑 = 本文件加一条记录（+ 可选 data/recipes.lua 配方），无需改动
-- building.lua / crafting.lua 与 Rust 核心（验收项④：纯数据新增无需改核心）。
--
-- 字段：
--   name          显示名
--   w / h         footprint 格数（锚点格为左下角，向右/向上扩展 w×h 格）
--   ground_tiles  允许放置的地表 tile 值集合（1=GRASS 2=DIRT 3=STONE 4=WALL
--                 5=ROOF 6=WATER，与 tools/gen_manor_scene.py 列映射一致）；
--                 可放置的地表之外一律 ground 冲突
--   levels        等级数组（1..N 即最大等级 N）。每级：
--                   sprite  Sprite 组件贴图路径
--                   size    "宽x高"（像素，与贴图一致）
--                   social_bonus  可选（M34-05）：建到这一档之后给"社交收益"的加成比例
--                   （0.1 = +10%，累加到 1..当前等级为止）。只有一个消费者
--                   `player/relationship.lua` 的 `social_multiplier()`，所以这一格数字
--                   就是 docs/33 §7.2 "社交收益 +10%" 的那一格，别在别处再写一份。
--   upgrade_cost  升到下一级的消耗（level→level+1）：{ {item,count}... }
--                 **每升一级都收这一份**（字段本来就没按等级分档）；三档建筑的第二跳
--                 也走同一份成本，要分档定价得先给这个字段加等级维度。
--   station       制作设施类型（nil = 纯建筑，无制作能力）；引用 data/recipes
local M = {}

-- 等级行的键闭集：写一个没人读的键（`social_bonus` 少个 s）不会崩，只会让那一档的加成
-- 静默消失，所以在加载期就报出来（与 data/skills.lua 的招式键白名单同规矩）。
local LEVEL_KEYS = { sprite = true, size = true, social_bonus = true }

M.version = 1

M.buildings = {
  -- 药庐：2x2 制作设施，level2 解锁行气散配方。
  herb_hut = {
    name = "药庐",
    w = 2,
    h = 2,
    ground_tiles = { 1, 3 },
    levels = {
      { sprite = "assets/art/buildings/build_foundation.png", size = "128x64" },
      { sprite = "assets/art/buildings/build_herb_hut.png", size = "128x112" }
    },
    upgrade_cost = { { item = "timber", count = 4 } },
    station = "herb"
  },
  -- 库房：3x2 纯建筑（不可制作，验证 stationless 路径）。
  storehouse = {
    name = "库房",
    w = 3,
    h = 2,
    ground_tiles = { 1, 3 },
    levels = {
      { sprite = "assets/art/buildings/build_scaffold.png", size = "128x96" },
      { sprite = "assets/art/buildings/build_storehouse.png", size = "128x112" }
    },
    upgrade_cost = { { item = "timber", count = 6 } },
    station = nil
  },
  -- 茶棚（M34-05）：2x2 制作设施，三档对应 docs/33 §7.2 的三格——
  -- 1 级"解锁茶饮"（station = "tea"，配方 data/recipes.lua:tea_spring_brew）、
  -- 2 级"社交收益 +10%"（levels[2].social_bonus）、3 级"解锁节气事件"
  -- （解锁条件写在 data/festivals.lua 的 teafair 行上：要几级是那条事件的属性，建筑只管自己有几级）。
  -- 第三档暂时沿用第二档的贴图（112x96）：本任务的范围是数值与接线，美术另案，
  -- 升级时"看不出变化"是这条已知限制，不是 bug。
  tea_shed = {
    name = "茶棚",
    w = 2,
    h = 2,
    ground_tiles = { 1, 3 },
    levels = {
      { sprite = "assets/art/buildings/build_foundation.png", size = "128x64" },
      { sprite = "assets/art/buildings/build_tea_shed.png", size = "112x96", social_bonus = 0.1 },
      { sprite = "assets/art/buildings/build_tea_shed.png", size = "112x96" }
    },
    upgrade_cost = { { item = "timber", count = 3 } },
    station = "tea"
  },
  -- v0.2 铁匠炉：2x2 制作设施，level2 解锁精铁锭。
  forge = {
    name = "铁匠炉",
    w = 2,
    h = 2,
    ground_tiles = { 1, 3, 4 },
    levels = {
      { sprite = "assets/art/buildings/build_foundation.png", size = "128x64" },
      { sprite = "assets/art/buildings/build_forge_stand.png", size = "128x112" }
    },
    upgrade_cost = { { item = "timber", count = 5 }, { item = "iron_ingot", count = 2 } },
    station = "forge"
  },
  -- v0.2 膳房：2x2 制作设施，level1 煮饭团，level2 炖药汤。
  kitchen = {
    name = "膳房",
    w = 2,
    h = 2,
    ground_tiles = { 1, 3 },
    levels = {
      { sprite = "assets/art/buildings/build_scaffold.png", size = "128x96" },
      { sprite = "assets/art/buildings/build_tea_shed.png", size = "112x96" }
    },
    upgrade_cost = { { item = "timber", count = 4 } },
    station = "kitchen"
  }
}

function M.get(id)
  return M.buildings[id]
end

-- 建筑 id 稳定排序（诊断/遍历确定性）。
function M.ids()
  local ids = {}
  for id in pairs(M.buildings) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

function M.max_level(def)
  return def and #(def.levels or {}) or 0
end

-- 「这座建筑建到过几级」记在 core.state 的 vars 里（`building_level_<id>`）：由
-- systems/building.lua 在放置/升级/进图时写，随既有 state provider 进存档，不需要新 provider。
-- 为什么不直接读建筑记录：记录按地图存进 map.state，玩家在云泽村送礼时读不到山庄里的茶棚，
-- 而"茶棚建到过几级"是一件庄园属性，不该随站位变化。取**历史最高**：拆了不回收——
-- 与节气解锁同一口径（解锁过的事件不再锁回去）。
local LEVEL_VAR_PREFIX = "building_level_"

--- 该建筑的等级 var 名（写方 systems/building.lua，读方 relationship.lua / festival.lua）。
function M.level_var(building_id)
  return LEVEL_VAR_PREFIX .. tostring(building_id)
end

--- 1..level 各级 `social_bonus` 的累加（没写就是 0）：关系分乘子的数据来源。
--- 上限取建筑自己的 max level，传入超界等级不会多算。
function M.social_bonus_at(def, level)
  if type(def) ~= "table" or type(def.levels) ~= "table" then
    return 0
  end
  local top = math.floor(tonumber(level) or 0)
  if top < 1 then
    return 0
  end
  if top > #def.levels then
    top = #def.levels
  end
  local total = 0
  for index = 1, top do
    local entry = def.levels[index]
    local bonus = type(entry) == "table" and tonumber(entry.social_bonus) or nil
    if bonus then
      total = total + bonus
    end
  end
  return total
end

function M.validate_all()
  local issues = {}
  local items = require("data.items")
  local tools = require("data.tools")
  local function item_exists(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/buildings.lua: version: unsupported version " .. tostring(M.version)
  end
  local seen_station = {}
  for id, def in pairs(M.buildings) do
    local path = "data/buildings.lua:buildings." .. tostring(id)
    if type(def) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if type(def.name) ~= "string" or def.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      for _, field in ipairs({ "w", "h" }) do
        local value = def[field]
        if type(value) ~= "number" or value < 1 or value ~= math.floor(value) then
          issues[#issues + 1] = path .. "." .. field .. ": expected positive integer"
        end
      end
      if type(def.ground_tiles) ~= "table" or #def.ground_tiles == 0 then
        issues[#issues + 1] = path .. ".ground_tiles: expected non-empty array"
      else
        for index, tile in ipairs(def.ground_tiles) do
          if type(tile) ~= "number" or tile < 1 or tile > 6 or tile ~= math.floor(tile) then
            issues[#issues + 1] = path .. ".ground_tiles[" .. tostring(index) .. "]: expected tile 1..6"
          end
        end
      end
      if type(def.levels) ~= "table" or #def.levels == 0 then
        issues[#issues + 1] = path .. ".levels: expected non-empty array"
      else
        for index, level in ipairs(def.levels) do
          local lpath = path .. ".levels[" .. tostring(index) .. "]"
          if type(level) ~= "table" then
            issues[#issues + 1] = lpath .. ": expected object"
          else
            if type(level.sprite) ~= "string" or level.sprite == "" then
              issues[#issues + 1] = lpath .. ".sprite: required non-empty string"
            end
            if type(level.size) ~= "string" or not level.size:match("^%d+x%d+$") then
              issues[#issues + 1] = lpath .. ".size: expected \"WxH\""
            end
            for key in pairs(level) do
              if not LEVEL_KEYS[key] then
                issues[#issues + 1] = lpath .. "." .. tostring(key) .. ": unknown level key"
              end
            end
            -- 社交加成写在这一格上（M34-05）：0 或负数就是"这一档其实没加成"，
            -- 大于 1 会把关系分翻到两倍以上——两者都该在加载期挡住，而不是等玩家发现。
            if level.social_bonus ~= nil then
              local bonus = tonumber(level.social_bonus)
              if not bonus or bonus <= 0 or bonus > 1 then
                issues[#issues + 1] = lpath .. ".social_bonus: expected number in (0, 1]"
              end
            end
          end
        end
      end
      if type(def.upgrade_cost) ~= "table" then
        issues[#issues + 1] = path .. ".upgrade_cost: expected array"
      else
        for index, cost in ipairs(def.upgrade_cost) do
          local cpath = path .. ".upgrade_cost[" .. tostring(index) .. "]"
          if type(cost) ~= "table" then
            issues[#issues + 1] = cpath .. ": expected object"
          else
            if not item_exists(cost.item) then
              issues[#issues + 1] = cpath .. ".item: unknown item " .. tostring(cost.item)
            end
            if type(cost.count) ~= "number" or cost.count <= 0 or cost.count ~= math.floor(cost.count) then
              issues[#issues + 1] = cpath .. ".count: expected positive integer"
            end
          end
        end
      end
      if def.station ~= nil then
        if type(def.station) ~= "string" or def.station == "" then
          issues[#issues + 1] = path .. ".station: expected non-empty string when present"
        elseif seen_station[def.station] then
          issues[#issues + 1] = path .. ".station: duplicate station type " .. def.station
        else
          seen_station[def.station] = id
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
