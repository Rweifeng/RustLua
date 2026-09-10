-- data/schedules/gu_changfeng.lua：顾长风（山庄·退隐镖师）日程。
return {
  id = "gu_changfeng",
  blocks = {
    { from = 360,  to = 480,  place = "manor_home",         activity = "morning" },
    { from = 480,  to = 720,  place = "manor_sword_ground", activity = "work",   rain_place = "manor_study" },
    { from = 720,  to = 1080, place = "manor_garden",       activity = "midday", rain_place = "manor_study" },
    { from = 1080, to = 1260, place = "manor_study",        activity = "social" },
    { from = 1260, to = 1380, place = "manor_home",         activity = "return_home" },
    { from = 1380, to = 360,  place = "manor_home",         activity = "sleep" }
  }
}
