-- data/crops.lua：作物注册表。
--
-- 数值口径 = `examples/game_jhyt/docs/33-江湖有田-数值设计案.md` §4.1（作物表）。
-- `yield` 是**每块田收获一次的基础产量**：`systems/farm.lua` 收获时读
-- `crop.yield`；M23 之前这里没有 `yield` 字段，于是 `crop.yield or 1` 让每茬恒收 1
-- ——稻谷按设计案应为 2，属于"数据缺字段导致数值静默走兜底"。
-- 该字段现在由 `data/registry_check.lua` 强制必填（正整数），并由
-- `jhyt_m24_numeric_alignment` 同时校验"设计案表格 == 本文件"与"真实收获数量 == yield"。
return {
  lingzhi = {
    name = "灵芝",
    stages = 3,
    growth_days = 7,
    yield = 1,
    season = { "spring", "summer" },
    seed_item = "seed_lingzhi",
    harvest_item = "lingzhi",
    sell_price = 80,
    land = "medicinal"
  },
  rice = {
    name = "稻谷",
    stages = 3,
    growth_days = 4,
    yield = 2,
    season = { "spring", "summer", "autumn" },
    seed_item = "seed_rice",
    harvest_item = "rice",
    sell_price = 30,
    land = "field"
  },
  -- ---- Alpha 作物扩展（2026-09-27，docs/33 §4.1 同步 +4）----
  -- 萝卜：快熟教学作物，早期现金流；2 天即收（唯一 2 天作物）。
  radish = {
    name = "萝卜",
    stages = 3,
    growth_days = 2,
    yield = 2,
    season = { "spring", "summer" },
    seed_item = "seed_radish",
    harvest_item = "radish",
    sell_price = 8,
    land = "field"
  },
  -- 草药：制药原料（§4.2 药粉/内力丹配方待接），也是商店药材（§5.2 草药转正）。
  herb = {
    name = "草药",
    stages = 3,
    growth_days = 3,
    yield = 2,
    season = { "spring", "summer", "autumn" },
    seed_item = "seed_herb",
    harvest_item = "herb",
    sell_price = 8,
    land = "medicinal"
  },
  -- 何首乌（docs/30 §6.3：8 天）：高价药材，礼物用。
  polygonum = {
    name = "何首乌",
    stages = 3,
    growth_days = 8,
    yield = 1,
    season = { "spring", "summer", "autumn" },
    seed_item = "seed_polygonum",
    harvest_item = "polygonum",
    sell_price = 120,
    land = "medicinal"
  },
  -- 人参（docs/30 §6.3：10 天）：高价药材、主线药材。
  ginseng = {
    name = "人参",
    stages = 3,
    growth_days = 10,
    yield = 1,
    season = { "spring", "autumn" },
    seed_item = "seed_ginseng",
    harvest_item = "ginseng",
    sell_price = 200,
    land = "medicinal"
  },
  -- ---- 四季作物矩阵扩展（2026-10-05，docs/33 §4.1 同步 +18，改造计划 §4.2/§5.3）----
  -- 规则：所有新作物只落在现有区划（field/medicinal）上；湿地/茶园/温室等新土地
  -- 待新地图里程碑接入（M37+）。`regrow_days` 为再生作物字段：成熟收获后地块
  -- 保留作物，再浇 `regrow_days` 天水即复熟（systems/farm.lua 收割逻辑）。
  -- 数值约束（docs/33 §5.4 经济曲线）：首茬利润/天落在 4–15 文区间；卖价 > 买价（无套利）。
  -- ---- 春 ----
  -- 薄荷：药圃再生作物，制药与茶饮原料；首茬 3 天，之后每 3 天一茬。
  mint = {
    name = "薄荷",
    stages = 3,
    growth_days = 3,
    regrow_days = 3,
    yield = 2,
    season = { "spring" },
    seed_item = "seed_mint",
    harvest_item = "mint",
    sell_price = 12,
    land = "medicinal"
  },
  -- 桑葚：耕地再生作物，果品；首茬 3 天，之后每 2 天一茬（最快的复熟作物）。
  mulberry = {
    name = "桑葚",
    stages = 3,
    growth_days = 3,
    regrow_days = 2,
    yield = 2,
    season = { "spring" },
    seed_item = "seed_mulberry",
    harvest_item = "mulberry",
    sell_price = 18,
    land = "field"
  },
  -- 青豌豆：春耕地豆类，快收。
  pea = {
    name = "青豌豆",
    stages = 3,
    growth_days = 4,
    yield = 2,
    season = { "spring" },
    seed_item = "seed_pea",
    harvest_item = "pea",
    sell_price = 25,
    land = "field"
  },
  -- ---- 夏 ----
  -- 青瓜：夏耕地再生作物，蔬菜；复熟 3 天。
  cucumber = {
    name = "青瓜",
    stages = 3,
    growth_days = 4,
    regrow_days = 3,
    yield = 2,
    season = { "summer" },
    seed_item = "seed_cucumber",
    harvest_item = "cucumber",
    sell_price = 30,
    land = "field"
  },
  -- 绿豆：夏耕地豆类，快收。
  mung_bean = {
    name = "绿豆",
    stages = 3,
    growth_days = 3,
    yield = 2,
    season = { "summer" },
    seed_item = "seed_mung_bean",
    harvest_item = "mung_bean",
    sell_price = 20,
    land = "field"
  },
  -- 夏枯草：夏药圃药材，入药（清肝明目）。
  prunella = {
    name = "夏枯草",
    stages = 3,
    growth_days = 5,
    yield = 1,
    season = { "summer" },
    seed_item = "seed_prunella",
    harvest_item = "prunella",
    sell_price = 70,
    land = "medicinal"
  },
  -- 茄子：夏秋两季耕地蔬菜。
  eggplant = {
    name = "茄子",
    stages = 3,
    growth_days = 5,
    yield = 2,
    season = { "summer", "autumn" },
    seed_item = "seed_eggplant",
    harvest_item = "eggplant",
    sell_price = 24,
    land = "field"
  },
  -- ---- 秋 ----
  -- 南瓜：秋耕地瓜果，节令食材（重阳宴）。
  pumpkin = {
    name = "南瓜",
    stages = 3,
    growth_days = 7,
    yield = 1,
    season = { "autumn" },
    seed_item = "seed_pumpkin",
    harvest_item = "pumpkin",
    sell_price = 80,
    land = "field"
  },
  -- 枸杞：秋药圃再生作物，药酒原料；复熟 4 天。
  gouqi = {
    name = "枸杞",
    stages = 3,
    growth_days = 5,
    regrow_days = 4,
    yield = 2,
    season = { "autumn" },
    seed_item = "seed_gouqi",
    harvest_item = "gouqi",
    sell_price = 25,
    land = "medicinal"
  },
  -- 白菜：秋耕地蔬菜。
  cabbage = {
    name = "白菜",
    stages = 3,
    growth_days = 5,
    yield = 2,
    season = { "autumn" },
    seed_item = "seed_cabbage",
    harvest_item = "cabbage",
    sell_price = 25,
    land = "field"
  },
  -- 菊花：秋药圃花卉，入药与赠礼。
  chrysanthemum = {
    name = "菊花",
    stages = 3,
    growth_days = 6,
    yield = 1,
    season = { "autumn" },
    seed_item = "seed_chrysanthemum",
    harvest_item = "chrysanthemum",
    sell_price = 60,
    land = "medicinal"
  },
  -- 韭葱：秋冬两季耕地蔬菜。
  leek = {
    name = "韭葱",
    stages = 3,
    growth_days = 6,
    yield = 2,
    season = { "autumn", "winter" },
    seed_item = "seed_leek",
    harvest_item = "leek",
    sell_price = 28,
    land = "field"
  },
  -- ---- 冬 ----
  -- 冬麦：冬耕地粮作，冬季主要现金流之一。
  winter_wheat = {
    name = "冬麦",
    stages = 3,
    growth_days = 6,
    yield = 2,
    season = { "winter" },
    seed_item = "seed_winter_wheat",
    harvest_item = "winter_wheat",
    sell_price = 35,
    land = "field"
  },
  -- 雪里蕻：冬耕地再生作物，腌菜原料；复熟 4 天。
  xueli = {
    name = "雪里蕻",
    stages = 3,
    growth_days = 5,
    regrow_days = 4,
    yield = 2,
    season = { "winter" },
    seed_item = "seed_xueli",
    harvest_item = "xueli",
    sell_price = 20,
    land = "field"
  },
  -- 冬参：冬药圃高价药材，冬季药田主力。
  winter_shen = {
    name = "冬参",
    stages = 3,
    growth_days = 8,
    yield = 1,
    season = { "winter" },
    seed_item = "seed_winter_shen",
    harvest_item = "winter_shen",
    sell_price = 150,
    land = "medicinal"
  },
  -- 腊梅：冬药圃花卉，节气赠礼（霜降后采）。
  plum = {
    name = "腊梅",
    stages = 3,
    growth_days = 5,
    yield = 1,
    season = { "winter" },
    seed_item = "seed_plum",
    harvest_item = "plum",
    sell_price = 90,
    land = "medicinal"
  },
  -- 冬白菜：冬耕地快熟蔬菜。
  winter_cabbage = {
    name = "冬白菜",
    stages = 3,
    growth_days = 4,
    yield = 2,
    season = { "winter" },
    seed_item = "seed_winter_cabbage",
    harvest_item = "winter_cabbage",
    sell_price = 18,
    land = "field"
  },
  -- 冬青：冬药圃再生作物，常绿药材；复熟 3 天。
  evergreen = {
    name = "冬青",
    stages = 3,
    growth_days = 4,
    regrow_days = 3,
    yield = 2,
    season = { "winter" },
    seed_item = "seed_evergreen",
    harvest_item = "evergreen",
    sell_price = 22,
    land = "medicinal"
  },
  -- ---- 果树（2026-10-05，docs/33 §4.1.5 新增）----
  -- 树 = 长生命周期地块：不浇水/不施肥/无品质档/跨年保留。tree=true 由
  -- systems/farm.lua 识别：settle_day 每日 tree_days +1（浇水无关），成树后
  -- 每年 fruit_season 每天结 1 果（当日限次）。growth_days 保留常规值以便
  -- 既有校验（validate_crops growth_days>=1）通过，实际成长走 tree_days。
  peach = {
    name = "桃树",
    stages = 1,
    growth_days = 15,
    tree = true,
    tree_days = 15,
    yield = 1,
    season = { "spring", "summer", "autumn", "winter" },
    seed_item = "seed_peach",
    harvest_item = "peach",
    sell_price = 25,
    land = "field",
    fruit_season = "autumn"
  },
  mume = {
    name = "青梅树",
    stages = 1,
    growth_days = 15,
    tree = true,
    tree_days = 15,
    yield = 1,
    season = { "spring", "summer", "autumn", "winter" },
    seed_item = "seed_mume",
    harvest_item = "mume",
    sell_price = 20,
    land = "field",
    fruit_season = "autumn"
  },
  loquat = {
    name = "枇杷树",
    stages = 1,
    growth_days = 15,
    tree = true,
    tree_days = 15,
    yield = 1,
    season = { "spring", "summer", "autumn", "winter" },
    seed_item = "seed_loquat",
    harvest_item = "loquat",
    sell_price = 30,
    land = "field",
    fruit_season = "autumn"
  },
  jujube = {
    name = "枣树",
    stages = 1,
    growth_days = 15,
    tree = true,
    tree_days = 15,
    yield = 1,
    season = { "spring", "summer", "autumn", "winter" },
    seed_item = "seed_jujube",
    harvest_item = "jujube",
    sell_price = 15,
    land = "field",
    fruit_season = "autumn"
  },
  ginkgo = {
    name = "银杏",
    stages = 1,
    growth_days = 15,
    tree = true,
    tree_days = 15,
    yield = 1,
    season = { "spring", "summer", "autumn", "winter" },
    seed_item = "seed_ginkgo",
    harvest_item = "ginkgo_fruit",
    sell_price = 45,
    land = "field",
    fruit_season = "autumn"
  }
}
