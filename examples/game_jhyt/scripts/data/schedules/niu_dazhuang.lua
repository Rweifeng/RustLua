-- data/schedules/niu_dazhuang.lua：牛大壮（云泽村·老黄牛倌）日程（docs/54 §4.8）。
--
-- 时段 [from,to) 分钟无缝覆盖 0..1439；to<=from 跨午夜。白天在村西茶田旁放牛，
-- 雨天回牛棚避雨；傍晚广场闲坐，夜里归牛棚。
return {
  version = 1,
  id = "niu_dazhuang",
  blocks = {
    { from = 360,  to = 480,  place = "village_cattle_shed", activity = "morning" },
    { from = 480,  to = 1080, place = "village_teafield",    activity = "graze", rain_place = "village_cattle_shed" },
    { from = 1080, to = 1260, place = "village_plaza",      activity = "social", rain_place = "village_cattle_shed" },
    { from = 1260, to = 360,  place = "village_cattle_shed", activity = "sleep" }
  }
}
