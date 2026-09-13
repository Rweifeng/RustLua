-- data/quests/order_rice.lua：市集订单·稻米（M19-01 纯数据）。
--
-- 稻谷可经农田（seed_rice → 1 个有效成长日 → 收获 2）或山道野采获得，
-- 因此订单不需要额外前置即可独立完成。发奖含云泽村声望。
--
-- 发布者用林小满（云泽村·茶农）而不是货摊掌柜赵铁算盘：掌柜同时挂着
-- shop_keeper 交互目标（priority 20 > 对话 10），站到他面前按 E 只会开店，
-- 该 NPC 的对话在当前设计下不可达（详见 docs/46 §已知限制）。
return {
  version = 1,
  id = "order_rice",
  name = "订单·市集稻米",
  giver = "npc_lin_xiaoman",
  summary = "茶农林小满替过路商队代收稻谷，两斗起收。",
  objectives = {
    { id = "bring_rice", kind = "item", item = "rice", count = 2, label = "持有稻谷", consume = true }
  },
  rewards = {
    { kind = "money", amount = 45 },
    { kind = "reputation", faction = "village", amount = 6 }
  },
  on_complete = {
    set_var = { order_rice_done = true }
  }
}
