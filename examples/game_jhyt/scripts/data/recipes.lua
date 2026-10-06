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
  },
  -- v0.2 药庐高等级：回气丹（大补丸）。
  qi_pill_refine = {
    name = "炼制回气丹",
    station = "herb",
    inputs = { { item = "ginseng", count = 1 }, { item = "lingzhi", count = 2 } },
    output = { item = "qi_pill", count = 1 },
    minutes = 240,
    min_level = 2
  },
  -- 铁匠炉：3 级配方链。铁矿石→铁锭→麻布（炼铁副产品）
  forge_smelt_iron = {
    name = "熔铁矿石",
    station = "forge",
    inputs = { { item = "iron_ore", count = 3 } },
    output = { item = "iron_ingot", count = 1 },
    minutes = 90,
    min_level = 1
  },
  forge_weave_cloth = {
    name = "沤麻织布",
    station = "forge",
    inputs = { { item = "reed", count = 4 } },
    output = { item = "cloth", count = 1 },
    minutes = 60,
    min_level = 1
  },
  -- 洒水器（2026-10-05，docs/33 §4.1.4）：机关类灌溉设施挂铁匠铺；
  -- 机关洒水器需 forge 2 级（铁匠炉升级链）。产出物品 buy=0/sell=0 不进商店。
  sprinkler_bamboo_build = {
    name = "造竹筒引水器",
    station = "forge",
    inputs = { { item = "timber", count = 3 } },
    output = { item = "sprinkler_bamboo", count = 1 },
    minutes = 60,
    min_level = 1
  },
  sprinkler_waterwheel_build = {
    name = "造水车洒水器",
    station = "forge",
    inputs = { { item = "timber", count = 8 }, { item = "iron_ingot", count = 1 } },
    output = { item = "sprinkler_waterwheel", count = 1 },
    minutes = 120,
    min_level = 1
  },
  sprinkler_machine_build = {
    name = "造机关洒水器",
    station = "forge",
    inputs = { { item = "timber", count = 20 }, { item = "iron_ingot", count = 3 } },
    output = { item = "sprinkler_machine", count = 1 },
    minutes = 180,
    min_level = 2
  },
  -- 膳房：饭团与行药汤。
  kitchen_rice_ball = {
    name = "捏饭团",
    station = "kitchen",
    inputs = { { item = "rice", count = 2 } },
    output = { item = "rice_ball", count = 1 },
    minutes = 30,
    min_level = 1
  },
  -- 茶棚（M34-05）：这条原来挂在膳房上（`kitchen_brew_tea`）。docs/33 §7.2 写的是
  -- "茶棚 1 级 = 解锁茶饮"，所以配方跟着设施走：膳房不再顺便出茶，想喝茶得先把茶棚盖起来。
  -- 同一产出留两条无差异配方就是第二条数据源（改一份忘一份），故整体改挂、不并存。
  tea_spring_brew = {
    name = "冲山泉茶",
    station = "tea",
    inputs = { { item = "tea_leaf", count = 2 } },
    output = { item = "tea_brew", count = 1 },
    minutes = 20,
    min_level = 1
  },
  kitchen_herb_soup = {
    name = "炖行药汤",
    station = "kitchen",
    inputs = { { item = "lingzhi", count = 1 }, { item = "herb", count = 2 } },
    output = { item = "mountain_herb_soup", count = 1 },
    minutes = 120,
    min_level = 2
  },
  -- ---- 料理与厨房（2026-10-06，docs/33 §4.7 / docs/54 §4.6，1.3）----
  -- 全部挂膳房（station="kitchen"）。1 级家常菜以体力回复为主，2 级药膳带
  -- attack/defense 临时 Buff（days 1..3，经 inventory.use → stats.add_buff）。
  -- 输入只引用 data.items.lua 已存在物品（rice/lingzhi/ginseng/boar_meat/lotus_seed/
  -- 四季作物/树果等）；蛋/鱼等畜牧·钓鱼系统产物的配方见文件末补丁说明，本文件未落库。
  kitchen_rice_congee = {
    name = "熬白米粥",
    station = "kitchen",
    inputs = { { item = "rice", count = 2 } },
    output = { item = "rice_congee", count = 1 },
    minutes = 20,
    min_level = 1
  },
  kitchen_stirfry_radish = {
    name = "清炒萝卜",
    station = "kitchen",
    inputs = { { item = "radish", count = 2 } },
    output = { item = "stirfry_radish", count = 1 },
    minutes = 30,
    min_level = 1
  },
  kitchen_braised_radish_meat = {
    name = "萝卜炖肉",
    station = "kitchen",
    inputs = { { item = "radish", count = 1 }, { item = "boar_meat", count = 1 } },
    output = { item = "braised_radish_meat", count = 1 },
    minutes = 60,
    min_level = 1
  },
  kitchen_stirfry_cabbage = {
    name = "醋溜白菜",
    station = "kitchen",
    inputs = { { item = "cabbage", count = 1 }, { item = "leek", count = 1 } },
    output = { item = "stirfry_cabbage", count = 1 },
    minutes = 35,
    min_level = 1
  },
  kitchen_braised_eggplant = {
    name = "红烧茄子",
    station = "kitchen",
    inputs = { { item = "eggplant", count = 2 } },
    output = { item = "braised_eggplant", count = 1 },
    minutes = 50,
    min_level = 1
  },
  kitchen_steamed_pumpkin = {
    name = "清蒸南瓜",
    station = "kitchen",
    inputs = { { item = "pumpkin", count = 1 } },
    output = { item = "steamed_pumpkin", count = 1 },
    minutes = 45,
    min_level = 1
  },
  kitchen_pumpkin_congee = {
    name = "南瓜粥",
    station = "kitchen",
    inputs = { { item = "pumpkin", count = 1 }, { item = "rice", count = 1 } },
    output = { item = "pumpkin_congee", count = 1 },
    minutes = 60,
    min_level = 1
  },
  kitchen_mung_bean_soup = {
    name = "绿豆汤",
    station = "kitchen",
    inputs = { { item = "mung_bean", count = 2 }, { item = "mint", count = 1 } },
    output = { item = "mung_bean_soup", count = 1 },
    minutes = 40,
    min_level = 1
  },
  kitchen_laba_congee = {
    name = "腊八粥",
    station = "kitchen",
    inputs = {
      { item = "rice", count = 1 },
      { item = "lotus_seed", count = 1 },
      { item = "jujube", count = 1 },
      { item = "gouqi", count = 1 }
    },
    output = { item = "laba_congee", count = 1 },
    minutes = 90,
    min_level = 1
  },
  kitchen_osmanthus_cake = {
    name = "蒸菊花糕",
    station = "kitchen",
    inputs = { { item = "rice", count = 2 }, { item = "chrysanthemum", count = 1 } },
    output = { item = "osmanthus_cake", count = 1 },
    minutes = 40,
    min_level = 1
  },
  kitchen_fruit_compote = {
    name = "蜜饯果盘",
    station = "kitchen",
    inputs = {
      { item = "wild_fruit", count = 1 },
      { item = "jujube", count = 1 },
      { item = "mume", count = 1 }
    },
    output = { item = "fruit_compote", count = 1 },
    minutes = 30,
    min_level = 1
  },
  kitchen_braised_pork = {
    name = "红烧肉",
    station = "kitchen",
    inputs = { { item = "boar_meat", count = 2 }, { item = "wild_fruit", count = 1 } },
    output = { item = "braised_pork", count = 1 },
    minutes = 100,
    min_level = 1
  },
  kitchen_winter_hotpot = {
    name = "冬暖锅",
    station = "kitchen",
    inputs = {
      { item = "winter_cabbage", count = 1 },
      { item = "boar_meat", count = 1 },
      { item = "xueli", count = 1 }
    },
    output = { item = "winter_hotpot", count = 1 },
    minutes = 80,
    min_level = 1
  },
  -- 2 级药膳：带临时 Buff（attack/defense，days 2..3）。
  kitchen_herb_chicken_soup = {
    name = "炖药膳鸡汤",
    station = "kitchen",
    inputs = {
      { item = "ginseng", count = 1 },
      { item = "boar_meat", count = 1 },
      { item = "gouqi", count = 1 }
    },
    output = { item = "herb_chicken_soup", count = 1 },
    minutes = 150,
    min_level = 2
  },
  kitchen_bone_strength_soup = {
    name = "炖壮骨汤",
    station = "kitchen",
    inputs = { { item = "boar_meat", count = 2 }, { item = "gouqi", count = 1 } },
    output = { item = "bone_strength_soup", count = 1 },
    minutes = 120,
    min_level = 2
  },
  kitchen_qi_blood_soup = {
    name = "煎补血养气汤",
    station = "kitchen",
    inputs = {
      { item = "lingzhi", count = 1 },
      { item = "ginseng", count = 1 },
      { item = "jujube", count = 1 }
    },
    output = { item = "qi_blood_soup", count = 1 },
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
