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
--   upgrade_cost  升到下一级的消耗（level→level+1）：{ {item,count}... }
--   station       制作设施类型（nil = 纯建筑，无制作能力）；引用 data/recipes
local M = {}

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
  -- 茶棚：2x2 纯建筑（装饰/占位，验证多建筑共存与占用互斥）。
  tea_shed = {
    name = "茶棚",
    w = 2,
    h = 2,
    ground_tiles = { 1, 3 },
    levels = {
      { sprite = "assets/art/buildings/build_foundation.png", size = "128x64" },
      { sprite = "assets/art/buildings/build_tea_shed.png", size = "112x96" }
    },
    upgrade_cost = { { item = "timber", count = 3 } },
    station = nil
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
