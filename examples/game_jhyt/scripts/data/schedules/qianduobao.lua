-- data/schedules/qianduobao.lua：钱多宝（云泽村·商会掌柜）日程（docs/54 §4.8）。
--
-- 上午在货摊盘点账货，雨天回商会对账；傍晚茶摊谈生意，夜里归商会。
return {
  version = 1,
  id = "qianduobao",
  blocks = {
    { from = 360,  to = 480,  place = "village_merchant_hall", activity = "morning" },
    { from = 480,  to = 1080, place = "village_stall",         activity = "trade", rain_place = "village_merchant_hall" },
    { from = 1080, to = 1260, place = "village_teahouse",      activity = "social", rain_place = "village_merchant_hall" },
    { from = 1260, to = 360,  place = "village_merchant_hall", activity = "sleep" }
  }
}
