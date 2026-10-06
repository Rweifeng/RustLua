-- data/items_fish.lua：钓鱼系统的物品扩展注册表（阶段 2，docs/33 §4.5，2026-10-06 1.1）。
--
-- player/inventory.lua 启动时自动 require 本模块，把 ext.items / ext.tools 注册进引擎
-- （item.define）；data/registry_check.lua 的 validate_item_extensions 用与主注册表
-- 同一套 check_item / check_tool 规则校验本文件。缺失即跳过（系统未落地）。
--
-- 鱼物品的 name/sell 由 data/fish.lua 单一事实来源派生（避免价格双写漂移）；
-- 这里只补物品层字段（icon/stack/buy/kind）。icon 路径约定 assets/art/items/<id>.png
-- ——具体鱼图/竿图属阶段 4 美术管线，当前为占位路径（registry_check 只校验 assets/
-- 前缀，不校验文件存在）。图标显式逐条写出（不拼接 id），让 docs/48 资源接线清单的
-- 文本扫描能看见每条引用（拼接路径对扫描器不可见，会误判为内容缺口）。
local fish_data = require("data.fish")
local base_items = require("data.items")

local fish_icons = {
  crucian = "assets/art/items/crucian.png",
  carp = "assets/art/items/carp.png",
  whitebait = "assets/art/items/whitebait.png",
  goldfish = "assets/art/items/goldfish.png",
  loach = "assets/art/items/loach.png",
  catfish = "assets/art/items/catfish.png",
  pondfish = "assets/art/items/pondfish.png",
  yellowhead = "assets/art/items/yellowhead.png",
  lakecarp = "assets/art/items/lakecarp.png",
  grasscarp = "assets/art/items/grasscarp.png",
  perch = "assets/art/items/perch.png",
  mandarin = "assets/art/items/mandarin.png",
  trout = "assets/art/items/trout.png",
  silverfish = "assets/art/items/silverfish.png",
  eel = "assets/art/items/eel.png",
  cavesilver = "assets/art/items/cavesilver.png",
  blindfish = "assets/art/items/blindfish.png",
  icefish = "assets/art/items/icefish.png",
  snowlotus = "assets/art/items/snowlotus.png",
  floodcarp = "assets/art/items/floodcarp.png",
  snakehead = "assets/art/items/snakehead.png",
  swangeel = "assets/art/items/swangeel.png",
}

-- 防撞名守护（与本文件末尾合并守护同一意图）：渔获 id 若已被既有物品占用
-- （典型：桃花鱼 fish.lua id="peach" 与果树收获物桃子 items.lua:peach kind=fruit
-- 撞 id），**不**为其生成物品条目——既不并入 Lua 主表，也不让 inventory.lua 的
-- 注册循环把 engine item.def 覆盖成 kind=fish/sell=30（那样 data/crops.lua 果树校验
-- harvest_item=peach 须 kind=fruit/sell=25 启动即红）。fish_data.fish["peach"]
-- 键保留（m45 force/catchable 季节判定用），不受影响；其余全新渔获 id 照常注册。
local items = {}
for id, f in pairs(fish_data.fish) do
  if base_items[id] == nil then
    items[id] = {
      name = f.name,
      description = "钓上来的" .. f.name .. "，可入镖局货箱或售予货摊。",
      icon = fish_icons[id] or ("assets/art/items/" .. tostring(id) .. ".png"),
      stack = 99,
      buy = 0, -- 鱼不可商店购买（只能钓）
      sell = f.sell,
      kind = "fish",
    }
  end
end

-- 钓竿（tools）：family=fishing_rod，占用与锄/壶同一 hand-tool（tool）槽——
-- 与既有工具同槽切换（见 systems/fishing.lua 头注）。tier 1..3 决定可钓鱼难度上限；
-- energy 为每次抛竿体力消耗（整数，可 0）。
local tools = {
  rod_basic = {
    name = "竹钓竿",
    kind = "tool",
    family = "fishing_rod",
    tier = 1,
    energy = 8,
    icon = "assets/art/items/rod_basic.png",
  },
  rod_iron = {
    name = "精铁钓竿",
    kind = "tool",
    family = "fishing_rod",
    tier = 2,
    energy = 5,
    icon = "assets/art/items/rod_iron.png",
  },
  rod_master = {
    name = "寒江钓竿",
    kind = "tool",
    family = "fishing_rod",
    tier = 3,
    energy = 3,
    icon = "assets/art/items/rod_master.png",
  },
}

-- 并入主表：让 Lua 侧 require("data.items")/require("data.tools") 能查到渔获与钓竿
-- （shipping 结算价、recipes 校验、图鉴派生、后续商店收购等消费方都直查主表）。
-- 防撞名守护：渔获 id 若已被既有物品占用（典型：桃花鱼 fish.lua id="peach" 与果树
-- 收获物桃子 items.lua:peach kind=fruit 撞 id），**保留既有物品定义不覆盖**——否则
-- data/items["peach"] 被鱼覆盖成 kind=fish/sell=30，data/crops.lua 果树校验
-- （harvest_item=peach 须 kind=fruit、sell_price 25）启动即红。全新渔获 id（鲫鱼等）
-- 照常注册。钓鱼系统的 fish_data.fish 键（m45 force/pool 用）不受影响。
local base_items = require("data.items")
for id, def in pairs(items) do
  if base_items[id] == nil then
    base_items[id] = def
  end
end
local base_tools = require("data.tools")
for id, def in pairs(tools) do
  base_tools[id] = def
end

return {
  items = items,
  tools = tools,
}
