-- data/schedules/su_wanqing.lua：苏晚晴（山庄·青云门弟子）日程。
return {
  version = 1,
  id = "su_wanqing",
  blocks = {
    { from = 360,  to = 480,  place = "manor_home_east",    activity = "morning" },
    { from = 480,  to = 720,  place = "manor_practice",     activity = "work",   rain_place = "manor_study" },
    { from = 720,  to = 1080, place = "manor_practice",     activity = "midday", rain_place = "manor_study" },
    { from = 1080, to = 1260, place = "manor_sword_ground", activity = "social", rain_place = "manor_study" },
    { from = 1260, to = 1380, place = "manor_home_east",    activity = "return_home" },
    { from = 1380, to = 360,  place = "manor_home_east",    activity = "sleep" }
  }
}
