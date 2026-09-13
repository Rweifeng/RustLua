-- data/quests/mainline_orders.lua：主线第二段·行走江湖（M19-01 纯数据）。
--
-- 两件事：
--   * flag  orders_started —— 已在任一处接下买卖（订单对话的接取选项 set_var）
--   * flag  order_herb_done / order_rice_done / order_pill_done
--                           —— 任意一笔订单结清（订单任务 on_complete 写入）。
--     三个 flag 是「或」语义，由本段对话的 cond 表达三选一。
--   * item  lingzhi ×1      —— 向沈青萝交一把采集来的药材（交付即消耗）
-- 交付后 mainline_orders_done = true，推进第三段。
return {
  version = 1,
  id = "mainline_orders",
  name = "主线·行走江湖",
  giver = "npc_shen_qingluo",
  summary = "接下村中买卖，结清一笔，再把采来的药材交给沈青萝验看。",
  objectives = {
    { id = "orders_started", kind = "flag", var = "orders_started", label = "接下一笔订单" },
    { id = "order_herb_done", kind = "flag", var = "order_herb_done", label = "结清药庐订单" },
    { id = "order_rice_done", kind = "flag", var = "order_rice_done", label = "结清市集订单" },
    { id = "order_pill_done", kind = "flag", var = "order_pill_done", label = "结清山庄订单" },
    { id = "bring_lingzhi", kind = "item", item = "lingzhi", count = 1, label = "交一把药材", consume = true }
  },
  rewards = {
    { kind = "money", amount = 40 },
    { kind = "reputation", faction = "village", amount = 8 }
  },
  on_complete = {
    set_var = { mainline_orders_done = true },
    relationship = { npc_shen_qingluo = 6 }
  }
}
