-- icon：工具/武器的图标（M29 接线：`assets/art/items/hoe.png`、`blade.png` 此前
-- 生成好了但没有任何引用点）。字段可选——水壶目前还没有美术，缺 icon 时显示层按
-- `ui/inventory_ui.lua` 的文字兜底，不报错；有 icon 就必须是 assets/ 路径
-- （data/registry_check.lua 校验）。
--
-- tier：工具档位（docs/33 §5.4 工具升级，2026-10-05 0.3）。1 = 基础档，2–5 =
-- 精铁/寒铁/陨铁/灵铁。`systems/farm.lua` 翻土/浇水按装备工具 tier 减耗
-- （TIER_COST，基础档与 MVP 基线一致）；高阶工具由韩铁山铁铺独家售卖
-- （data/shops.lua han_forge），基础工具仍由新档/补发发放、不进商店。
--
-- family：工具家族（docs/33 §5.4）。`systems/farm.lua` 的 `equipped_tool(family)`
-- 按家族匹配——装备精铁锄头（hoe_iron）同样满足"锄头"判定，不能按 id 精确
-- 比较（否则高阶工具永远"没有装备"）。family 必须与基础工具 id 一致。
-- 图标占位：高阶锄/壶暂复用基础工具图（美术里程碑统一补新图，docs/48 登记）。
return {
  hoe = { name = "锄头", kind = "tool", family = "hoe", tier = 1, energy = 5, icon = "assets/art/items/hoe.png" },
  hoe_iron = { name = "精铁锄头", kind = "tool", family = "hoe", tier = 2, energy = 4, icon = "assets/art/items/hoe.png" },
  hoe_cold = { name = "寒铁锄头", kind = "tool", family = "hoe", tier = 3, energy = 3, icon = "assets/art/items/hoe.png" },
  hoe_meteor = { name = "陨铁锄头", kind = "tool", family = "hoe", tier = 4, energy = 2, icon = "assets/art/items/hoe.png" },
  hoe_spirit = { name = "灵铁锄头", kind = "tool", family = "hoe", tier = 5, energy = 1, icon = "assets/art/items/hoe.png" },
  -- 伐木斧 / 矿镐（2026-09-27，docs/48 接线）：青云山道松木桩与矿石点的
  -- requires 前置。新档直接发放（与 watering_can 同先例：商店不售工具、无任务奖励）。
  -- 升级线（斧/镐）随采集/采矿里程碑（gather 当前不耗体力），数据层保持基础档。
  axe = { name = "伐木斧", kind = "tool", family = "axe", tier = 1, energy = 6, icon = "assets/art/items/axe.png" },
  pick = { name = "矿镐", kind = "tool", family = "pick", tier = 1, energy = 6, icon = "assets/art/items/pick.png" },
  watering_can = { name = "水壶", kind = "tool", family = "watering_can", tier = 1, energy = 2 },
  water_iron = { name = "精铁水壶", kind = "tool", family = "watering_can", tier = 2, energy = 2 },
  water_cold = { name = "寒铁水壶", kind = "tool", family = "watering_can", tier = 3, energy = 1 },
  water_meteor = { name = "陨铁水壶", kind = "tool", family = "watering_can", tier = 4, energy = 1 },
  water_spirit = { name = "灵铁水壶", kind = "tool", family = "watering_can", tier = 5, energy = 1 },
  training_sword = {
    name = "练习剑",
    kind = "weapon",
    damage = 8,
    icon = "assets/art/items/blade.png"
  }
}
