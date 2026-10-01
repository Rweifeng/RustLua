-- data/quests/mainline_truth.lua：主线第五段·真相浮现（v0.2）。
--
-- 三份信物 + 山魈内丹凑齐，陆归藏（师父）留书的解读浮出水面：
-- 玄冥会执事司徒魁才是窃书者，且有朝一日要登门夺最后半卷。
-- 玩家必须击败山魈头目（mainline_xuanming 已经拿到内丹），
-- 再交付 8 两铁锭（作为修复山庄防御的物资）来触发「结局抉择」对话。
return {
  version = 1,
  id = "mainline_truth",
  name = "主线·真相浮现",
  giver = "npc_lu_guizang",
  summary = "陆归藏留书的落款指向司徒魁；把八两铁锭交给顾长风，山庄要有一场硬仗。",
  objectives = {
    { id = "sealed_xuanming", kind = "flag", var = "mainline_xuanming_done", label = "已理清玄冥会线索" },
    { id = "bring_ingots", kind = "item", item = "iron_ingot", count = 8, label = "备 8 份铁锭", consume = true }
  },
  rewards = {
    { kind = "money", amount = 200 },
    { kind = "reputation", faction = "sect", amount = 25 }
  },
  on_complete = {
    set_var = { mainline_truth_done = true },
    relationship = { npc_lu_guizang = 20, npc_gu_changfeng = 10 }
  }
}
