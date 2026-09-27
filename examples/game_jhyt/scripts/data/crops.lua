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
  }
}
