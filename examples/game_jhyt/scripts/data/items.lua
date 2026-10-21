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
--                      social_day   当日社交锚点（M34-05）：饮用后**今天**送礼/对话加关系分
--                                   吃 +10% 乘子（数字在 data/relationships.lua 的
--                                   social.tea_bonus）。同一日第二杯不再加分，
--                                   返回 tea_today 且不扣物品——与"每日一次礼物"同族语义。
--                    无 use 字段即不可使用（如种子/收获物/工具）。
--
-- 品质条目派生（docs/33 §4.1.2，2026-10-05 0.4）：本表在字面量之后对每个
-- kind="crop" 收获物自动派生 `<id>_good` / `<id>_super` 两条（sell = floor(基础 ×
-- 1.25/1.6)、buy = 0、图标占位复用基础图）。派生条目不进任何商店 stock/buys
-- （上品是"专属用途"物品：高级药引/送礼/节日订单）；新增作物收获物时无需手工
-- 补品质条目。派生只对字面量中已存在的 crop 生效（先收集再合并，避免二次派生）。
local items = {
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
  -- ---- Alpha 作物扩展（2026-09-27，docs/33 §4.1/§5.2 同步）----
  -- 萝卜：快熟收获物；商店收价 8（zhao_stall buys）。
  radish = {
    name = "萝卜",
    description = "两天就能收的白萝卜，最实惠的练手作物。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 8,
    kind = "crop"
  },
  seed_radish = {
    name = "萝卜籽",
    description = "用于种植萝卜（普通耕地）。",
    icon = "assets/art/items/seed_radish.png",
    stack = 99,
    buy = 4,
    sell = 0,
    kind = "seed"
  },
  -- 草药：§5.2「草药 8」从白名单转正（buy=8 为商店参考价；为防套利不进 stock，
  -- 只进 buys——与 rice/lingzhi 同构）。图标 assets/art/items/herb.png 此前是孤儿，落地后闭环。
  herb = {
    name = "草药",
    description = "田间种的药草，制药和卖店两用。",
    icon = "assets/art/items/herb.png",
    stack = 99,
    buy = 8,
    sell = 8,
    kind = "crop"
  },
  seed_herb = {
    name = "草药籽",
    description = "用于种植草药（药田）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 6,
    sell = 0,
    kind = "seed"
  },
  -- 何首乌：8 天高价药材，礼物用（docs/30 §6.3）。
  polygonum = {
    name = "何首乌",
    description = "块根乌紫的贵重药材，送礼也拿得出手。",
    icon = "assets/art/items/polygonum.png",
    stack = 99,
    buy = 0,
    sell = 120,
    kind = "crop"
  },
  seed_polygonum = {
    name = "何首乌苗",
    description = "用于种植何首乌（药田）。",
    icon = "assets/art/items/seed_polygonum.png",
    stack = 99,
    buy = 40,
    sell = 0,
    kind = "seed"
  },
  -- 人参：10 天最高价药材，主线药材（docs/30 §6.3）。
  ginseng = {
    name = "人参",
    description = "药田里的十年参，主线要用的贵重药材。",
    icon = "assets/art/items/ginseng.png",
    stack = 99,
    buy = 0,
    sell = 200,
    kind = "crop"
  },
  seed_ginseng = {
    name = "人参籽",
    description = "用于种植人参（药田）。",
    icon = "assets/art/items/seed_ginseng.png",
    stack = 99,
    buy = 60,
    sell = 0,
    kind = "seed"
  },
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
  },
  -- ---- v0.2 内容扩展（配方/商店/铁匠与膳房）----
  iron_ore = {
    name = "铁矿石",
    description = "青云山道深处采出的矿石，韩铁山愿高价收购。",
    icon = "assets/art/items/reed.png",
    stack = 99,
    buy = 0,
    sell = 20,
    kind = "material"
  },
  iron_ingot = {
    name = "铁锭",
    description = "炉火中锤炼出的铁锭，打刀铸剑的原料。",
    icon = "assets/art/items/timber.png",
    stack = 99,
    buy = 40,
    sell = 15,
    kind = "material"
  },
  cloth = {
    name = "麻布",
    description = "芦苇纤维织成的粗麻布，缝衣包头都用得上。",
    icon = "assets/art/items/reed.png",
    stack = 99,
    buy = 0,
    sell = 10,
    kind = "material"
  },
  rice_ball = {
    name = "饭团",
    description = "稻米捏成的小团，赶路时最顶饿。",
    icon = "assets/art/items/rice.png",
    stack = 9,
    buy = 15,
    sell = 4,
    kind = "consumable",
    use = { energy = 25 }
  },
  tea_brew = {
    name = "山泉清茶",
    description = "茶叶滚水冲开，饮一口宁神定气。",
    icon = "assets/art/items/tea_leaf.png",
    stack = 9,
    buy = 20,
    sell = 6,
    kind = "consumable",
    -- social_day（M34-05）：喝过茶的当天，送礼与对话的关系分 +10%（docs/33 §8.1）。
    use = { inner_power = 15, social_day = true }
  },
  mountain_herb_soup = {
    name = "行药汤",
    description = "灵芝与山草药同煮，服后气血充盈。",
    icon = "assets/art/items/herb_tonic.png",
    stack = 9,
    buy = 0,
    sell = 30,
    kind = "consumable",
    use = { buff = { stat = "attack", amount = 5, days = 3 } }
  },
  qi_pill = {
    name = "回气丹",
    description = "人参灵芝同炼的大补丸，服之真气涌动。",
    icon = "assets/art/items/heal_pill.png",
    stack = 9,
    buy = 0,
    sell = 80,
    kind = "consumable",
    use = { energy = 60, inner_power = 30 }
  },
  -- ---- 四季作物矩阵扩展（2026-10-05，docs/33 §4.1/§5.2 同步 +36）----
  -- 图标为**占位复用**（与 iron_ore→reed.png 同一资源策略）：新作物美术在改造计划
  -- 阶段 4（M51+）批量替换，接线表（docs/48）届时补登记；种子 buy=商店价、
  -- 收获物 sell=货摊收价，买卖方向分离（无套利）。
  -- ---- 春 ----
  mint = {
    name = "薄荷",
    description = "药圃里种出的清凉草药，制药与茶饮两用。",
    icon = "assets/art/items/herb.png",
    stack = 99,
    buy = 0,
    sell = 12,
    kind = "crop"
  },
  seed_mint = {
    name = "薄荷籽",
    description = "用于种植薄荷（药田，再生作物）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 10,
    sell = 0,
    kind = "seed"
  },
  mulberry = {
    name = "桑葚",
    description = "桑树结的紫红浆果，酸甜可口。",
    icon = "assets/art/items/wild_fruit.png",
    stack = 99,
    buy = 0,
    sell = 18,
    kind = "crop"
  },
  seed_mulberry = {
    name = "桑葚籽",
    description = "用于种植桑葚（普通耕地，再生作物）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 14,
    sell = 0,
    kind = "seed"
  },
  pea = {
    name = "青豌豆",
    description = "春日田里的青豌豆，豆荚饱满。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 25,
    kind = "crop"
  },
  seed_pea = {
    name = "青豌豆籽",
    description = "用于种植青豌豆（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 15,
    sell = 0,
    kind = "seed"
  },
  -- ---- 夏 ----
  cucumber = {
    name = "青瓜",
    description = "夏日顶花带刺的嫩青瓜，解暑佳品。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 30,
    kind = "crop"
  },
  seed_cucumber = {
    name = "青瓜籽",
    description = "用于种植青瓜（普通耕地，再生作物）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 18,
    sell = 0,
    kind = "seed"
  },
  mung_bean = {
    name = "绿豆",
    description = "夏日田里的绿豆，煮汤熬粥两相宜。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 20,
    kind = "crop"
  },
  seed_mung_bean = {
    name = "绿豆籽",
    description = "用于种植绿豆（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 12,
    sell = 0,
    kind = "seed"
  },
  prunella = {
    name = "夏枯草",
    description = "夏至前后开花的药草，清肝明目。",
    icon = "assets/art/items/herb.png",
    stack = 99,
    buy = 0,
    sell = 70,
    kind = "crop"
  },
  seed_prunella = {
    name = "夏枯草籽",
    description = "用于种植夏枯草（药田）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 20,
    sell = 0,
    kind = "seed"
  },
  eggplant = {
    name = "茄子",
    description = "紫皮长茄，秋日下饭的好菜。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 24,
    kind = "crop"
  },
  seed_eggplant = {
    name = "茄子籽",
    description = "用于种植茄子（普通耕地，夏秋两季）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 16,
    sell = 0,
    kind = "seed"
  },
  -- ---- 秋 ----
  pumpkin = {
    name = "南瓜",
    description = "秋收的圆南瓜，重阳宴上的节令瓜果。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 80,
    kind = "crop"
  },
  seed_pumpkin = {
    name = "南瓜籽",
    description = "用于种植南瓜（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 30,
    sell = 0,
    kind = "seed"
  },
  gouqi = {
    name = "枸杞",
    description = "秋日药圃里的红枸杞，泡酒入药皆宜。",
    icon = "assets/art/items/wild_fruit.png",
    stack = 99,
    buy = 0,
    sell = 25,
    kind = "crop"
  },
  seed_gouqi = {
    name = "枸杞籽",
    description = "用于种植枸杞（药田，再生作物）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 20,
    sell = 0,
    kind = "seed"
  },
  cabbage = {
    name = "白菜",
    description = "秋田里卷心的大白菜，冬储主菜。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 25,
    kind = "crop"
  },
  seed_cabbage = {
    name = "白菜籽",
    description = "用于种植白菜（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 15,
    sell = 0,
    kind = "seed"
  },
  chrysanthemum = {
    name = "菊花",
    description = "秋日盛开的菊花，入药清火，也是体面赠礼。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 0,
    sell = 60,
    kind = "crop"
  },
  seed_chrysanthemum = {
    name = "菊花籽",
    description = "用于种植菊花（药田）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 15,
    sell = 0,
    kind = "seed"
  },
  leek = {
    name = "韭葱",
    description = "秋冬两季皆可长的韭葱，炒蛋包馅都好。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 28,
    kind = "crop"
  },
  seed_leek = {
    name = "韭葱籽",
    description = "用于种植韭葱（普通耕地，秋冬两季）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 18,
    sell = 0,
    kind = "seed"
  },
  -- ---- 冬 ----
  winter_wheat = {
    name = "冬麦",
    description = "雪下过冬的麦子，来年开春前最后一茬收成。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 35,
    kind = "crop"
  },
  seed_winter_wheat = {
    name = "冬麦籽",
    description = "用于种植冬麦（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 20,
    sell = 0,
    kind = "seed"
  },
  xueli = {
    name = "雪里蕻",
    description = "耐寒的腌菜用菜，霜雪里也能长。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 20,
    kind = "crop"
  },
  seed_xueli = {
    name = "雪里蕻籽",
    description = "用于种植雪里蕻（普通耕地，再生作物）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 15,
    sell = 0,
    kind = "seed"
  },
  winter_shen = {
    name = "冬参",
    description = "雪原药田里挖出的冬参，药性温补。",
    icon = "assets/art/items/ginseng.png",
    stack = 99,
    buy = 0,
    sell = 150,
    kind = "crop"
  },
  seed_winter_shen = {
    name = "冬参籽",
    description = "用于种植冬参（药田）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 45,
    sell = 0,
    kind = "seed"
  },
  plum = {
    name = "腊梅",
    description = "霜降后盛开的腊梅，冬日里的一缕清香。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 0,
    sell = 90,
    kind = "crop"
  },
  seed_plum = {
    name = "腊梅籽",
    description = "用于种植腊梅（药田）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 25,
    sell = 0,
    kind = "seed"
  },
  winter_cabbage = {
    name = "冬白菜",
    description = "雪地里快熟的菜，寒冬里最实惠的收成。",
    icon = "assets/art/items/radish.png",
    stack = 99,
    buy = 0,
    sell = 18,
    kind = "crop"
  },
  seed_winter_cabbage = {
    name = "冬白菜籽",
    description = "用于种植冬白菜（普通耕地）。",
    icon = "assets/art/items/seed_rice.png",
    stack = 99,
    buy = 12,
    sell = 0,
    kind = "seed"
  },
  evergreen = {
    name = "冬青",
    description = "常绿的冬青叶，四季入药，寒冬不凋。",
    icon = "assets/art/items/herb.png",
    stack = 99,
    buy = 0,
    sell = 22,
    kind = "crop"
  },
  seed_evergreen = {
    name = "冬青籽",
    description = "用于种植冬青（药田，再生作物）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 18,
    sell = 0,
    kind = "seed"
  },
  -- ---- 肥料（2026-10-05，docs/33 §4.1.3 新增）----
  -- 效果：quality = 收获至少良好（保底）；growth = 成熟所需浇水天数 -1；
  -- yield = 每茬产量 +floor(yield × 0.5)。售价 = 货摊买入价（zhao_stall stock），
  -- sell=0（肥料不进商店 buys，阻断"买肥→卖肥"套利）。
  herb_fertilizer = {
    name = "草药肥",
    description = "以药渣沤制的肥，施过的地收获至少为良好品质。",
    icon = "assets/art/items/herb.png",
    stack = 99,
    buy = 20,
    sell = 0,
    kind = "fertilizer",
    effect = "quality"
  },
  bone_fertilizer = {
    name = "骨肥",
    description = "兽骨烧成的灰肥，作物早熟一日。",
    icon = "assets/art/items/timber.png",
    stack = 99,
    buy = 25,
    sell = 0,
    kind = "fertilizer",
    effect = "growth"
  },
  spirit_fertilizer = {
    name = "灵肥",
    description = "灵气浸润的肥，单茬收成更丰。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 30,
    sell = 0,
    kind = "fertilizer",
    effect = "yield"
  },
  -- ---- 洒水器（2026-10-05，docs/33 §4.1.4 新增）----
  -- 获得：仅铁匠铺 forge 配方制作（data/recipes.lua），buy=0/sell=0 不进商店
  -- （阻断套利，与肥料口径一致）。sprinkler 字段 = { shape = "cross"/"square",
  -- radius = 1/2 }：cross=十字 4 邻，square=方阵（radius1=3×3、radius2=5×5）；
  -- systems/farm.lua 的 settle_day 按此清晨自动浇水（图标为占位复用）。
  sprinkler_bamboo = {
    name = "竹筒引水器",
    description = "竹筒引水的简易洒水器，每日清晨浇灌上下左右四格。",
    icon = "assets/art/items/timber.png",
    stack = 99,
    buy = 0,
    sell = 0,
    kind = "sprinkler",
    sprinkler = { shape = "cross", radius = 1 }
  },
  sprinkler_waterwheel = {
    name = "水车洒水器",
    description = "水车驱动的洒水器，每日清晨浇灌 3×3 田块。",
    icon = "assets/art/items/reed.png",
    stack = 99,
    buy = 0,
    sell = 0,
    kind = "sprinkler",
    sprinkler = { shape = "square", radius = 1 }
  },
  sprinkler_machine = {
    name = "机关洒水器",
    description = "机关驱动的洒水器，每日清晨浇灌 5×5 田块。",
    icon = "assets/art/items/timber.png",
    stack = 99,
    buy = 0,
    sell = 0,
    kind = "sprinkler",
    sprinkler = { shape = "square", radius = 2 }
  },
  -- ---- 果树苗（2026-10-05，docs/33 §4.1.5 新增）----
  -- kind=seed（复用播种链路），货摊 zhao_stall 有售；sell=0 阻断"买苗卖苗"套利。
  -- 对应树条目在 data/crops.lua（tree=true），由 farm.settle_day 按 tree_days 成长。
  seed_peach = {
    name = "桃树苗",
    description = "桃树幼苗，种下 15 天成树，每年秋季结果。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 150,
    sell = 0,
    kind = "seed"
  },
  seed_mume = {
    name = "青梅树苗",
    description = "青梅树幼苗，种下 15 天成树，每年秋季结果。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 120,
    sell = 0,
    kind = "seed"
  },
  seed_loquat = {
    name = "枇杷树苗",
    description = "枇杷树幼苗，种下 15 天成树，每年秋季结果。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 180,
    sell = 0,
    kind = "seed"
  },
  seed_jujube = {
    name = "枣树苗",
    description = "枣树幼苗，种下 15 天成树，每年秋季结果。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 100,
    sell = 0,
    kind = "seed"
  },
  seed_ginkgo = {
    name = "银杏树苗",
    description = "银杏幼苗，种下 15 天成树，每年秋季结果（银杏果为贵重药引）。",
    icon = "assets/art/items/seed_herb.png",
    stack = 99,
    buy = 300,
    sell = 0,
    kind = "seed"
  },
  -- ---- 树果（2026-10-05，docs/33 §4.1.5 新增）----
  -- kind=fruit：可卖（货摊 buys 收购）可送礼（data/relationships.lua），
  -- 无品质档（不进良好/上品派生——果树不走 quality_for）。
  peach = {
    name = "桃子",
    description = "秋熟鲜桃，送礼也拿得出手。",
    icon = "assets/art/items/peach.png",
    stack = 99,
    buy = 0,
    sell = 25,
    kind = "fruit"
  },
  mume = {
    name = "青梅",
    description = "秋熟青梅，酸甜开胃。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 20,
    kind = "fruit"
  },
  loquat = {
    name = "枇杷",
    description = "秋熟枇杷，润肺佳果。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 30,
    kind = "fruit"
  },
  jujube = {
    name = "红枣",
    description = "秋熟红枣，补气小食。",
    icon = "assets/art/items/rice.png",
    stack = 99,
    buy = 0,
    sell = 15,
    kind = "fruit"
  },
  ginkgo_fruit = {
    name = "银杏果",
    description = "白果入药，贵重药引，送礼极佳。",
    icon = "assets/art/items/lingzhi.png",
    stack = 99,
    buy = 0,
    sell = 45,
    kind = "fruit"
  }
}

-- 品质条目派生（docs/33 §4.1.2）：见文件头注释。派生条目不参与再派生。
local QUALITY_MULTIPLIERS = { good = 1.25, super = 1.6 }
local derived = {}
for id, def in pairs(items) do
  if def.kind == "crop" and type(def.sell) == "number" and def.sell > 0 then
    for suffix, multiplier in pairs(QUALITY_MULTIPLIERS) do
      local quality_id = id .. "_" .. suffix
      if not items[quality_id] then
        derived[quality_id] = {
          name = def.name .. (suffix == "good" and "（良好）" or "（上品）"),
          description = (def.description or "") .. (suffix == "good"
            and " 品质良好，卖店价上浮。" or " 品质上品，留作高级药引、送礼或节日订单。"),
          icon = def.icon,
          stack = def.stack or 99,
          buy = 0,
          sell = math.floor(def.sell * multiplier),
          kind = "crop"
        }
      end
    end
  end
end
for id, def in pairs(derived) do
  items[id] = def
end

return items