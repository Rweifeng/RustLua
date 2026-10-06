-- data/items_stage3.lua：阶段3「黑市抉择循环」违禁品/稀有货物品扩展
-- （docs/54 阶段3「黑市抉择循环」，2026-10-06 2.2）。
--
-- 契约（与 data/items_mine.lua / data/items_cooking.lua 同一套）：
--   * 返回 { items = {...}, tools = {...} }；启动时由 player/inventory.lua 的
--     register_items() 对每个条目 item.define 进引擎注册表；字段形状由
--     data/registry_check.lua 的 validate_item_extensions 用主表同一套 check_item 校验。
--   * 模块加载时把物品并入 require("data.items") 主表——否则 data/black_market.lua
--     的引用完整性校验、inv 入包后的显示名查询都会看不到这些违禁品（静默断链）。
--
-- 口径（docs/33 §5 黑市章草案）：
--   * 这些物品**只在黑市流通**（black_market.stock/buys），不进任何正规商店的
--     stock/buys（否则「黑市买 → 货摊卖」就是稳定套利）。正规商店（shops.lua）逐字不动。
--   * kind 取闭集内：material（暗器/甲胄/秘笈/鬼面等器物）、gem（血玉）、
--     consumable（迷香/烧刀子/虎骨/九转还魂丹等可服用物）。
--   * 夜明珠复用矿脉既有条目 luminous_pearl（items_mine.lua，sell=900），不重复声明
--     （重复声明会被 check_tool「id also exists」类校验挡住，且污染图鉴派生）。
--   * icon 为占位（assets/art/items/<id>.png），正式美术随改造计划阶段4管线替换
--     （registry_check 只校验路径前缀 assets/，不查文件存在性）。
local base = require("data.items")

local contraband = {
  -- ---- 违禁器物（kind=material：只在黑市买卖，不进正规商店）----
  poison_dagger = {
    name = "淬毒暗器",
    description = "喂了见血封喉麻药的细刃针筒，江湖宵小的看家家伙，官面上见不得光。",
    icon = "assets/art/items/poison_dagger.png",
    stack = 99,
    buy = 180,
    sell = 40,
    kind = "material"
  },
  silk_armor = {
    name = "金丝软甲",
    description = "天蚕丝织就的贴身软甲，刀枪难入，来历不明的锦衣侍卫之物。",
    icon = "assets/art/items/silk_armor.png",
    stack = 99,
    buy = 320,
    sell = 90,
    kind = "material"
  },
  forbidden_manual = {
    name = "残页秘笈",
    description = "半本被师门查禁的手抄残卷，墨迹淋漓，练岔了容易走火入魔。",
    icon = "assets/art/items/forbidden_manual.png",
    stack = 99,
    buy = 260,
    sell = 60,
    kind = "material"
  },
  demon_mask = {
    name = "鬼面",
    description = "绘着凶神恶煞的木雕面具，夜行人戴它遮脸，也遮不住心虚。",
    icon = "assets/art/items/demon_mask.png",
    stack = 99,
    buy = 150,
    sell = 50,
    kind = "material"
  },
  -- ---- 黑市宝石（kind=gem：与矿脉宝石同级，来路不正）----
  blood_jade = {
    name = "血玉",
    description = "浸了人血气的暗红玉石，温润中透着一股煞气，正经玉铺不敢收。",
    icon = "assets/art/items/blood_jade.png",
    stack = 99,
    buy = 300,
    sell = 130,
    kind = "gem"
  },
  -- ---- 可服用物（kind=consumable：use 只走 energy/inner_power 白名单键）----
  revival_pill = {
    name = "九转还魂丹",
    description = "黑市坐地起价的救命丹药，服下大补元气与内力，丹方却经不起官面盘问。",
    icon = "assets/art/items/revival_pill.png",
    stack = 99,
    buy = 500,
    sell = 200,
    kind = "consumable",
    use = { energy = 30, inner_power = 50 }
  },
  smuggled_salt = {
    name = "私盐",
    description = "未经官营盐课的贩私盐，成色不差却犯禁，夜里才敢私下过秤。",
    icon = "assets/art/items/smuggled_salt.png",
    stack = 99,
    buy = 25,
    sell = 10,
    kind = "consumable"
  },
  smuggled_liquor = {
    name = "烧刀子",
    description = "私酿的烈酒，入口如火，跑江湖的夜里暖身子，税册上却没有这一桶。",
    icon = "assets/art/items/smuggled_liquor.png",
    stack = 99,
    buy = 40,
    sell = 15,
    kind = "consumable",
    use = { energy = 25 }
  },
  poison_powder = {
    name = "迷香",
    description = "点燃后能迷倒看家护院的闷香，拍花子与夜行者的勾当，正经人家避之不及。",
    icon = "assets/art/items/poison_powder.png",
    stack = 99,
    buy = 60,
    sell = 20,
    kind = "consumable",
    use = { inner_power = 15 }
  },
  tiger_bone = {
    name = "虎骨",
    description = "私猎得来的虎骨，泡酒壮骨，可私猎走兽本就犯了山禁。",
    icon = "assets/art/items/tiger_bone.png",
    stack = 99,
    buy = 90,
    sell = 35,
    kind = "consumable",
    use = { energy = 20, inner_power = 10 }
  }
}

-- 并入主表（见文件头注释：让 data/black_market.lua 校验、inv 显示名查询都看得见）。
for id, definition in pairs(contraband) do
  base[id] = definition
end

return { items = contraband, tools = {} }
