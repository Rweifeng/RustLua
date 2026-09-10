-- data/quests/quest_spar.lua：顾长风·木桩三式（M17-03 纯数据）。
-- 目标用剧情变量计数（sword_drills），由对话选项的 add_var 副作用推进。
return {
  id = "quest_spar",
  name = "木桩三式",
  giver = "npc_gu_changfeng",
  summary = "随顾长风在山庄练功场走完三式基础刀法。",
  objectives = {
    { id = "drills", kind = "var", var = "sword_drills", count = 3, label = "练习基础刀法" }
  },
  rewards = {
    { kind = "inner_power", amount = 10 }
  },
  on_complete = {
    set_var = { sword_basics_learned = true }
  }
}
