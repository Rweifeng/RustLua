-- data/registry_check.lua：纯表注册表（items/tools/crops/farm）的加载期校验。
--
-- 为什么这些校验不写在注册表文件自己里：`data/items.lua`、`data/tools.lua`、
-- `data/crops.lua` 返回的是**注册表本身**（`items[id]` 直接取定义）。
-- `player/inventory.lua` 的 `M.setup()` 会
-- `for id, definition in pairs(items) do item.define(id, definition) end`，
-- 在这三张表里塞 `validate_all` 会被当作一个物品定义注册进引擎（同理 `version`
-- 也会变成一个物品）。所以纯表注册表的校验规则集中在本模块，由
-- `validate_data.lua` 统一登记：**规则只写一次**的约定不变，只是换了个宿主。
--
-- 交叉引用（items ↔ tools ↔ crops ↔ farm）在这里一次性查完：单个注册表自己看不出
-- "crop.seed_item 指向不存在的物品"这类跨文件断链。
local M = {}

M.version = 1

-- 与 `player/stats.lua` 的 VALID_STATS 一致（buff.stat 只能是这些）。
local STATS = { attack = true, defense = true }
-- 物品分类闭集：新增分类必须同时改这里，否则校验直接报错（宁可吵也不要静默）。
local ITEM_KINDS = { crop = true, seed = true, consumable = true, material = true }
local TOOL_KINDS = { tool = true, weapon = true }
local SEASONS = { spring = true, summer = true, autumn = true, winter = true }
local FACINGS = { s = true, n = true, e = true, w = true }
local USE_KEYS = { energy = true, inner_power = true, buff = true }
-- 与 data/gather.lua 相同的地图格边界（cell 0-index，40x22）。
local MAP_COLUMNS = 40
local MAP_ROWS = 22

local function is_non_empty_string(value)
  return type(value) == "string" and value ~= ""
end

local function is_integer(value)
  return type(value) == "number" and value == math.floor(value)
end

local function assets_path(value)
  return is_non_empty_string(value) and value:find("^assets/") ~= nil
end

local function sorted_keys(tbl)
  local keys = {}
  for key in pairs(tbl) do
    keys[#keys + 1] = tostring(key)
  end
  table.sort(keys)
  return keys
end

local function check_item(path, id, definition, issues)
  if type(definition) ~= "table" then
    issues[#issues + 1] = path .. ": expected object"
    return
  end
  if not is_non_empty_string(definition.name) then
    issues[#issues + 1] = path .. ".name: required non-empty string"
  end
  if not assets_path(definition.icon) then
    issues[#issues + 1] = path .. ".icon: required assets/ path, got " .. tostring(definition.icon)
  end
  if definition.stack ~= nil and (not is_integer(definition.stack) or definition.stack < 1) then
    issues[#issues + 1] = path .. ".stack: expected integer >= 1"
  end
  for _, field in ipairs({ "buy", "sell" }) do
    local value = definition[field]
    if value ~= nil and (type(value) ~= "number" or value < 0) then
      issues[#issues + 1] = path .. "." .. field .. ": expected non-negative number"
    end
  end
  if definition.kind ~= nil and not ITEM_KINDS[definition.kind] then
    issues[#issues + 1] = path .. ".kind: unknown kind " .. tostring(definition.kind)
      .. " (known: " .. table.concat(sorted_keys(ITEM_KINDS), "/") .. ")"
  end
  local use = definition.use
  if use ~= nil then
    if type(use) ~= "table" then
      issues[#issues + 1] = path .. ".use: expected object"
    else
      local effects = 0
      for key in pairs(use) do
        if not USE_KEYS[key] then
          issues[#issues + 1] = path .. ".use." .. tostring(key) .. ": unknown effect key"
        else
          effects = effects + 1
        end
      end
      for _, key in ipairs({ "energy", "inner_power" }) do
        local value = use[key]
        if value ~= nil and type(value) ~= "number" then
          issues[#issues + 1] = path .. ".use." .. key .. ": expected number"
        end
      end
      if use.buff ~= nil then
        local buff = use.buff
        if type(buff) ~= "table" then
          issues[#issues + 1] = path .. ".use.buff: expected object"
        else
          if not STATS[buff.stat] then
            issues[#issues + 1] = path .. ".use.buff.stat: unknown stat " .. tostring(buff.stat)
          end
          if type(buff.amount) ~= "number" or buff.amount == 0 then
            issues[#issues + 1] = path .. ".use.buff.amount: expected non-zero number"
          end
          if not is_integer(buff.days) or buff.days < 1 then
            issues[#issues + 1] = path .. ".use.buff.days: expected integer >= 1"
          end
        end
      end
      if effects == 0 then
        issues[#issues + 1] = path .. ".use: has no effect (energy/inner_power/buff)"
      end
    end
  end
  if id == "version" or id == "validate_all" then
    issues[#issues + 1] = path .. ": reserved key used as an item id"
  end
end

--- items 注册表：字段形状 + `use` 效果闭集。
function M.validate_items()
  local issues = {}
  local items = require("data.items")
  if type(items) ~= "table" then
    return false, { "data/items.lua: expected table registry" }
  end
  for id, definition in pairs(items) do
    check_item("data/items.lua:" .. tostring(id), id, definition, issues)
  end
  return #issues == 0, issues
end

--- tools 注册表：id 不得与物品同名（消费方 `items[id] or tools[id]` 会互相遮蔽），
--- 工具必须有体力消耗，武器必须有伤害。
function M.validate_tools()
  local issues = {}
  local tools = require("data.tools")
  local items = require("data.items")
  if type(tools) ~= "table" then
    return false, { "data/tools.lua: expected table registry" }
  end
  for id, definition in pairs(tools) do
    local path = "data/tools.lua:" .. tostring(id)
    if type(definition) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if items[id] ~= nil then
        issues[#issues + 1] = path .. ": id also exists in data/items.lua (shadowed lookup)"
      end
      if not is_non_empty_string(definition.name) then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if not TOOL_KINDS[definition.kind] then
        issues[#issues + 1] = path .. ".kind: unknown kind " .. tostring(definition.kind)
          .. " (known: " .. table.concat(sorted_keys(TOOL_KINDS), "/") .. ")"
      elseif definition.kind == "tool" then
        if not is_integer(definition.energy) or definition.energy < 0 then
          issues[#issues + 1] = path .. ".energy: tool requires integer >= 0"
        end
        if definition.damage ~= nil then
          issues[#issues + 1] = path .. ".damage: tool must not carry damage (use kind=weapon)"
        end
      else
        if type(definition.damage) ~= "number" or definition.damage < 0 then
          issues[#issues + 1] = path .. ".damage: weapon requires non-negative number"
        end
      end
      if definition.icon ~= nil and not assets_path(definition.icon) then
        issues[#issues + 1] = path .. ".icon: expected assets/ path"
      end
    end
  end
  return #issues == 0, issues
end

--- crops 注册表：种子/收获物必须真实存在于物品注册表，成熟期与季节闭集，
--- 土地类型必须被 data/farm.lua 的 land 区划覆盖，sell_price 必须与物品收价一致
--- （数值双写的漂移在这里挡住）。
function M.validate_crops()
  local issues = {}
  local crops = require("data.crops")
  local items = require("data.items")
  local tools = require("data.tools")
  local farm = require("data.farm")
  local function known_item(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  local land_types = {}
  for _, rect in ipairs(farm.land or {}) do
    if type(rect) == "table" and is_non_empty_string(rect.type) then
      land_types[rect.type] = true
    end
  end
  for id, crop in pairs(crops) do
    local path = "data/crops.lua:" .. tostring(id)
    if type(crop) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if not is_non_empty_string(crop.name) then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if not is_integer(crop.stages) or crop.stages < 1 then
        issues[#issues + 1] = path .. ".stages: expected integer >= 1"
      end
      if not is_integer(crop.growth_days) or crop.growth_days < 1 then
        issues[#issues + 1] = path .. ".growth_days: expected integer >= 1"
      end
      -- `systems/farm.lua` 收获时读 `crop.yield`（旧写法 `or 1` 会让缺字段的作物
      -- 静默按 1 产量结算：稻谷按设计案应为 2）。必填 + 正整数，杜绝静默兜底。
      if not is_integer(crop.yield) or crop.yield < 1 then
        issues[#issues + 1] = path .. ".yield: expected integer >= 1 (per-plot harvest amount)"
      end
      if type(crop.season) ~= "table" or #crop.season == 0 then
        issues[#issues + 1] = path .. ".season: expected non-empty array"
      else
        for index, season in ipairs(crop.season) do
          if not SEASONS[season] then
            issues[#issues + 1] = path .. ".season[" .. tostring(index) .. "]: unknown season "
              .. tostring(season)
          end
        end
      end
      if not is_non_empty_string(crop.land) then
        issues[#issues + 1] = path .. ".land: required non-empty string"
      elseif not land_types[crop.land] then
        issues[#issues + 1] = path .. ".land: no data/farm.lua land rect uses type "
          .. tostring(crop.land) .. " (crop can never be planted)"
      end
      if not is_non_empty_string(crop.seed_item) then
        issues[#issues + 1] = path .. ".seed_item: required non-empty string"
      elseif not known_item(crop.seed_item) then
        issues[#issues + 1] = path .. ".seed_item: unknown item " .. tostring(crop.seed_item)
      else
        local seed = items[crop.seed_item]
        if seed and seed.kind ~= nil and seed.kind ~= "seed" then
          issues[#issues + 1] = path .. ".seed_item: " .. tostring(crop.seed_item)
            .. " has kind " .. tostring(seed.kind) .. ", expected seed"
        end
      end
      if not is_non_empty_string(crop.harvest_item) then
        issues[#issues + 1] = path .. ".harvest_item: required non-empty string"
      elseif not known_item(crop.harvest_item) then
        issues[#issues + 1] = path .. ".harvest_item: unknown item " .. tostring(crop.harvest_item)
      else
        local harvest = items[crop.harvest_item]
        if harvest and harvest.kind ~= nil and harvest.kind ~= "crop" then
          issues[#issues + 1] = path .. ".harvest_item: " .. tostring(crop.harvest_item)
            .. " has kind " .. tostring(harvest.kind) .. ", expected crop"
        end
        if crop.sell_price ~= nil and harvest and harvest.sell ~= nil
          and crop.sell_price ~= harvest.sell then
          issues[#issues + 1] = path .. ".sell_price: " .. tostring(crop.sell_price)
            .. " != data/items.lua:" .. tostring(crop.harvest_item) .. ".sell "
            .. tostring(harvest.sell)
        end
      end
      if crop.sell_price ~= nil and (type(crop.sell_price) ~= "number" or crop.sell_price < 0) then
        issues[#issues + 1] = path .. ".sell_price: expected non-negative number"
      end
    end
  end
  return #issues == 0, issues
end

--- farm 初始数据：地块必须是地图内合法格、作物状态自洽、land 区划是合法矩形。
function M.validate_farm()
  local issues = {}
  local farm = require("data.farm")
  local crops = require("data.crops")
  if type(farm) ~= "table" then
    return false, { "data/farm.lua: expected table" }
  end
  if farm.version ~= 1 then
    issues[#issues + 1] = "data/farm.lua: version: unsupported version " .. tostring(farm.version)
  end
  if not is_non_empty_string(farm.title) then
    issues[#issues + 1] = "data/farm.lua: title: required non-empty string"
  end
  if not is_non_empty_string(farm.description) then
    issues[#issues + 1] = "data/farm.lua: description: required non-empty string"
  end
  if type(farm.plot_radius) ~= "number" or farm.plot_radius <= 0 then
    issues[#issues + 1] = "data/farm.lua: plot_radius: expected positive number"
  end
  if type(farm.plots) ~= "table" then
    issues[#issues + 1] = "data/farm.lua: plots: expected array"
  else
    local seen = {}
    for index, plot in ipairs(farm.plots) do
      local path = "data/farm.lua:plots[" .. tostring(index) .. "]"
      if type(plot) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        if not is_integer(plot.x) or not is_integer(plot.y) then
          issues[#issues + 1] = path .. ": expected integer {x, y}"
        elseif plot.x < 0 or plot.x >= MAP_COLUMNS or plot.y < 0 or plot.y >= MAP_ROWS then
          issues[#issues + 1] = path .. ": out of map bounds"
        else
          local key = plot.x .. "," .. plot.y
          if seen[key] then
            issues[#issues + 1] = path .. ": duplicate plot cell " .. key
          end
          seen[key] = true
        end
        if plot.crop ~= nil then
          local crop = crops[plot.crop]
          if crop == nil then
            issues[#issues + 1] = path .. ".crop: unknown crop " .. tostring(plot.crop)
          elseif not is_integer(plot.stage) or plot.stage < 0 or plot.stage > crop.stages then
            issues[#issues + 1] = path .. ".stage: expected integer 0.."
              .. tostring(crop.stages) .. " for crop " .. tostring(plot.crop)
          end
        elseif plot.stage ~= nil and (not is_integer(plot.stage) or plot.stage ~= 0) then
          issues[#issues + 1] = path .. ".stage: empty plot must have stage 0"
        end
        if type(plot.watered) ~= "boolean" then
          issues[#issues + 1] = path .. ".watered: expected boolean"
        end
      end
    end
  end
  if type(farm.land) ~= "table" or #farm.land == 0 then
    issues[#issues + 1] = "data/farm.lua: land: expected non-empty array"
  else
    for index, rect in ipairs(farm.land) do
      local path = "data/farm.lua:land[" .. tostring(index) .. "]"
      if type(rect) ~= "table" then
        issues[#issues + 1] = path .. ": expected object"
      else
        for _, field in ipairs({ "x0", "x1", "y0", "y1" }) do
          if not is_integer(rect[field]) then
            issues[#issues + 1] = path .. "." .. field .. ": expected integer"
          end
        end
        if is_integer(rect.x0) and is_integer(rect.x1) and rect.x0 > rect.x1 then
          issues[#issues + 1] = path .. ": x0 > x1"
        end
        if is_integer(rect.y0) and is_integer(rect.y1) and rect.y0 > rect.y1 then
          issues[#issues + 1] = path .. ": y0 > y1"
        end
        if is_integer(rect.x1) and rect.x1 >= MAP_COLUMNS then
          issues[#issues + 1] = path .. ".x1: out of map bounds"
        end
        if is_integer(rect.y1) and rect.y1 >= MAP_ROWS then
          issues[#issues + 1] = path .. ".y1: out of map bounds"
        end
        if not is_non_empty_string(rect.type) then
          issues[#issues + 1] = path .. ".type: required non-empty string"
        end
      end
    end
  end
  return #issues == 0, issues
end

--- enemy_art 的跨文件部分：`data/enemies.lua` 的 `art` 引用必须存在，且每个
--- 美术条目的动画串必须自洽（default 指向已声明的 clip、朝向键合法）。
--- 形状部分仍由 `data/enemy_art.lua` 自己的 validate_all 负责。
function M.validate_enemy_art_refs()
  local issues = {}
  local art = require("data.enemy_art")
  local enemies = require("data.enemies")
  local referenced = {}
  for _, enemy in ipairs((enemies and enemies.enemies) or {}) do
    if type(enemy) == "table" and enemy.art ~= nil then
      if not is_non_empty_string(enemy.art) then
        issues[#issues + 1] = "data/enemies.lua:" .. tostring(enemy.persist_id)
          .. ".art: expected non-empty string"
      elseif art.get(enemy.art) == nil then
        issues[#issues + 1] = "data/enemies.lua:" .. tostring(enemy.persist_id)
          .. ".art: unknown enemy art " .. tostring(enemy.art)
      else
        referenced[enemy.art] = true
      end
    end
  end
  local entries = (art and art.enemies) or {}
  for id, entry in pairs(entries) do
    local path = "data/enemy_art.lua:" .. tostring(id)
    if type(entry) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if not referenced[id] then
        issues[#issues + 1] = path .. ": no data/enemies.lua entry references this art"
      end
      for facing in pairs((type(entry.static_sprite) == "table") and entry.static_sprite or {}) do
        if not FACINGS[facing] then
          issues[#issues + 1] = path .. ".static_sprite." .. tostring(facing)
            .. ": unknown facing (known: s/n/e/w)"
        end
      end
      if is_non_empty_string(entry.animation) then
        local default_clip = entry.animation:match("default=([^;]+)")
        if default_clip == nil then
          issues[#issues + 1] = path .. ".animation: missing default=<clip>"
        else
          local clips = {}
          for clip in entry.animation:gmatch("clip:([^:;]+):") do
            clips[clip] = true
          end
          if not clips[default_clip] then
            issues[#issues + 1] = path .. ".animation: default clip " .. tostring(default_clip)
              .. " is not declared"
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

--- 合并入口：validate_data.lua 只登记这一个模块。
function M.validate_all()
  local issues = {}
  for _, validator in ipairs({
    M.validate_items,
    M.validate_tools,
    M.validate_crops,
    M.validate_farm,
    M.validate_enemy_art_refs
  }) do
    local ok, valid, list = pcall(validator)
    if not ok then
      issues[#issues + 1] = "data/registry_check.lua: validator crashed: " .. tostring(valid)
    elseif valid ~= true then
      for _, issue in ipairs(list or {}) do
        issues[#issues + 1] = tostring(issue)
      end
    end
  end
  return #issues == 0, issues
end

return M
