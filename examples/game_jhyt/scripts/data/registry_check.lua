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
-- 阶段 2+ 新增分类（改造计划 §4.3–§4.6/§8.2）：animal_product 畜牧产物、
-- fish 渔获、ore/gem 矿石与宝石、dish 菜肴、bait 鱼饵、rod 钓竿（工具走 tools）。
local ITEM_KINDS = { crop = true, seed = true, consumable = true, material = true, fertilizer = true, sprinkler = true, fruit = true, animal_product = true, fish = true, ore = true, gem = true, dish = true, bait = true, rod = true }
local TOOL_KINDS = { tool = true, weapon = true }
local SEASONS = { spring = true, summer = true, autumn = true, winter = true }
local FACINGS = { s = true, n = true, e = true, w = true, se = true, sw = true, ne = true, nw = true }
local USE_KEYS = { energy = true, inner_power = true, buff = true, social_day = true }
-- 肥料效果闭集（docs/33 §4.1.3）：quality/growth/yield。
local FERTILIZER_EFFECTS = { quality = true, growth = true, yield = true }
-- 洒水器形状闭集（docs/33 §4.1.4）：cross = 十字 4 邻，square = 方阵。
local SPRINKLER_SHAPES = { cross = true, square = true }
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
  -- 肥料（docs/33 §4.1.3）：kind=fertilizer 必须带 effect 且只能三选一；
  -- `systems/farm.lua` 按 effect 分支（quality/growth/yield），脏值会让肥料
  -- 施了也没效果（静默失效）。
  if definition.kind == "fertilizer" then
    if not FERTILIZER_EFFECTS[definition.effect] then
      issues[#issues + 1] = path .. ".effect: fertilizer requires one of "
        .. table.concat(sorted_keys(FERTILIZER_EFFECTS), "/")
    end
    if type(definition.buy) ~= "number" or definition.buy <= 0 then
      issues[#issues + 1] = path .. ".buy: fertilizer must be sold at shop (buy > 0)"
    end
    if definition.sell ~= nil and definition.sell ~= 0 then
      issues[#issues + 1] = path .. ".sell: fertilizer must not be buyable back (sell = 0)"
    end
  end
  -- 洒水器（docs/33 §4.1.4）：sprinkler 字段决定清晨自动浇水范围
  -- （shape = cross 十字 / square 方阵，radius = 1..2）。脏值会让洒水器
  -- 放了也不浇（静默失效）；buy/sell 必须为 0（仅制作获得，阻断套利）。
  if definition.kind == "sprinkler" then
    local spec = definition.sprinkler
    if type(spec) ~= "table" then
      issues[#issues + 1] = path .. ".sprinkler: required { shape, radius }"
    else
      if not SPRINKLER_SHAPES[spec.shape] then
        issues[#issues + 1] = path .. ".sprinkler.shape: requires one of "
          .. table.concat(sorted_keys(SPRINKLER_SHAPES), "/")
      end
      if type(spec.radius) ~= "number" or spec.radius < 1 or spec.radius > 2
        or spec.radius ~= math.floor(spec.radius) then
        issues[#issues + 1] = path .. ".sprinkler.radius: expected integer 1..2"
      end
    end
    if definition.buy ~= nil and definition.buy ~= 0 then
      issues[#issues + 1] = path .. ".buy: sprinkler must not be sold at shop (buy = 0)"
    end
    if definition.sell ~= nil and definition.sell ~= 0 then
      issues[#issues + 1] = path .. ".sell: sprinkler must not be buyable back (sell = 0)"
    end
  end
  -- 树果（docs/33 §4.1.5，2026-10-05 0.6）：kind=fruit，可卖（buys 收购）可送礼；
  -- 无品质档（不走 items 构建式派生）。buy 必须为 0（树上长的不能商店买）。
  if definition.kind == "fruit" then
    if type(definition.sell) ~= "number" or definition.sell <= 0 then
      issues[#issues + 1] = path .. ".sell: fruit must have positive sell price (shop buys it)"
    end
    if definition.buy ~= nil and definition.buy ~= 0 then
      issues[#issues + 1] = path .. ".buy: fruit must not be sold at shop (buy = 0)"
    end
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
      if use.social_day ~= nil and use.social_day ~= true then
        issues[#issues + 1] = path .. ".use.social_day: expected true or absent"
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

--- 单条工具/武器定义的结构校验（validate_tools 与 validate_item_extensions 共用，
--- 保证 data/tools.lua 与 data/items_<system>.lua 的 tools 表走同一套规则）。
local function check_tool(path, id, definition, issues)
  local items = require("data.items")
  if type(definition) ~= "table" then
    issues[#issues + 1] = path .. ": expected object"
    return
  end
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
    -- 工具家族（docs/33 §5.4）：family 必须与基础工具 id 一致（hoe/watering_can/
    -- axe/pick），`systems/farm.lua` 按家族判定装备；脏值会让高阶工具永远
    -- "没有装备"（need_hoe 兜底）或把斧头当锄头用。
    if not is_non_empty_string(definition.family) then
      issues[#issues + 1] = path .. ".family: tool requires non-empty family (docs/33 §5.4)"
    end
    -- 工具档位（docs/33 §5.4）：tier 1..5（1=基础，2–5=精铁/寒铁/陨铁/灵铁）。
    -- `systems/farm.lua` 翻土/浇水按装备工具 tier 减耗，脏值会让减耗静默
    -- 走基础档（等于升级无效）或越界读取（TIER_COST[6] 为 nil 按 8/4 兜底）。
    if not is_integer(definition.tier) or definition.tier < 1 or definition.tier > 5 then
      issues[#issues + 1] = path .. ".tier: tool requires integer 1..5 (docs/33 §5.4)"
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

--- tools 注册表：id 不得与物品同名（消费方 `items[id] or tools[id]` 会互相遮蔽），
--- 工具必须有体力消耗，武器必须有伤害。
function M.validate_tools()
  local issues = {}
  local tools = require("data.tools")
  if type(tools) ~= "table" then
    return false, { "data/tools.lua: expected table registry" }
  end
  for id, definition in pairs(tools) do
    check_tool("data/tools.lua:" .. tostring(id), id, definition, issues)
  end
  return #issues == 0, issues
end

--- 分系统物品扩展（阶段 2+）：data/items_<system>.lua 的可选模块
--- （{ items = {...}, tools = {...} }）与主注册表走同一套字段校验。
--- 模块缺失 = 对应系统尚未落地，跳过；落地后自动纳入启动门。
local ITEM_EXTENSION_MODULES = {
  "data.items_animal",
  "data.items_fish",
  "data.items_mine",
  "data.items_cooking",
  "data.items_stage3"
}

function M.validate_item_extensions()
  local issues = {}
  for _, module_name in ipairs(ITEM_EXTENSION_MODULES) do
    local ok, ext = pcall(require, module_name)
    if not ok then
      -- 模块缺失：系统未落地或已被移除，跳过（不报错，避免启动门依赖不存在文件）
    elseif type(ext) ~= "table" then
      issues[#issues + 1] = module_name .. ": expected module table { items, tools }"
    else
      for id, definition in pairs(ext.items or {}) do
        check_item(module_name .. ":" .. tostring(id), id, definition, issues)
      end
      for id, definition in pairs(ext.tools or {}) do
        check_tool(module_name .. ":" .. tostring(id), id, definition, issues)
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
      -- 再生作物（2026-10-05 四季矩阵）：`regrow_days` 可选，但一旦出现必须是正整数
      -- ——`systems/farm.lua` 的 `growth_need` 只在 cycles>0 时读取它，脏值会让复熟判定
      -- 静默走首茬口径（watered_days 直接越过成长天数，等于无限速熟）。
      if crop.regrow_days ~= nil
        and (not is_integer(crop.regrow_days) or crop.regrow_days < 1) then
        issues[#issues + 1] = path .. ".regrow_days: expected integer >= 1 when present"
      end
      -- 果树（docs/33 §4.1.5）：tree=true 时 tree_days（成树天数）与
      -- fruit_season（结果季节）必填合法；脏值会让树永不成熟或非秋可收。
      if crop.tree then
        if not is_integer(crop.tree_days) or crop.tree_days < 1 then
          issues[#issues + 1] = path .. ".tree_days: expected integer >= 1 when tree"
        end
        if not is_non_empty_string(crop.fruit_season) then
          issues[#issues + 1] = path .. ".fruit_season: required non-empty string when tree"
        elseif not SEASONS[crop.fruit_season] then
          issues[#issues + 1] = path .. ".fruit_season: unknown season "
            .. tostring(crop.fruit_season)
        end
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
          -- 果树特例（docs/33 §4.1.5）：tree 作物的收获物是树果（kind=fruit，
          -- 无品质档、不派生），不是常规 crop。
          if not (crop.tree and harvest.kind == "fruit") then
            issues[#issues + 1] = path .. ".harvest_item: " .. tostring(crop.harvest_item)
              .. " has kind " .. tostring(harvest.kind) .. ", expected crop"
          end
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
            .. ": unknown facing (known: s/n/e/w/se/sw/ne/nw)"
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

--- 品质条目派生（docs/33 §4.1.2，2026-10-05 0.4）：每个 kind=crop 且 sell>0 的
--- 收获物必须存在 `<id>_good` / `<id>_super` 派生条目，sell 恰为
--- floor(基础 × 1.25 / 1.6)、buy=0、kind=crop；且品质条目**不进任何商店**
--- stock/buys（上品是专属用途物品：高级药引/送礼/节日订单，不是套利通道）。
function M.validate_quality()
  local issues = {}
  local items = require("data.items")
  local shops = require("data.shops")
  local stock, buys = {}, {}
  for _, shop in pairs(shops) do
    if type(shop) == "table" then
      for _, it in ipairs(shop.stock or {}) do stock[it.item] = true end
      for _, it in ipairs(shop.buys or {}) do buys[it] = true end
    end
  end
  for id, def in pairs(items) do
    if def.kind == "crop" and type(def.sell) == "number" and def.sell > 0 then
      -- 派生条目（<id>_good/_super）本身就是 crop：跳过，不再要求其二次派生
      -- （否则 cabbage_good 又要 cabbage_good_good，校验无限外推）。
      -- 注意 Lua pattern 无 `|` 分支语法，必须写两个独立 pattern。
      if id:match("_good$") or id:match("_super$") then
        -- 只查它的商店泄漏（派生条目不进任何 stock/buys）
        if stock[id] or buys[id] then
          issues[#issues + 1] = "data/items.lua:" .. id .. ": quality crop must not appear in any shop stock/buys"
        end
      else
        for _, suffix in ipairs({ "good", "super" }) do
          local quality_id = id .. "_" .. suffix
          local multiplier = suffix == "good" and 1.25 or 1.6
          local quality = items[quality_id]
          local path = "data/items.lua:" .. quality_id
          if type(quality) ~= "table" then
            issues[#issues + 1] = path .. ": missing quality derivative for crop " .. id
          else
            local expected_sell = math.floor(def.sell * multiplier)
            if quality.sell ~= expected_sell then
              issues[#issues + 1] = path .. ".sell: expected floor(" .. tostring(def.sell)
                .. " × " .. tostring(multiplier) .. ") = " .. tostring(expected_sell)
                .. ", got " .. tostring(quality.sell)
            end
            if quality.buy ~= 0 then
              issues[#issues + 1] = path .. ".buy: quality crop must not be sold at shop (buy=0)"
            end
            if quality.kind ~= "crop" then
              issues[#issues + 1] = path .. ".kind: expected crop"
            end
          end
          if stock[quality_id] or buys[quality_id] then
            issues[#issues + 1] = path .. ": quality crop must not appear in any shop stock/buys"
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

--- 洒水器配方闭环（docs/33 §4.1.4，2026-10-05 0.5）：每个 kind=sprinkler 物品
--- 必须存在产出它的铁匠铺（forge）配方（data/recipes.lua），否则玩家永远无法
--- 获得该洒水器（物品成为死数据）。
function M.validate_sprinklers()
  local issues = {}
  local items = require("data.items")
  local recipes = require("data.recipes")
  for id, def in pairs(items) do
    if def and def.kind == "sprinkler" then
      local found = false
      for recipe_id, recipe in pairs(recipes.recipes or {}) do
        if recipe and recipe.output and recipe.output.item == id then
          if recipe.station ~= "forge" then
            issues[#issues + 1] = "data/recipes.lua:" .. recipe_id
              .. ": sprinkler " .. id .. " must be crafted at forge station"
          end
          found = true
          break
        end
      end
      if not found then
        issues[#issues + 1] = "data/items.lua:" .. id
          .. ": no forge recipe produces this sprinkler (dead item)"
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
    M.validate_enemy_art_refs,
    M.validate_quality,
    M.validate_sprinklers,
    M.validate_item_extensions
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
