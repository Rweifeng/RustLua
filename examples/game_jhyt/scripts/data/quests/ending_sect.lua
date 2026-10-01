-- data/quests/ending_sect.lua：结局二·承业（v0.2）。
-- 玩家接过青云门衣钵，苏晚晴与玄鹤见证，从此执掌山庄。
return {
  version = 1,
  id = "ending_sect",
  name = "结局·承业",
  giver = "npc_su_wanqing",
  summary = "苏晚晴递来青云掌门信物：山庄不能再无人。备三颗回气丹以壮行色。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "bring_pill", kind = "item", item = "qi_pill", count = 3, label = "备 3 颗回气丹", consume = true }
  },
  rewards = {
    { kind = "reputation", faction = "sect", amount = 50 }
  },
  on_complete = {
    set_var = { ending = "sect", ending_sect = true },
    relationship = { npc_su_wanqing = 30, npc_xuanhe = 20 }
  }
}
