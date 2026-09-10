-- data/schedules/lin_xiaoman.lua：林小满（云泽村·茶农）日程。
return {
  id = "lin_xiaoman",
  blocks = {
    { from = 360,  to = 480,  place = "village_home_north", activity = "morning" },
    { from = 480,  to = 720,  place = "village_teafield",   activity = "work",   rain_place = "village_teahouse" },
    { from = 720,  to = 1080, place = "village_teafield",   activity = "midday", rain_place = "village_home_north" },
    { from = 1080, to = 1260, place = "village_teahouse",   activity = "social" },
    { from = 1260, to = 1380, place = "village_home_north", activity = "return_home" },
    { from = 1380, to = 360,  place = "village_home_north", activity = "sleep" }
  }
}
