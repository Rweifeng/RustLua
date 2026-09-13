-- data/recipes.lua：《江湖有田》制作配方注册表（M18-02 纯数据）。
--
-- 新增配方 = 本文件加一条记录，无需改动 crafting.lua 与 Rust 核心（验收项④）。
-- 配方必须挂在建筑的 station 上，且满足建筑等级门槛。
--
-- 字段：
--   name       显示名
--   station    设施类型（必须等于某建筑 data/buildings.lua 的 station）
--   inputs     消耗材料：{ {item, count}... }（item 引用 data/items.lua / data.tools.lua）
--   output     产出：{ item, count }
--   minutes    制作耗时（游戏内分钟；由 clock 推导完成时刻，跨天自动进位）
--   min_level  设施最低等级门槛（1..建筑最大等级）
local M = {}

M.version = 1

M.recipes = {
  pill_refine = {
    name = "炼制回复药",
    station = "herb",
    inputs = { { item = "lingzhi", count = 2 } },
    output = { item = "heal_pill", count = 1 },
    minutes = 120,
    min_level = 1
  },
  tonic_refine = {
    name = "熬制行气散",
    station = "herb",
    inputs = { { item = "lingzhi", count = 1 }, { item = "rice", count = 2 } },
    output = { item = "herb_tonic", count = 1 },
    minutes = 180,
    min_level = 2
  }
}

function M.get(id)
  return M.recipes[id]
end

function M.ids()
  local ids = {}
  for id in pairs(M.recipes) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

-- 某个 station 的可用配方 id（按 min_level、id 稳定排序）。
function M.for_station(station)
  local ids = {}
  for id, def in pairs(M.recipes) do
    if def.station == station then
      ids[#ids + 1] = id
    end
  end
  table.sort(ids, function(a, b)
    local la = M.recipes[a].min_level or 1
    local lb = M.recipes[b].min_level or 1
    if la ~= lb then
      return la < lb
    end
    return a < b
  end)
  return ids
end

function M.validate_all()
  local issues = {}
  local items = require("data.items")
  local tools = require("data.tools")
  local buildings = require("data.buildings")
  local function item_exists(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  local stations = {}
  local max_level = {}
  for id, def in pairs(buildings.buildings) do
    if type(def) == "table" and type(def.station) == "string" then
      stations[def.station] = id
      max_level[def.station] = buildings.max_level(def)
    end
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/recipes.lua: version: unsupported version " .. tostring(M.version)
  end
  for id, def in pairs(M.recipes) do
    local path = "data/recipes.lua:recipes." .. tostring(id)
    if type(def) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if type(def.name) ~= "string" or def.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if type(def.station) ~= "string" or def.station == "" then
        issues[#issues + 1] = path .. ".station: required non-empty string"
      elseif not stations[def.station] then
        issues[#issues + 1] = path .. ".station: unknown station " .. tostring(def.station)
      end
      if type(def.inputs) ~= "table" or #def.inputs == 0 then
        issues[#issues + 1] = path .. ".inputs: expected non-empty array"
      else
        for index, entry in ipairs(def.inputs) do
          local ipath = path .. ".inputs[" .. tostring(index) .. "]"
          if type(entry) ~= "table" then
            issues[#issues + 1] = ipath .. ": expected object"
          else
            if not item_exists(entry.item) then
              issues[#issues + 1] = ipath .. ".item: unknown item " .. tostring(entry.item)
            end
            if type(entry.count) ~= "number" or entry.count <= 0 or entry.count ~= math.floor(entry.count) then
              issues[#issues + 1] = ipath .. ".count: expected positive integer"
            end
          end
        end
      end
      local output = def.output
      if type(output) ~= "table" then
        issues[#issues + 1] = path .. ".output: expected object"
      else
        if not item_exists(output.item) then
          issues[#issues + 1] = path .. ".output.item: unknown item " .. tostring(output.item)
        end
        if type(output.count) ~= "number" or output.count <= 0 or output.count ~= math.floor(output.count) then
          issues[#issues + 1] = path .. ".output.count: expected positive integer"
        end
      end
      if type(def.minutes) ~= "number" or def.minutes <= 0 then
        issues[#issues + 1] = path .. ".minutes: expected positive number"
      end
      if type(def.min_level) ~= "number" or def.min_level < 1 or def.min_level ~= math.floor(def.min_level) then
        issues[#issues + 1] = path .. ".min_level: expected positive integer"
      elseif type(def.station) == "string" and stations[def.station] then
        local ceiling = max_level[def.station] or 0
        if def.min_level > ceiling then
          issues[#issues + 1] = path
            .. ".min_level: exceeds max level "
            .. tostring(ceiling)
            .. " of station "
            .. def.station
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
