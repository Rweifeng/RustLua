-- data/quests/ending_sect.lua：结局二·承业（v0.2）。
-- 玩家接过青云门衣钵，苏晚晴与玄鹤见证，从此执掌山庄。
--
-- M34-01 的声望门槛：承业要门派把你当自己人，门槛压到"重视"（下限 60）。
-- 今天青云门派声望的四笔来路是主线初至 +5、主线封印 +10、主线玄冥会 +20、
-- 主线真相 +25 = 60，**恰好**踩在重视档下限上：理清真相（本任务的 flag 目标）
-- 与到档是同一刻发生的，所以这道门今天不是墙而是栏杆——jhyt_m34 的验收把
-- "主线走通后门派声望正好 60"钉成契约，后续任务（M34-02 节日、M34-05 茶棚）
-- 要加的就是把这 0 点余量撑开的来路。
return {
  version = 1,
  id = "ending_sect",
  name = "结局·承业",
  giver = "npc_su_wanqing",
  summary = "苏晚晴递来青云掌门信物：山庄不能再无人。备三颗回气丹以壮行色。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "bring_pill", kind = "item", item = "qi_pill", count = 3, label = "备 3 颗回气丹", consume = true },
    { id = "sect_esteem", kind = "reputation", faction = "sect", tier = "esteemed", label = "青云门派声望到重视" }
  },
  rewards = {
    { kind = "reputation", faction = "sect", amount = 50 }
  },
  on_complete = {
    set_var = { ending = "sect", ending_sect = true },
    relationship = { npc_su_wanqing = 30, npc_xuanhe = 20 }
  }
}
