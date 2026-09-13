-- data/quests/order_herb.lua：药庐订单·灵芝（M19-01 纯数据）。
--
-- 订单是可独立完成的「持有 N 件 → 交付」委托：材料经种田/采集获得，
-- 交付后消耗目标物并发奖，on_complete 推进主线变量 mainline_stage。
-- 交付幂等由 quest.lua 保证（completed 态落定后重复交付返回 already_completed）。
return {
  version = 1,
  id = "order_herb",
  name = "订单·药庐灵芝",
  giver = "npc_shen_qingluo",
  summary = "药庐常年收灵芝入药，交两株即可结账。",
  objectives = {
    { id = "bring_lingzhi", kind = "item", item = "lingzhi", count = 2, label = "持有灵芝", consume = true }
  },
  rewards = {
    { kind = "money", amount = 60 },
    { kind = "relationship", npc = "npc_shen_qingluo", amount = 8 }
  },
  on_complete = {
    set_var = { order_herb_done = true }
  }
}
