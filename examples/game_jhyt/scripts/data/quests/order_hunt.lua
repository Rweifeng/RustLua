-- data/quests/order_hunt.lua：订单·山道猎获（M28 / 计划 G3 纯数据）。
--
-- 为什么需要这一单：掉落系统（`data/drops.lua` + `systems/loot.lua`）如果没有任何
-- 消费方，就是"能掉但没人要"的死内容；这一单把「战斗 → 掉落 → 待入库 → 交付」
-- 串成一条可验收的线：狼皮来自演武场灰狼，兽肉来自青云山道野猪。
--
-- 目标物品与掉落表一致（enemy_wolf → wolf_pelt ×1、enemy_wild_boar → boar_meat ×1..2），
-- 因此"打死一只狼 + 一头猪"就够交付，不需要反复刷。
return {
  version = 1,
  id = "order_hunt",
  name = "订单·山道猎获",
  giver = "npc_lin_xiaoman",
  summary = "林小满替村里办席，要一张狼皮与两份兽肉。",
  objectives = {
    { id = "bring_pelt", kind = "item", item = "wolf_pelt", count = 1, label = "持有狼皮", consume = true },
    { id = "bring_meat", kind = "item", item = "boar_meat", count = 2, label = "持有兽肉", consume = true }
  },
  rewards = {
    { kind = "money", amount = 90 },
    { kind = "relationship", npc = "npc_lin_xiaoman", amount = 6 }
  },
  on_complete = {
    set_var = { order_hunt_done = true }
  }
}
