-- data/quests/mainline_arrival.lua：主线第一段·初到山庄（M19-01 纯数据）。
--
-- 主线只有一条，分三段（arrival → orders → seal），每段都是普通任务定义：
-- 目标用 item/var/flag 三类既有目标表达，交付后 on_complete 写主线变量，
-- 因此主线状态完全落在 quest.records + state.vars 里，随存档持久化且幂等。
--
-- 本段两个目标（都用 quest.lua 既有目标类型，纯数据、可存档、幂等）：
--   * var  tutorial_step >= 1 —— 「翻一块药田」：新手引导第①步正是翻土，
--     由 input.lua 既有的 tutorial.on_action(state.last_action) 接线推进
--     （farm.act 成功 → last_action = "till" → tutorial_step = 1）。
--     这样主线不需要改 input.lua / tutorial.lua，也不新增接线即可判定。
--   * flag gu_taught         —— 听顾长风讲过一遍山庄规矩（对话 set_var）
-- 交付在顾长风的对话里（deliver_quest），完成后 mainline_arrival_done = true。
return {
  version = 1,
  id = "mainline_arrival",
  name = "主线·初到山庄",
  giver = "npc_gu_changfeng",
  summary = "顾长风要你先熟悉山庄：翻一块药田，再听他讲讲门中规矩。",
  objectives = {
    { id = "till_soil", kind = "var", var = "tutorial_step", count = 1, label = "翻一块药田" },
    { id = "gu_taught", kind = "flag", var = "gu_taught", label = "听顾长风讲规矩" }
  },
  rewards = {
    { kind = "inner_power", amount = 10 },
    { kind = "reputation", faction = "sect", amount = 5 }
  },
  on_complete = {
    set_var = { mainline_arrival_done = true },
    relationship = { npc_gu_changfeng = 5 }
  }
}
