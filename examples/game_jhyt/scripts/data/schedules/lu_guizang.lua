-- data/schedules/lu_guizang.lua：陆归藏（师父，主线失踪者）日程。
--
-- "已失踪"的人不该有正常作息：他只在深夜出现在山庄书房与渡口，
-- 白天时段留在书房（玩家白天进书房只会看到空屋——由对话/任务判断是否现身）。
return {
  version = 1,
  id = "lu_guizang",
  blocks = {
    { from = 360,  to = 1200, place = "manor_study",  activity = "hidden" },
    { from = 1200, to = 1320, place = "manor_dock",   activity = "gaze" },
    { from = 1320, to = 360,  place = "manor_study",  activity = "hidden" }
  }
}
