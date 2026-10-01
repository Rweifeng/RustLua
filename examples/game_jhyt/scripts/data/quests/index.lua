-- data/quests/index.lua：任务注册表（M17-03 纯数据，M19-01 追加订单与主线）。
--
-- 新增一条任务 = 本文件加一个 id + data/quests/<id>.lua 一份定义，无需改动
-- quest.lua / dialogue.lua / Rust 核心（验收项①）。校验与遍历都以此表为准。
--
-- M19-01 追加：
--   * 三个订单（order_herb / order_rice / order_pill）——可独立完成的买卖
--   * 三段主线（mainline_arrival / mainline_orders / mainline_seal）
-- M28 追加：
--   * 山道猎获（order_hunt）——交付敌人掉落物（狼皮/兽肉），把战斗掉落接进经济
-- 顺序即任务日志（L 面板）的展示顺序。
return {
  version = 1,
  ids = {
    "quest_herb",
    "quest_spar",
    "order_herb",
    "order_rice",
    "order_pill",
    "order_hunt",
    "mainline_arrival",
    "mainline_orders",
    "mainline_seal",
    "mainline_xuanming",
    "mainline_truth",
    "ending_field",
    "ending_sect",
    "ending_renounce",
    "ending_vengeance"
  }
}
