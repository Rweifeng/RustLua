-- data/schedules/a_yao.lua：阿药（云泽村·药童）日程（docs/54 §4.8）。
--
-- 白天在医馆/药圃捣药晒药，雨天回药圃檐下；傍晚广场跑腿，夜里归药圃小屋。
return {
  version = 1,
  id = "a_yao",
  blocks = {
    { from = 360,  to = 480,  place = "village_herb_yard", activity = "morning" },
    { from = 480,  to = 1080, place = "village_clinic",    activity = "work", rain_place = "village_herb_yard" },
    { from = 1080, to = 1260, place = "village_plaza",      activity = "errand", rain_place = "village_herb_yard" },
    { from = 1260, to = 360,  place = "village_herb_yard", activity = "sleep" }
  }
}
