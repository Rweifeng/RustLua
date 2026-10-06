-- data/schedules/guimian.lua：鬼面（玄冥会眼线）日程（docs/54 §4.8）。
--
-- 敌对立场（无阵营，与司徒魁同族）：白昼在后巷蹲梢、集市混人堆，夜里缩暗巷。
-- 雨天躲进暗巷不露面。
return {
  version = 1,
  id = "guimian",
  blocks = {
    { from = 360,  to = 600,  place = "village_ghost_alley", activity = "lurk" },
    { from = 600,  to = 1080, place = "village_alley",       activity = "spy", rain_place = "village_ghost_alley" },
    { from = 1080, to = 1260, place = "village_market",      activity = "mingle", rain_place = "village_ghost_alley" },
    { from = 1260, to = 360,  place = "village_ghost_alley", activity = "sleep" }
  }
}
