-- data/quests/mainline_seal.lua：主线第三段·药香落印（M19-01 纯数据）。
--
-- 收束段：三笔订单全部完成 + 手上留有一份**自家炼出的行气散**。
--   行气散（herb_tonic）是 data/recipes 的 tonic_refine 产物，本剧本里的
--   来源是「订单·山庄丸药」的交付奖励（order_pill.rewards），也就是玩家
--   亲手炼药换来的成品 —— 与主线「药香落印」的叙事一致。
--   为什么不用回复药做这个目标：回复药在「订单·山庄丸药」交付时已被消耗
--   （objectives consume = true），而主线各段的交付顺序由玩家决定，不能假设
--   玩家会在交付订单前多炼一丸；用同一条奖励链上的行气散可保证顺序无关。
-- 目标全部用既有目标类型表达（flag ×3 + item ×1），因此可交付态是纯派生态，
-- 读档后由 state.vars / inventory / quest.records 重新推导，无需额外持久化。
-- 交付后 on_complete 写入主线锚点 mainline_done = true（「主线节点完成」）。
return {
  version = 1,
  id = "mainline_seal",
  name = "主线·药香落印",
  giver = "npc_shen_qingluo",
  summary = "三笔买卖皆已结清，再亮一份自家炼的行气散，沈青萝便认你这味药香。",
  objectives = {
    { id = "order_herb_done", kind = "flag", var = "order_herb_done", label = "完成药庐订单" },
    { id = "order_rice_done", kind = "flag", var = "order_rice_done", label = "完成市集订单" },
    { id = "order_pill_done", kind = "flag", var = "order_pill_done", label = "完成山庄订单" },
    { id = "bring_tonic", kind = "item", item = "herb_tonic", count = 1, label = "备一份行气散", consume = true }
  },
  rewards = {
    { kind = "money", amount = 120 },
    { kind = "reputation", faction = "village", amount = 15 },
    { kind = "reputation", faction = "sect", amount = 10 }
  },
  on_complete = {
    set_var = { mainline_done = true },
    relationship = { npc_shen_qingluo = 10 },
    reputation = { village = 5 }
  }
}
