-- data/schedules/zhao_tiesuanpan.lua：赵铁算盘（云泽村·集市商人）日程。
return {
  id = "zhao_tiesuanpan",
  blocks = {
    { from = 360,  to = 480,  place = "village_home_south", activity = "morning" },
    { from = 480,  to = 720,  place = "village_stall",      activity = "work",  rain_place = "village_home_south" },
    { from = 720,  to = 1080, place = "village_stall",      activity = "midday", rain_place = "village_home_south" },
    { from = 1080, to = 1260, place = "village_teahouse",   activity = "social" },
    { from = 1260, to = 1380, place = "village_home_south", activity = "return_home" },
    { from = 1380, to = 360,  place = "village_home_south", activity = "sleep" }
  }
}
