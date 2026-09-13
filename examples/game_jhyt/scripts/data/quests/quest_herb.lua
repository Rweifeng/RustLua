-- data/quests/quest_herb.lua：沈青萝·为医馆采药（M17-03 纯数据）。
--
-- 字段：
--   id/name/giver/summary  身份与展示（giver 引用 data/npcs.lua 的 persist_id）
--   objectives             目标列表，判定"可交付"的依据
--     kind = "item"        按背包数量：item + count
--     kind = "var"         按剧情变量数值：var + count（>= count 即达成）
--     kind = "flag"        按剧情变量真值：var
--     consume = true       交付时消耗（仅 item 有效）
--   rewards                交付奖励，幂等发放一次
--     kind = "item"/"money"/"inner_power"/"relationship"/"reputation"
--     （relationship: npc + amount；reputation: faction + amount，M18-04）
--   on_complete            交付后的剧情变量副作用（set_var / add_var /
--                          relationship / reputation）
return {
  version = 1,
  id = "quest_herb",
  name = "为医馆采药",
  giver = "npc_shen_qingluo",
  summary = "沈青萝医馆缺灵芝入药，需采三株。",
  objectives = {
    { id = "gather_lingzhi", kind = "item", item = "lingzhi", count = 3, label = "采集灵芝", consume = true }
  },
  rewards = {
    { kind = "money", amount = 50 },
    { kind = "item", item = "rice", count = 2 },
    { kind = "relationship", npc = "npc_shen_qingluo", amount = 10 },
    { kind = "reputation", faction = "village", amount = 5 }
  },
  on_complete = {
    set_var = { herb_delivered = true }
  }
}
