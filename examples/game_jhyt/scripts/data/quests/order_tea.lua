-- data/quests/order_tea.lua：春分茶会·茶叶订单（M34-02 内容批）。
--
-- 七条节气里只有春分带**订单**（docs/30 §12「春分茶会 | 茶叶订单和 NPC 社交」）。
-- 接取入口挂在沈青萝的对话里，且只在那三天出现（`cond.festival = "teafair"`）；
-- **交付不设节气门槛**——单子接了随时能交，"错过春分"只意味着听不到那句话，
-- 不会把已经接下的单子变成死单（这是本条与"封山"最大的区别：封山拒的是方向，不是委托）。
--
-- 云泽村 +8 的分量：M34-01 记下的账是"必得 8+15，可选 +6/+5 = 上限 34"，
-- 这一笔把上限抬到 42，于是**受信档（下限 40、购买折扣 8%）第一次成为内容可达的目标**；
-- 重视/核心两档仍然要等 M34-05 的社交收益才有消费路径（docs/33 §8.2 同步改账）。
return {
  version = 1,
  id = "order_tea",
  name = "订单·春分茶会",
  giver = "npc_shen_qingluo",
  summary = "春分茶会要用鲜叶，沈掌柜请你在茶会这几天备下六两茶叶。",
  objectives = {
    { id = "bring_tea_leaf", kind = "item", item = "tea_leaf", count = 6, label = "持有茶叶", consume = true }
  },
  rewards = {
    { kind = "money", amount = 90 },
    { kind = "relationship", npc = "npc_shen_qingluo", amount = 8 },
    { kind = "reputation", faction = "village", amount = 8 }
  },
  on_complete = {
    set_var = { order_tea_done = true }
  }
}
