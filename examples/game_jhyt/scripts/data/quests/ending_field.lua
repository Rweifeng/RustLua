-- data/quests/ending_field.lua：结局一·归田（v0.2）。
-- 玩家选择放下武名，回山庄专心把药田经营起来。交付即锁死 ending = "field"。
return {
  version = 1,
  id = "ending_field",
  name = "结局·归田",
  giver = "npc_shen_qingluo",
  summary = "沈青萝说：药田也是一种江湖。囤够稻谷，回山庄把这一亩三分地种成模样。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "field_rice", kind = "item", item = "rice", count = 20, label = "囤 20 份稻谷", consume = true }
  },
  rewards = {
    { kind = "money", amount = 500 },
    { kind = "reputation", faction = "village", amount = 30 }
  },
  on_complete = {
    set_var = { ending = "field", ending_field = true },
    relationship = { npc_shen_qingluo = 30 }
  }
}
