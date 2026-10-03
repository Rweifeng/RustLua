-- data/quests/ending_vengeance.lua：结局四·雪恨（v0.2）。
-- 玩家主动出击，把玄冥会执事司徒魁留下的一件贴身物证带到山门，正面对峙。
-- 交付需要 ape_core + 狼皮，代表"练到家"与"敢下手"。
--
-- M34-01 的声望门槛：对峙要有人在山门前替你说话，压到"受信"（下限 40）。
-- 比承业（重视 60）低一档：雪恨是"你自己去"的路子，门派信你但不必把你当自己人。
return {
  version = 1,
  id = "ending_vengeance",
  name = "结局·雪恨",
  giver = "npc_xuanhe",
  summary = "玄鹤拂尘一摆：徒儿若要了结这段因果，便带上山魈内丹与狼皮，山门前候他。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "bring_core", kind = "item", item = "ape_core", count = 1, label = "备山魈内丹", consume = true },
    { id = "bring_pelt", kind = "item", item = "wolf_pelt", count = 3, label = "备 3 张狼皮作路费", consume = true },
    { id = "sect_trust", kind = "reputation", faction = "sect", tier = "trusted", label = "青云门派声望到受信" }
  },
  rewards = {
    { kind = "reputation", faction = "sect", amount = 40 }
  },
  on_complete = {
    set_var = { ending = "vengeance", ending_vengeance = true },
    relationship = { npc_xuanhe = 30 }
  }
}
