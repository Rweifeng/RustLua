-- data/schedules/han_tieshan.lua：韩铁山（云泽村·铁匠）日程。
return {
  id = "han_tieshan",
  blocks = {
    { from = 360,  to = 480,  place = "village_home_forge", activity = "morning" },
    { from = 480,  to = 720,  place = "village_smithy",     activity = "work" },
    { from = 720,  to = 1080, place = "village_smithy",     activity = "midday", rain_place = "village_home_forge" },
    { from = 1080, to = 1260, place = "village_alley",      activity = "social" },
    { from = 1260, to = 1380, place = "village_home_forge", activity = "return_home" },
    { from = 1380, to = 360,  place = "village_home_forge", activity = "sleep" }
  }
}
