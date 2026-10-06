-- data/schedules/shi_gandang.lua：石敢当（护安镖局·赵铁算盘手下镖师）日程（docs/54 §4.8）。
--
-- 上午在集市接镖护货，午后上山庄练功场与阿七对练，傍晚茶摊歇脚，夜里归镖局院。
return {
  version = 1,
  id = "shi_gandang",
  blocks = {
    { from = 360,  to = 480,  place = "village_escort_yard", activity = "morning" },
    { from = 480,  to = 900,  place = "village_market",     activity = "escort", rain_place = "village_escort_yard" },
    { from = 900,  to = 1140, place = "manor_practice",      activity = "train", rain_place = "manor_forge" },
    { from = 1140, to = 1320, place = "village_teahouse",    activity = "social" },
    { from = 1320, to = 360,  place = "village_escort_yard", activity = "sleep" }
  }
}
