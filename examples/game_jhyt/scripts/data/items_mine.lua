-- data/items_mine.lua：《江湖有田》矿脉系统物品扩展（阶段2 青云矿脉，docs/33 §4.6，
-- 2026-10-06 1.2）。
--
-- 为什么单独一张扩展表而不是塞进 data/items.lua：矿脉是**并行开发**的阶段2系统，
-- 多人同时改主注册表会合并冲突；registry_check.lua 的 validate_item_extensions 会
-- 自动 require 本模块并与主注册表走同一套 check_item/check_tool 规则（见该文件头注释）。
-- inventory.lua 的 register_items() 启动时也会 item.define 本模块的全部物品（显示名口）。
--
-- 口径（docs/33 §4.6）：
--   * 矿石分级 5 级：铜(copper_ore)/铁(iron_ore **复用 data/items.lua 既有条目**，
--     sell=20，本文件不新增以免 id 冲突)/银(silver_ore)/金(gold_ore)/寒铁(cold_iron_ore)。
--     深度越深越高阶：前10层铜铁、11–20银金、21–30金/寒铁。
--   * 宝石 7 种（kind=gem，稀有掉落，低概率）：琥珀/玛瑙/玉石/紫晶/翡翠/金刚石/夜明珠。
--   * 全部 buy=0（矿洞里挖出来的，商店不卖）、sell>0（可入镖局货箱/货摊收购）。
--     icon 用 assets/art/items/<id>.png 占位——真美术在改造计划阶段4批量替换
--     （与 iron_ore→reed.png、四季作物复用图标同一资源策略；registry_check 只校验
--     路径前缀 assets/，不查文件存在性）。
local items = {
  -- ---- 矿石（kind=ore；iron_ore 复用主表，不重复声明）----
    copper_ore = {
      name = "铜矿石",
      description = "青云矿脉浅层的黄铜矿石，炼铜铸器的粗料。",
      icon = "assets/art/items/copper_ore.png",
      stack = 99,
      buy = 0,
      sell = 8,
      kind = "ore"
    },
    silver_ore = {
      name = "银矿石",
      description = "矿脉中层的白银矿石，纹理泛青，成色可观。",
      icon = "assets/art/items/silver_ore.png",
      stack = 99,
      buy = 0,
      sell = 45,
      kind = "ore"
    },
    gold_ore = {
      name = "金矿石",
      description = "矿脉深处的金砂矿石，色泽沉厚，炼出足赤真金。",
      icon = "assets/art/items/gold_ore.png",
      stack = 99,
      buy = 0,
      sell = 100,
      kind = "ore"
    },
    cold_iron_ore = {
      name = "寒铁矿石",
      description = "矿脉最深处的玄寒铁矿石，触手生冰，是打造神兵的料。",
      icon = "assets/art/items/cold_iron_ore.png",
      stack = 99,
      buy = 0,
      sell = 200,
      kind = "ore"
    },
    -- ---- 宝石（kind=gem；稀有低概率掉落，越深越好）----
    amber = {
      name = "琥珀",
      description = "封着远古松脂的金黄宝石，透光见虫，温润可人。",
      icon = "assets/art/items/amber.png",
      stack = 99,
      buy = 0,
      sell = 90,
      kind = "gem"
    },
    agate = {
      name = "玛瑙",
      description = "缠丝纹理的红褐玛瑙，雕作佩饰最是雅致。",
      icon = "assets/art/items/agate.png",
      stack = 99,
      buy = 0,
      sell = 120,
      kind = "gem"
    },
    jade = {
      name = "玉石",
      description = "羊脂白玉，温润内敛，江湖人视作硬通的体面。",
      icon = "assets/art/items/jade.png",
      stack = 99,
      buy = 0,
      sell = 150,
      kind = "gem"
    },
    amethyst = {
      name = "紫晶",
      description = "深紫通透的水晶，凝神定气，武者珍爱。",
      icon = "assets/art/items/amethyst.png",
      stack = 99,
      buy = 0,
      sell = 220,
      kind = "gem"
    },
    emerald = {
      name = "翡翠",
      description = "浓阳正绿的翡翠，水头十足，是千金难求的珍玩。",
      icon = "assets/art/items/emerald.png",
      stack = 99,
      buy = 0,
      sell = 280,
      kind = "gem"
    },
    diamond = {
      name = "金刚石",
      description = "无坚不摧的至坚宝石，划玉削铁，稀世之珍。",
      icon = "assets/art/items/diamond.png",
      stack = 99,
      buy = 0,
      sell = 500,
      kind = "gem"
    },
    luminous_pearl = {
      name = "夜明珠",
      description = "暗夜自生寒光的明珠，传说吸月华而成，矿脉至宝藏。",
      icon = "assets/art/items/luminous_pearl.png",
      stack = 99,
      buy = 0,
      sell = 900,
      kind = "gem"
    }
}

local tools = {}

-- 并入主表：让 Lua 侧 require("data.items")/require("data.tools") 能查到矿石/宝石
-- （shipping 结算价、recipes 校验、图鉴派生、货摊收购等消费方都直查主表）。
local base_items = require("data.items")
for id, def in pairs(items) do
  base_items[id] = def
end
local base_tools = require("data.tools")
for id, def in pairs(tools) do
  base_tools[id] = def
end

return {
  items = items,
  tools = tools,
}
