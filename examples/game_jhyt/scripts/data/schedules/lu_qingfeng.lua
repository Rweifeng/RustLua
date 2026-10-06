-- data/schedules/lu_qingfeng.lua：陆青峰（青云门弟子）日程（docs/54 §4.8）。
--
-- 白天在剑坪/演武堂练剑，雨天回厢房；午后练功场与同门切磋，夜里归演武堂。
return {
  version = 1,
  id = "lu_qingfeng",
  blocks = {
    { from = 360,  to = 480,  place = "manor_sword_hall",   activity = "morning" },
    { from = 480,  to = 1080, place = "manor_sword_ground", activity = "train", rain_place = "manor_home" },
    { from = 1080, to = 1260, place = "manor_practice",    activity = "spar", rain_place = "manor_home" },
    { from = 1260, to = 360,  place = "manor_sword_hall",  activity = "sleep" }
  }
}
