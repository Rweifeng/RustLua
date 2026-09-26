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
--                      buff         临时属性 Buff（M18-04）：
--                                   { stat=, amount=, days= }，经
--                                   inventory.use → stats.add_buff 入账
--                    无 use 字段即不可使用（如种子/收获物/工具）。
return {
  lingzhi = {
    name = "灵芝",
    description = "山庄药田的基础药材。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 25,
    sell = 80,
    kind = "crop"
  },
  rice = {
    name = "稻谷",
    description = "江南常见作物。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 10,
    sell = 30,
    kind = "crop"
  },
  seed_lingzhi = {
    name = "灵芝菌种",
    description = "用于种植灵芝。",
    icon = "assets/art/items/seed_lingzhi.png",
    stack = 99,
    buy = 25,
    sell = 5,
    kind = "seed"
  },
  seed_rice = {
    name = "稻种",
    description = "用于种植稻谷。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 12,
    sell = 2,
    kind = "seed"
  },
  heal_pill = {
    name = "回复药",
    description = "云泽村药铺的常备丸药，服下可恢复体力。",
    icon = "assets/art/items/heal_pill.png",
    stack = 9,
    buy = 60,
    sell = 20,
    kind = "consumable",
    use = { energy = 40 }
  },
  -- 行气散（M18-04）：临时 Buff 的具体来源——use.buff 经 inventory.use →
  -- stats.add_buff 入账（attack+3，2 个游戏日后过期）。
  herb_tonic = {
    name = "行气散",
    description = "以山间草药调配的行气散，服后气血涌动，短期内臂力增强。",
    icon = "assets/art/items/herb_tonic.png",
    stack = 9,
    buy = 40,
    sell = 12,
    kind = "consumable",
    use = { buff = { stat = "attack", amount = 3, days = 2 } }
  },
  -- 木料（M18-02）：建筑升级材料，云泽村货摊出售（纯数据新增，无代码改动）。
  timber = {
    name = "木料",
    description = "修缮与扩建山庄建筑所需的木料，云泽村货摊有售。",
    icon = "assets/art/items/timber.png",
    stack = 99,
    buy = 15,
    sell = 4,
    kind = "material"
  },
  -- ---- 敌人掉落物（M28 / 计划 G3：掉落表见 data/drops.lua）----
  -- 狼皮（演武场灰狼）：山道猎获订单（order_hunt）的交付物，也是"打猎"的第一份收获。
  wolf_pelt = {
    name = "狼皮",
    description = "灰狼身上剥下的整皮，村里人拿它换钱。",
    icon = "assets/art/items/wolf_pelt.png",
    stack = 99,
    buy = 0,
    sell = 22,
    kind = "material"
  },
  -- 兽肉（青云山道野猪）：猎获订单需要两份；卖出价按"打猎比种田更赚一点"定。
  boar_meat = {
    name = "兽肉",
    description = "山道野猪的鲜肉，村中饭桌上常见。",
    icon = "assets/art/items/boar_meat.png",
    stack = 99,
    buy = 0,
    sell = 12,
    kind = "material"
  },
  -- 山魈内丹（第一章 Boss 掉落）：主线物品，只进不出（buy/sell 都是 0，卖不掉）。
  ape_core = {
    name = "山魈内丹",
    description = "山魈首领体内凝出的内丹，寒气逼人。",
    icon = "assets/art/items/ape_core.png",
    stack = 9,
    buy = 0,
    sell = 0,
    kind = "material"
  },
  -- 莲子（2026-09-26，docs/48 接线）：青云山道野塘日刷采集物。
  -- 野采物不入商店（buy=0，与狼皮/兽肉口径一致）；sell 参照木料（4 文）上浮为稀有度。
  lotus_seed = {
    name = "莲子",
    description = "青云山道野塘里的莲子，煮粥入药两相宜。",
    icon = "assets/art/items/lotus_seed.png",
    stack = 99,
    buy = 0,
    sell = 18,
    kind = "material"
  },
  -- 芦苇（2026-09-26，docs/48 接线）：青云山道水边日刷采集物，大宗低值材料。
  reed = {
    name = "芦苇",
    description = "青云山道水边的芦苇，编席扎帚的好材料。",
    icon = "assets/art/items/reed.png",
    stack = 99,
    buy = 0,
    sell = 6,
    kind = "material"
  },
  -- 茶叶（2026-09-27，docs/48 接线）：青云山道茶棚东侧日刷采集物。
  -- 野采物不入商店（buy=0，与狼皮/兽肉/莲子口径一致）；sell 对齐兽肉（12 文）。
  tea_leaf = {
    name = "茶叶",
    description = "青云山道茶丛里的嫩叶，晒青后煮饮，回神解乏。",
    icon = "assets/art/items/tea_leaf.png",
    stack = 99,
    buy = 0,
    sell = 12,
    kind = "material"
  },
  -- 野果（2026-09-27，docs/48 接线）：青云山道野果树日刷采集物。
  -- 与野谷丛（wild_berry_1）相邻成片；sell 8 文（低值果品）。
  wild_fruit = {
    name = "野果",
    description = "青云山道野果树结的果子，酸甜解渴，也能入菜。",
    icon = "assets/art/items/wild_fruit.png",
    stack = 99,
    buy = 0,
    sell = 8,
    kind = "material"
  }
}