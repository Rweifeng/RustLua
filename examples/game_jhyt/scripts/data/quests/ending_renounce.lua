-- data/quests/ending_renounce.lua：结局三·断念（v0.2）。
-- 玩家自废武学，把行气散留给村民，从此不再卷入江湖恩怨。
return {
  version = 1,
  id = "ending_renounce",
  name = "结局·断念",
  giver = "npc_gu_changfeng",
  summary = "顾长风说：放下比拿起更难。真要走这条路，就把五份行气散留在剑坪。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "bring_tonic", kind = "item", item = "herb_tonic", count = 5, label = "备 5 份行气散作最后的补给", consume = true }
  },
  rewards = {
    { kind = "money", amount = 200 }
  },
  on_complete = {
    set_var = { ending = "renounce", ending_renounce = true },
    relationship = { npc_gu_changfeng = 25 }
  }
}
