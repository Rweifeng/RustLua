-- data/items_animal.lua：畜牧产物物品注册表（阶段 2，docs/33 §4.4，2026-10-06 1.0）。
--
-- 分系统物品扩展（player/inventory.lua 的 ITEM_EXTENSION_MODULES 已登记本模块）：
-- 启动时自动 item.define 注册，缺失即跳过。校验走 data/registry_check.lua 的
-- validate_item_extensions（与主注册表同一套 check_item 规则）。
--
-- 【运行期并入主表】与 data/items_cooking.lua 同一纪律：shipping.sell_price、
-- systems.animal.validate_all 的 known() 都只查 require('data.items') 这张表。
-- 扩展产物若只走 item.define（引擎侧显示名）而不并入主表，会出现两个静默故障：
--   1) shipping.add(蛋) → sell_price 查不到 → not_shippable（产了卖不掉）；
--   2) systems.animal.validate_all → produce 判 unknown（校验门红）。
-- 故模块加载时并入 data.items（require 缓存同一张表；item.define 幂等）。
--
-- 口径（docs/33 §5 经济）：畜牧中档收益、略高于种植（稻谷 sell=30 一档为锚）；
-- 产物自产不可购（buy=0），可出货箱/货摊卖出（sell>0）。kind 闭集已含
-- animal_product（registry_check.ITEM_KINDS），不走 crop 品质派生。
-- 图标 assets/art/items/<id>.png 为占位，阶段 4 美术管线批量替换。
local base = require("data.items")

local animal_products = {
  -- 白凤鸡日产：低值日产物。
  egg = {
    name = "蛋",
    description = "白凤鸡日产的鲜蛋，山庄药膳与日常吃食两用。",
    icon = "assets/art/items/egg.png",
    stack = 99,
    buy = 0,
    sell = 25,
    kind = "animal_product"
  },
  -- 芦花鸭日产。
  duck_egg = {
    name = "鸭蛋",
    description = "芦花鸭所产，壳青个大，腌食俱佳。",
    icon = "assets/art/items/egg.png",
    stack = 99,
    buy = 0,
    sell = 30,
    kind = "animal_product"
  },
  -- 青山羊剪绒 / 青驴冬绒（周期产物）。
  wool = {
    name = "羊毛",
    description = "羊身上剪下的绒，纺线织毡的材料。",
    icon = "assets/art/items/reed.png",
    stack = 99,
    buy = 0,
    sell = 40,
    kind = "animal_product"
  },
  -- 老黄牛挤奶（周期产物）。
  milk = {
    name = "牛乳",
    description = "老黄牛每日所挤的鲜奶，温补滋养。",
    icon = "assets/art/items/milk.png",
    stack = 99,
    buy = 0,
    sell = 45,
    kind = "animal_product"
  },
  -- 花猪出栏（周期产物）。
  pork = {
    name = "猪肉",
    description = "花猪育肥后的鲜肉，山庄饭桌的主菜。",
    icon = "assets/art/items/boar_meat.png",
    stack = 99,
    buy = 0,
    sell = 60,
    kind = "animal_product"
  },
  -- 老黄牛出栏（周期产物）。
  beef = {
    name = "牛肉",
    description = "耕牛老迈后取的肉，炖煮大补。",
    icon = "assets/art/items/boar_meat.png",
    stack = 99,
    buy = 0,
    sell = 80,
    kind = "animal_product"
  },
  -- 灵蛋：白凤鸡好感极高（≥8）时的特殊产物（systems/animal.lua），高档卖价。
  spirit_egg = {
    name = "灵蛋",
    description = "白凤鸡日日受抚、吸聚药园灵气所产的灵蛋，远超寻常鸡蛋。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 0,
    sell = 120,
    kind = "animal_product"
  }
}

-- 并入主表（见文件头注释：让 shipping.sell_price / animal.validate_all 都看得见）。
for id, definition in pairs(animal_products) do
  base[id] = definition
end

return { items = animal_products, tools = {} }
