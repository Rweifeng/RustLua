-- data/fish.lua：《江湖有田》钓鱼水域与鱼种注册表（docs/33 §4.5，2026-10-06 1.1）。
--
-- 本文件是**钓鱼专属数据**（不进 data.items.lua 的物品注册表）：
--   * waters：4 处钓点（村溪/山湖/寒泉洞暗河/雨季涨水）；
--   * fish ：23 种鱼，覆盖四季与 4 水域。
-- 鱼的**物品定义**（kind=fish、icon、sell）在 data/items_fish.lua 的 ext.items 里
-- （player/inventory.lua 启动自动注册）；钓竿工具在同文件 ext.tools。
-- 本文件只承载"在哪钓、何时钓、多难、值多少钱"的钓鱼逻辑口径。
--
-- 字段：
--   water   归属钓点 id（waters 的键）
--   season  可钓季节数组（spring/summer/autumn/winter）；空/nil = 全年可钓
--   hours   可钓时段窗口 { from, to }（当日分钟，0..1440，左闭右开）；
--           nil = 全天。晨≈{420,720}（7:00–12:00）、昏≈{1020,1320}（17:00–22:00）、
--           夜≈{1320,1440}（22:00–24:00）。
--   difficulty 难度 1..5：影响上竿成功率与所需竿阶（systems/fishing.lua：
--           竿阶 t 可钓 difficulty <= t*2 的鱼；t=1→≤2、t=2→≤4、t=3→≤5）。
--   sell    商店收购价（文）；与 data/items_fish.lua 里同名 fish 物品的 sell 一致
--           （systems/fishing.lua.validate_all 会交叉校验双写漂移）。
local M = {
  version = 1,
}

-- 钓点（4 处）。season = 该钓点开放的季节（nil=全年）；weather = 额外天气闸
-- （nil=不限；"rainy"= 仅雨天开放，雨季涨水临时钓点）。
M.waters = {
  village_brook = {
    id = "village_brook",
    name = "村溪",
    description = "云泽村外的浅溪，水缓鱼小，入门下竿处。",
    temp = false,
    weather = nil, -- 不限天气
  },
  mountain_lake = {
    id = "mountain_lake",
    name = "山湖",
    description = "青云山深处的高山湖，水冷鱼肥，进阶钓场。",
    temp = false,
    weather = nil,
  },
  cave_river = {
    id = "cave_river",
    name = "寒泉洞暗河",
    description = "寒泉洞地下暗河，阴冷幽暗，藏稀有冷水鱼。",
    temp = false,
    season = { "winter" }, -- 仅冬季可入洞（其余季节洞内泥泞）
    weather = nil,
  },
  flood_pool = {
    id = "flood_pool",
    name = "雨季涨水潭",
    description = "夏日暴雨后村溪漫出的临时水潭，雨停即退（临时钓点）。",
    temp = true,
    season = { "summer" },
    weather = "rainy", -- 仅雨天才有水
  },
}

-- 鱼种（23 种）。sell 与 data/items_fish.lua 同名物品一致。
M.fish = {
  -- ===== 村溪（入门，难度 1..2）=====
  crucian = { id = "crucian", name = "鲫鱼", water = "village_brook", season = nil, hours = nil, difficulty = 1, sell = 12 },
  carp = { id = "carp", name = "鲤鱼", water = "village_brook", season = { "spring", "summer", "autumn" }, hours = nil, difficulty = 1, sell = 18 },
  whitebait = { id = "whitebait", name = "白条", water = "village_brook", season = { "spring", "summer" }, hours = { from = 420, to = 720 }, difficulty = 1, sell = 10 },
  goldfish = { id = "goldfish", name = "金鱼", water = "village_brook", season = { "spring" }, hours = { from = 420, to = 720 }, difficulty = 1, sell = 22 },
  loach = { id = "loach", name = "泥鳅", water = "village_brook", season = { "summer", "autumn" }, hours = { from = 1020, to = 1320 }, difficulty = 2, sell = 15 },
  catfish = { id = "catfish", name = "鲶鱼", water = "village_brook", season = { "summer" }, hours = { from = 1320, to = 1440 }, difficulty = 2, sell = 25 },
  pondfish = { id = "pondfish", name = "鳙鱼", water = "village_brook", season = { "summer" }, hours = nil, difficulty = 2, sell = 22 },
  yellowhead = { id = "yellowhead", name = "黄颡鱼", water = "village_brook", season = { "summer" }, hours = { from = 1320, to = 1440 }, difficulty = 2, sell = 28 },
  -- 特色种：桃花鱼（春日溪滩名产）
  peach = { id = "peach", name = "桃花鱼", water = "village_brook", season = { "spring" }, hours = { from = 420, to = 720 }, difficulty = 2, sell = 30 },

  -- ===== 山湖（进阶，难度 2..3）=====
  lakecarp = { id = "lakecarp", name = "青鱼", water = "mountain_lake", season = { "autumn" }, hours = nil, difficulty = 2, sell = 35 },
  grasscarp = { id = "grasscarp", name = "草鱼", water = "mountain_lake", season = { "summer", "autumn" }, hours = nil, difficulty = 2, sell = 28 },
  perch = { id = "perch", name = "鲈鱼", water = "mountain_lake", season = { "spring", "autumn" }, hours = { from = 1020, to = 1320 }, difficulty = 3, sell = 45 },
  mandarin = { id = "mandarin", name = "鳜鱼", water = "mountain_lake", season = { "spring", "summer" }, hours = { from = 420, to = 720 }, difficulty = 3, sell = 55 },
  trout = { id = "trout", name = "虹鳟", water = "mountain_lake", season = { "autumn", "winter" }, hours = { from = 420, to = 720 }, difficulty = 3, sell = 60 },
  silverfish = { id = "silverfish", name = "银鱼", water = "mountain_lake", season = { "spring" }, hours = { from = 1320, to = 1440 }, difficulty = 3, sell = 50 },
  eel = { id = "eel", name = "鳗鱼", water = "mountain_lake", season = { "summer" }, hours = { from = 1320, to = 1440 }, difficulty = 3, sell = 65 },

  -- ===== 寒泉洞暗河（稀有，冬季，难度 4..5）=====
  cavesilver = { id = "cavesilver", name = "寒泉银鱼", water = "cave_river", season = { "winter" }, hours = { from = 1320, to = 1440 }, difficulty = 4, sell = 120 },
  blindfish = { id = "blindfish", name = "盲鱼", water = "cave_river", season = { "winter" }, hours = { from = 1320, to = 1440 }, difficulty = 4, sell = 90 },
  icefish = { id = "icefish", name = "冰鱼", water = "cave_river", season = { "winter" }, hours = nil, difficulty = 4, sell = 110 },
  -- 特色种：雪莲鱼（暗河极珍稀，难度 5，三阶竿方可下）
  snowlotus = { id = "snowlotus", name = "雪莲鱼", water = "cave_river", season = { "winter" }, hours = nil, difficulty = 5, sell = 200 },

  -- ===== 雨季涨水潭（临时，夏日雨天，难度 2..4）=====
  floodcarp = { id = "floodcarp", name = "涨水鲤", water = "flood_pool", season = { "summer" }, hours = nil, difficulty = 3, sell = 40 },
  snakehead = { id = "snakehead", name = "黑鱼", water = "flood_pool", season = { "summer" }, hours = { from = 1020, to = 1320 }, difficulty = 4, sell = 70 },
  swangeel = { id = "swangeel", name = "黄鳝", water = "flood_pool", season = { "summer" }, hours = { from = 1320, to = 1440 }, difficulty = 3, sell = 45 },
}

return M
