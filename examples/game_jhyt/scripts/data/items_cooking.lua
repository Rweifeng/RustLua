-- data/items_cooking.lua：阶段 2「料理与厨房」菜肴物品扩展
-- （docs/33 §4.7 / docs/54 §4.6，2026-10-06 1.3）。
--
-- 契约：返回 { items = {...}, tools = {...} }。启动时由 player/inventory.lua 的
-- register_items() 对每个条目 item.define 进引擎注册表；字段形状（name/icon/buy/sell/
-- kind/use）由 data/registry_check.lua 的 validate_item_extensions 用主表同一套
-- check_item 校验。tools 留空（本系统不新增工具）。
--
-- 【运行期并入主表】use_effect（player/inventory.lua）、recipes.validate_all 的
-- item_exists、quality 派生都只查 require('data.items') 这张表。扩展物品若只走
-- item.define（引擎侧显示名）而不并入主表，会出现两个静默故障：
--   1) inventory.use(菜肴) → use_effect 查不到 → 返回 no_effect（菜吃不动）；
--   2) recipes.lua 以菜肴为 output → item_exists 判 unknown item（校验门红）。
-- 故模块加载时把菜肴并入 data.items（require 缓存的是同一张表；item.define 幂等，
-- register_items 后续对主表再 item.define 一次也无副作用）。
--
-- 数值口径（docs/33 §5）：菜肴是「自产消耗品」——buy=0（膳房自炊，不上货架），
-- sell 按原料种植成本回收 + 合理薄利定档（与既有饭团 sell=4 / 行药汤 sell=30 同
-- 一档思路，高价药膳对应贵价药引）。use 只走四键（energy / inner_power /
-- buff{stat,amount,days} / social_day）；buff.stat 仅 attack/defense。
-- 图标为占位（assets/art/items/<id>.png），正式美术随改造计划阶段 4 管线替换。
local base = require("data.items")

local dishes = {
  -- ---- 家常菜（膳房 1 级）----
  rice_congee = {
    name = "白米粥",
    description = "文火慢熬的米粥，清淡暖胃，最顶寻常饥肠。",
    icon = "assets/art/items/rice_congee.png",
    stack = 9,
    buy = 0,
    sell = 8,
    kind = "dish",
    use = { energy = 20 }
  },
  stirfry_radish = {
    name = "清炒萝卜",
    description = "快火翻出的萝卜片，脆嫩爽口，下饭一绝。",
    icon = "assets/art/items/stirfry_radish.png",
    stack = 9,
    buy = 0,
    sell = 12,
    kind = "dish",
    use = { energy = 18 }
  },
  braised_radish_meat = {
    name = "萝卜炖肉",
    description = "兽肉与萝卜同炖，汤汁浓白，一碗下去浑身发热。",
    icon = "assets/art/items/braised_radish_meat.png",
    stack = 9,
    buy = 0,
    sell = 25,
    kind = "dish",
    use = { energy = 35 }
  },
  stirfry_cabbage = {
    name = "醋溜白菜",
    description = "急火快炒的酸辣白菜，醒神开胃。",
    icon = "assets/art/items/stirfry_cabbage.png",
    stack = 9,
    buy = 0,
    sell = 18,
    kind = "dish",
    use = { energy = 15 }
  },
  braised_eggplant = {
    name = "红烧茄子",
    description = "油亮软糯的红烧茄子，酱香浓郁。",
    icon = "assets/art/items/braised_eggplant.png",
    stack = 9,
    buy = 0,
    sell = 24,
    kind = "dish",
    use = { energy = 25 }
  },
  steamed_pumpkin = {
    name = "清蒸南瓜",
    description = "原笼清蒸的南瓜，本身甜糯，无需调料。",
    icon = "assets/art/items/steamed_pumpkin.png",
    stack = 9,
    buy = 0,
    sell = 30,
    kind = "dish",
    use = { energy = 30 }
  },
  pumpkin_congee = {
    name = "南瓜粥",
    description = "南瓜与粳米同熬，金黄甜香，最是养人。",
    icon = "assets/art/items/pumpkin_congee.png",
    stack = 9,
    buy = 0,
    sell = 32,
    kind = "dish",
    use = { energy = 35 }
  },
  mung_bean_soup = {
    name = "绿豆汤",
    description = "绿豆薄荷同煮，冰过之后清热解暑。",
    icon = "assets/art/items/mung_bean_soup.png",
    stack = 9,
    buy = 0,
    sell = 26,
    kind = "dish",
    use = { energy = 20, inner_power = 10 }
  },
  laba_congee = {
    name = "腊八粥",
    description = "莲子红枣枸杞与米同熬，节令暖身大补粥。",
    icon = "assets/art/items/laba_congee.png",
    stack = 9,
    buy = 0,
    sell = 42,
    kind = "dish",
    use = { energy = 45 }
  },
  osmanthus_cake = {
    name = "菊花糕",
    description = "入秋采菊和米蒸成的花糕，清甜一口。",
    icon = "assets/art/items/osmanthus_cake.png",
    stack = 9,
    buy = 0,
    sell = 34,
    kind = "dish",
    use = { energy = 20, inner_power = 10 }
  },
  fruit_compote = {
    name = "蜜饯果盘",
    description = "野果红枣青梅蜜渍成盘，闲时解闷小食。",
    icon = "assets/art/items/fruit_compote.png",
    stack = 9,
    buy = 0,
    sell = 30,
    kind = "dish",
    use = { inner_power = 20 }
  },
  braised_pork = {
    name = "红烧肉",
    description = "野果提色的红烧兽肉，肥而不腻，齿颊生香。",
    icon = "assets/art/items/braised_pork.png",
    stack = 9,
    buy = 0,
    sell = 40,
    kind = "dish",
    use = { energy = 50 }
  },
  winter_hotpot = {
    name = "冬暖锅",
    description = "雪夜围炉，冬菜兽肉雪里蕻一锅炖透。",
    icon = "assets/art/items/winter_hotpot.png",
    stack = 9,
    buy = 0,
    sell = 36,
    kind = "dish",
    use = { energy = 40 }
  },
  -- ---- 药膳（膳房 2 级，带临时 Buff）----
  herb_chicken_soup = {
    name = "药膳鸡汤",
    description = "人参枸杞与兽肉慢煨的老汤，大补元气。",
    icon = "assets/art/items/herb_chicken_soup.png",
    stack = 9,
    buy = 0,
    sell = 70,
    kind = "dish",
    use = { energy = 40, inner_power = 30 }
  },
  bone_strength_soup = {
    name = "壮骨汤",
    description = "枸杞炖兽骨的浓汤，饮下筋骨强健，护甲护体。",
    icon = "assets/art/items/bone_strength_soup.png",
    stack = 9,
    buy = 0,
    sell = 48,
    kind = "dish",
    use = { buff = { stat = "defense", amount = 4, days = 2 } }
  },
  qi_blood_soup = {
    name = "补血养气汤",
    description = "灵芝人参同煎，辅以红枣，服之气血翻涌、臂力大增。",
    icon = "assets/art/items/qi_blood_soup.png",
    stack = 9,
    buy = 0,
    sell = 80,
    kind = "dish",
    use = { buff = { stat = "attack", amount = 4, days = 3 }, inner_power = 20 }
  }
}

-- 并入主表（见文件头注释：让 use_effect / recipes.validate_all / quality 都看得见）。
for id, definition in pairs(dishes) do
  base[id] = definition
end

return { items = dishes, tools = {} }
