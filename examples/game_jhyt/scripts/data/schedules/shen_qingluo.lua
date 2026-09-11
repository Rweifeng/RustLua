-- data/schedules/shen_qingluo.lua：沈青萝（云泽村·医馆掌柜）日程。
--
-- 时段语义：[from, to) 分钟（0..1439）；to <= from 表示跨午夜（23:00 → 06:00）。
-- place / rain_place 引用 data/places.lua；rain_place 为雨天替代（缺省沿用 place）。
-- 时段须无缝覆盖 0..1439 且互不重叠，否则 schedule.validate() 报诊断。
return {
  version = 1,
  id = "shen_qingluo",
  blocks = {
    { from = 360,  to = 480,  place = "village_home_west", activity = "morning" },
    { from = 480,  to = 720,  place = "village_clinic",    activity = "work" },
    { from = 720,  to = 1080, place = "village_clinic",    activity = "midday" },
    { from = 1080, to = 1260, place = "village_plaza",     activity = "social" },
    { from = 1260, to = 1380, place = "village_home_west", activity = "return_home" },
    { from = 1380, to = 360,  place = "village_home_west", activity = "sleep" }
  }
}
