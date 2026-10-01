-- data/shops.lua：《江湖有田》商店注册表（M17-04 纯数据）。
--
-- 新增商店 = 本文件加一条记录，无需改动 shop.lua / inventory.lua 与 Rust 核心
-- （验收项①）。商店的经济规则都在数据里声明，运行时只做校验与执行。
--
-- 字段：
--   id      商店 id（必须与键一致）
--   name    显示名
--   keeper  店主 NPC persist_id（引用 data/npcs.lua），兼任交互目标
--   place   营业地点名（引用 data/places.lua），仅用于诊断/展示，可为 nil
--   hours   营业时间窗口 { open, close }，单位＝当日分钟数（0..1440），
--           左闭右开；缺省即全天营业。跨午夜窗口不支持（close <= open 校验失败）
--   stock   本店卖给玩家的商品列表（玩家「购买」标签页）：
--             item   物品 id（引用 data/items.lua / data/tools.lua）
--             price  单价（缺省取物品 buy）；必须 > 0
--             limit  每日限购上限（缺省不限购）；必须 > 0
--   buys    本店从玩家收购的物品 id 列表（玩家「出售」标签页），
--           收购价取物品自身的 sell 字段，必须 > 0。
--
-- 套利约束（设计案「不允许 MVP 通过重复购买再出售稳定套利」）：
-- 同一物品不得同时出现在 stock 与 buys 中，shop.validate_all 会报告违规。
-- 因此本店只卖种子/消耗品、只收收获物，价格方向天然不可套利。
return {
  version = 1,
  zhao_stall = {
    id = "zhao_stall",
    name = "云泽村·货摊",
    keeper = "npc_zhao_tiesuanpan",
    place = "village_stall",
    -- 08:00 - 18:00，与赵铁算盘日程（480..1080）一致。
    hours = { open = 480, close = 1080 },
    stock = {
      { item = "seed_rice", price = 12, limit = 10 },
      { item = "seed_lingzhi", price = 25, limit = 5 },
      { item = "heal_pill", price = 60, limit = 3 },
      -- 建筑升级材料（M18-02）：不收购以阻断「买入再卖出」套利。
      { item = "timber", price = 15, limit = 20 },
      -- Alpha 作物扩展（2026-09-27，docs/33 §5.2）：四种新种子。
      { item = "seed_radish", price = 4, limit = 10 },
      { item = "seed_herb", price = 6, limit = 10 },
      { item = "seed_polygonum", price = 40, limit = 3 },
      { item = "seed_ginseng", price = 60, limit = 2 }
    },
    buys = { "rice", "lingzhi", "radish", "herb", "polygonum", "ginseng", "iron_ore" }
  },
  -- v0.2 韩铁山的铁匠铺：卖铁锭与建筑材料，收铁矿石/麻布/兽皮。
  han_forge = {
    id = "han_forge",
    name = "云泽村·韩氏铁铺",
    keeper = "npc_han_tieshan",
    place = "village_home_forge",
    hours = { open = 540, close = 1020 },
    stock = {
      { item = "iron_ingot", price = 40, limit = 6 },
      { item = "timber", price = 18, limit = 15 }
    },
    buys = { "iron_ore", "cloth", "wolf_pelt", "boar_meat" }
  },
  -- v0.2 阿七的行商摊：卖熟食（饭团/清茶），收莲子野果等山货。
  a_qi_provisions = {
    id = "a_qi_provisions",
    name = "村口·阿七食摊",
    keeper = "npc_a_qi",
    place = "village_plaza",
    hours = { open = 420, close = 1200 },
    stock = {
      { item = "rice_ball", price = 15, limit = 8 },
      { item = "tea_brew", price = 20, limit = 5 }
    },
    buys = { "lotus_seed", "wild_fruit", "tea_leaf", "reed" }
  }
}
