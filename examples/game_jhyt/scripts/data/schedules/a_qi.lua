-- data/schedules/a_qi.lua：阿七（护安镖局·年轻镖师）日程。
--
-- 时段语义：[from, to) 分钟；to <= from 表示跨午夜。时段须无缝覆盖 0..1439。
-- 白天在村口/集市接镖，午后去山庄练功场，傍晚回村。
return {
  version = 1,
  id = "a_qi",
  blocks = {
    { from = 360,  to = 480,  place = "village_gate",      activity = "morning" },
    { from = 480,  to = 720,  place = "village_market",    activity = "work" },
    { from = 720,  to = 900,  place = "village_plaza",     activity = "midday" },
    { from = 900,  to = 1140, place = "manor_practice",    activity = "train" },
    { from = 1140, to = 1320, place = "village_teahouse",  activity = "social" },
    { from = 1320, to = 360,  place = "village_home_south", activity = "sleep" }
  }
}
