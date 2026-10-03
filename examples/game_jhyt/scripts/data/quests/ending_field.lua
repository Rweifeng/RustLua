-- data/quests/ending_field.lua：结局一·归田（v0.2）。
-- 玩家选择放下武名，回山庄专心把药田经营起来。交付即锁死 ending = "field"。
--
-- M34-01 的声望门槛：归田要的是"村里待见你"，按云泽村阵营的声望档位算。
-- 今天可拿到的云泽村声望是主线订单 +8、主线封印 +15（必得），另加稻米订单 +6、
-- 草药委托 +5（可选）= 34；熟识档下限 20，所以主线走通即可交付，
-- 而"重视/核心"两档今天拿不到（§8.2 的上两档要等节日与社交收益落地）。
return {
  version = 1,
  id = "ending_field",
  name = "结局·归田",
  giver = "npc_shen_qingluo",
  summary = "沈青萝说：药田也是一种江湖。囤够稻谷，回山庄把这一亩三分地种成模样。",
  objectives = {
    { id = "sealed_truth", kind = "flag", var = "mainline_truth_done", label = "理清真相" },
    { id = "field_rice", kind = "item", item = "rice", count = 20, label = "囤 20 份稻谷", consume = true },
    { id = "village_welcome", kind = "reputation", faction = "village", tier = "known", label = "云泽村声望到熟识" }
  },
  rewards = {
    { kind = "money", amount = 500 },
    { kind = "reputation", faction = "village", amount = 30 }
  },
  on_complete = {
    set_var = { ending = "field", ending_field = true },
    relationship = { npc_shen_qingluo = 30 }
  }
}
