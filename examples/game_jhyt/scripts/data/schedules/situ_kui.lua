-- data/schedules/situ_kui.lua：司徒魁（玄冥会执事）日程。
--
-- 反派日程刻意"避人"：白天在后巷与集市边缘收账，夜里才回后巷，
-- 让玩家在白天碰到他时觉得"这人不该在这儿"。
return {
  version = 1,
  id = "situ_kui",
  blocks = {
    { from = 360,  to = 540,  place = "village_alley",     activity = "morning" },
    { from = 540,  to = 720,  place = "village_market",    activity = "collect" },
    { from = 720,  to = 900,  place = "village_alley",     activity = "midday" },
    { from = 900,  to = 1080, place = "village_teahouse",  activity = "deal" },
    { from = 1080, to = 1320, place = "village_alley",     activity = "guard" },
    { from = 1320, to = 360,  place = "village_alley",     activity = "sleep" }
  }
}
