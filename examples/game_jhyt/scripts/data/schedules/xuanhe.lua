-- data/schedules/xuanhe.lua：玄鹤（云游道士）日程 —— 跨图 NPC。
--
-- 白天在云泽村（土地庙/广场/茶摊），清晨与夜间在山庄（门廊/厢房）：
-- 08:00 与 21:00 两次跨图（main <-> village），验证"按时间在两张地图间移动"。
-- 雨天全时段改为山庄室内（研究/后园），不跨图。
return {
  id = "xuanhe",
  blocks = {
    { from = 360,  to = 480,  place = "manor_gate",       activity = "morning" },
    { from = 480,  to = 720,  place = "village_shrine",   activity = "work",   rain_place = "manor_study" },
    { from = 720,  to = 1080, place = "village_plaza",    activity = "midday", rain_place = "manor_study" },
    { from = 1080, to = 1260, place = "village_teahouse", activity = "social", rain_place = "manor_garden" },
    { from = 1260, to = 1380, place = "manor_home",       activity = "return_home" },
    { from = 1380, to = 360,  place = "manor_home",       activity = "sleep" }
  }
}
