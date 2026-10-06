-- data/questboard.lua：《江湖有田》任务板 3 层（docs/54 §4.7，阶段 2 / 2026-10-06 1.6，纯数据）。
--
-- 任务板是**独立闭环**的新系统（不接入 world/quest.lua 状态机）：板子自己持有
-- "当前挂出的单子 + 接单/交单表"，奖励发放与既有 quest/shipping/goals 共存不冲突。
-- 本文件只描述"每层板怎么刷、有哪些候选模板"，运行时见 systems/questboard.lua。
--
-- 三层定位（docs/54 §4.7）：
--   daily     村民日常订单：每天刷 3 单，奖励中小（送药/送菜/采药这类跑腿）。
--   weekly    镖局护送与清剿：每 7 天（第 7/14/21... 天）刷 1–2 单，奖励中高
--             （护镖/剿山贼/猎户大宗，含战斗掉落物交付）。
--   quarterly 商会季度大单：季初即 day 1/29/57/85（每 28 天一季）刷 1 单，量大价高。
--
-- 目标口径：objective 只引用**已存在物品**（data.items / data.tools 及其扩展模块）。
-- "护镖到青云门 / 清剿山道山贼"这类武侠主题，与既有 order_hunt 同构——落地为
-- 交付该活动的产物（铁矿/狼皮/兽肉等既有掉落或货物），不新造战斗计数钩子。
--
-- 模板字段：
--   id         全局唯一（任务板实例 id 按 layer:day:id 派生）
--   title      中文标题（武侠化命名）
--   layer      "daily" | "weekly" | "quarterly"
--   objective  { item = <既有物品id>, count = 正整数 }  交付即消耗
--   reward     { money = 正数, item = {id=,count=} 可选, rep = {faction=, amount=} 可选 }
--   min_day    可选：最早第几天才进候选池（默认 1）
local M = {}

M.version = 1

-- 层白名单（运行时与校验共用）。
M.LAYERS = { "daily", "weekly", "quarterly" }

-- 每层板的生成规则：挂出数量 / 刷新周期 / 奖励金币区间（设计口径，校验器据此区间核对模板）。
-- refresh 周期只做注释说明，实际刷新判定写在 systems/questboard.on_day_start：
--   daily     每个游戏日刷新；
--   weekly    day % 7 == 0（第 7/14/21/28... 天）刷新；
--   quarterly (day-1) % 28 == 0（第 1/29/57/85... 天）刷新。
M.layer_rules = {
  daily = {
    title = "村民日常订单",
    offers = 3,
    refresh = "daily",
    money_min = 20,
    money_max = 60,
  },
  weekly = {
    title = "镖局护送与清剿",
    offers = 2,
    refresh = "weekly",
    money_min = 80,
    money_max = 180,
  },
  quarterly = {
    title = "商会季度大单",
    offers = 1,
    refresh = "quarterly",
    money_min = 300,
    money_max = 600,
  },
}

-- 候选任务模板池（22 条：日常 10 / 周单 7 / 季单 5）。
-- 奖励金额落在各自层区间内（validate_all 守住），声望引用 data/relationships 的既有阵营
-- （village=云泽村 / sect=青云门派）。
M.templates = {
  -- ---- 日常层（村民跑腿，奖励中小）----
  {
    id = "qb_d_herb_path",
    title = "送药到寒泉洞",
    layer = "daily",
    objective = { item = "lingzhi", count = 2 },
    reward = { money = 40, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_d_rice_share",
    title = "云泽村柴米",
    layer = "daily",
    objective = { item = "rice", count = 3 },
    reward = { money = 30, rep = { faction = "village", amount = 1 } },
  },
  {
    id = "qb_d_autumn_tea",
    title = "收集秋茶",
    layer = "daily",
    objective = { item = "tea_leaf", count = 2 },
    reward = { money = 36, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_d_wildfruit",
    title = "野果换糖",
    layer = "daily",
    objective = { item = "wild_fruit", count = 3 },
    reward = { money = 24, rep = { faction = "village", amount = 1 } },
  },
  {
    id = "qb_d_mint",
    title = "药圃薄荷",
    layer = "daily",
    objective = { item = "mint", count = 2 },
    reward = { money = 36, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_d_reed",
    title = "编席扎帚",
    layer = "daily",
    objective = { item = "reed", count = 4 },
    reward = { money = 24, rep = { faction = "village", amount = 1 } },
  },
  {
    id = "qb_d_lotus",
    title = "野塘莲子",
    layer = "daily",
    objective = { item = "lotus_seed", count = 2 },
    reward = { money = 36, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_d_radish",
    title = "萝卜下饭",
    layer = "daily",
    objective = { item = "radish", count = 3 },
    reward = { money = 24, rep = { faction = "village", amount = 1 } },
  },
  {
    id = "qb_d_herb_dressing",
    title = "草药敷伤",
    layer = "daily",
    objective = { item = "herb", count = 2 },
    reward = { money = 24, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_d_mume_soup",
    title = "梅子酸汤",
    layer = "daily",
    objective = { item = "mume", count = 2 },
    reward = { money = 30, rep = { faction = "village", amount = 1 } },
  },

  -- ---- 周层（镖局护送与清剿，奖励中高）----
  {
    id = "qb_w_escort_qingyun",
    title = "护镖到青云门",
    layer = "weekly",
    objective = { item = "iron_ore", count = 2 },
    reward = { money = 120, rep = { faction = "village", amount = 3 } },
  },
  {
    id = "qb_w_bandits",
    title = "清剿山道山贼",
    layer = "weekly",
    objective = { item = "wolf_pelt", count = 2 },
    reward = { money = 110, rep = { faction = "village", amount = 4 } },
  },
  {
    id = "qb_w_boar_meat",
    title = "猎户大宗兽肉",
    layer = "weekly",
    objective = { item = "boar_meat", count = 3 },
    reward = { money = 90, rep = { faction = "village", amount = 3 } },
  },
  {
    id = "qb_w_iron_ingot",
    title = "韩铁山订料",
    layer = "weekly",
    objective = { item = "iron_ingot", count = 1 },
    reward = { money = 130, rep = { faction = "village", amount = 4 } },
  },
  {
    id = "qb_w_cloth",
    title = "麻布成衣",
    layer = "weekly",
    objective = { item = "cloth", count = 2 },
    reward = { money = 80, rep = { faction = "village", amount = 2 } },
  },
  {
    id = "qb_w_tea_sect",
    title = "送茶青云门",
    layer = "weekly",
    objective = { item = "tea_brew", count = 2 },
    reward = { money = 100, rep = { faction = "sect", amount = 4 } },
  },
  {
    id = "qb_w_heal_pill",
    title = "镖局伤药",
    layer = "weekly",
    objective = { item = "heal_pill", count = 2 },
    reward = { money = 90, rep = { faction = "village", amount = 3 } },
  },

  -- ---- 季层（商会大单，量大价高）----
  {
    id = "qb_q_gift",
    title = "商会年节礼",
    layer = "quarterly",
    objective = { item = "lingzhi", count = 5 },
    reward = { money = 400, rep = { faction = "village", amount = 8 } },
  },
  {
    id = "qb_q_tea_bulk",
    title = "秋茶大宗",
    layer = "quarterly",
    objective = { item = "tea_leaf", count = 8 },
    reward = { money = 360, rep = { faction = "village", amount = 8 } },
  },
  {
    id = "qb_q_steel",
    title = "铸剑精料",
    layer = "quarterly",
    objective = { item = "iron_ore", count = 6 },
    reward = { money = 420, rep = { faction = "village", amount = 10 } },
  },
  {
    id = "qb_q_brew",
    title = "酿酒订单",
    layer = "quarterly",
    objective = { item = "rice", count = 10 },
    reward = { money = 300, rep = { faction = "village", amount = 6 } },
  },
  {
    id = "qb_q_rarity",
    title = "寒泉洞珍稀药",
    layer = "quarterly",
    objective = { item = "polygonum", count = 2 },
    reward = { money = 480, rep = { faction = "sect", amount = 8 } },
  },
}

-- 按 id 取模板（运行时与校验共用）。
function M.template(id)
  if type(id) ~= "string" then
    return nil
  end
  for _, tpl in ipairs(M.templates) do
    if tpl.id == id then
      return tpl
    end
  end
  return nil
end

-- 某一层的候选模板（可选按 min_day 过滤；校验期 day=0 = 不过滤）。
function M.pool(layer, day)
  local out = {}
  for _, tpl in ipairs(M.templates) do
    if tpl.layer == layer then
      local min_day = tonumber(tpl.min_day) or 1
      local at_day = tonumber(day) or 0
      if at_day == 0 or at_day >= min_day then
        out[#out + 1] = tpl
      end
    end
  end
  return out
end

return M
