-- data/quests/order_pill.lua：山庄订单·丸药（M19-01 纯数据）。
--
-- 回复药由药庐（herb_hut，station = herb）用 pill_refine 配方炼制
-- （灵芝 ×2 → 回复药 ×1），因此本订单把「种田/采集 → 制作 → 交付」串成一条线。
-- 奖励中的行气散是稀有消耗品（data/items.lua 已有，use.buff 加臂力 2 日）。
return {
  version = 1,
  id = "order_pill",
  name = "订单·山庄丸药",
  giver = "npc_gu_changfeng",
  summary = "顾长风托你炼一丸回复药，庄中弟子下山要用。",
  objectives = {
    { id = "bring_pill", kind = "item", item = "heal_pill", count = 1, label = "持有回复药", consume = true }
  },
  rewards = {
    { kind = "money", amount = 80 },
    { kind = "item", item = "herb_tonic", count = 1 },
    { kind = "relationship", npc = "npc_gu_changfeng", amount = 6 }
  },
  on_complete = {
    set_var = { order_pill_done = true }
  }
}
