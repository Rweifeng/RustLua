-- icon：工具/武器的图标（M29 接线：`assets/art/items/hoe.png`、`blade.png` 此前
-- 生成好了但没有任何引用点）。字段可选——水壶目前还没有美术，缺 icon 时显示层按
-- `ui/inventory_ui.lua` 的文字兜底，不报错；有 icon 就必须是 assets/ 路径
-- （data/registry_check.lua 校验）。
return {
  hoe = { name = "锄头", kind = "tool", energy = 5, icon = "assets/art/items/hoe.png" },
  -- 伐木斧 / 矿镐（2026-09-27，docs/48 接线）：青云山道松木桩与矿石点的
  -- requires 前置。新档直接发放（与 water_can 同先例：商店不售工具、无任务奖励）。
  axe = { name = "伐木斧", kind = "tool", energy = 6, icon = "assets/art/items/axe.png" },
  pick = { name = "矿镐", kind = "tool", energy = 6, icon = "assets/art/items/pick.png" },
  watering_can = { name = "水壶", kind = "tool", energy = 2 },
  training_sword = {
    name = "练习剑",
    kind = "weapon",
    damage = 8,
    icon = "assets/art/items/blade.png"
  }
}
