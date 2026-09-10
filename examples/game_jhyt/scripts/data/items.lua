-- data/items.lua：《江湖有田》物品注册表（M17-01，M17-04 补价格与使用效果）。
--
-- 字段：
--   name/description 显示名与说明
--   stack            最大堆叠（缺省 99）
--   buy              商店卖价（玩家购入价）；0 表示不出售
--   sell             商店收价（玩家卖出价）；0 表示不收购
--   kind             分类（crop/seed/consumable…），仅用于展示与筛选
--   use              可使用物品的效果表（M17-04 硬风险③）：
--                      energy       使用后体力增减（正数回复）
--                      inner_power  使用后内力增减
--                    无 use 字段即不可使用（如种子/收获物/工具）。
return {
  lingzhi = {
    name = "灵芝",
    description = "山庄药田的基础药材。",
    stack = 99,
    buy = 25,
    sell = 80,
    kind = "crop"
  },
  rice = {
    name = "稻谷",
    description = "江南常见作物。",
    stack = 99,
    buy = 10,
    sell = 30,
    kind = "crop"
  },
  seed_lingzhi = {
    name = "灵芝菌种",
    description = "用于种植灵芝。",
    stack = 99,
    buy = 25,
    sell = 5,
    kind = "seed"
  },
  seed_rice = {
    name = "稻种",
    description = "用于种植稻谷。",
    stack = 99,
    buy = 12,
    sell = 2,
    kind = "seed"
  },
  heal_pill = {
    name = "回复药",
    description = "云泽村药铺的常备丸药，服下可恢复体力。",
    stack = 9,
    buy = 60,
    sell = 20,
    kind = "consumable",
    use = { energy = 40 }
  }
}
