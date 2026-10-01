-- data/quests/mainline_xuanming.lua：主线第四段·玄冥初现（v0.2）。
--
-- 承接 mainline_seal 之后的调查线：顾长风透露师父陆归藏当年查到的玄冥会线索。
-- 玩家需要跑三张图取到三份实证物（村后巷的司徒魁面巾碎片 → 用 cloth 代表；
-- 山道贼寨的令箭 → 用 reed 编成的信物；寒泉洞暗河的玄冥寒铁 → iron_ore）。
-- 完成条件全部用既有 item 目标类型表达，读档后是纯派生态。
return {
  version = 1,
  id = "mainline_xuanming",
  name = "主线·玄冥初现",
  giver = "npc_gu_changfeng",
  summary = "顾长风说：师父失踪前查过玄冥会。带三份实证物回来给他看。",
  objectives = {
    { id = "bring_cloth", kind = "item", item = "cloth", count = 1, label = "取一份村后巷的织品", consume = false },
    { id = "bring_reed", kind = "item", item = "reed", count = 3, label = "取三束山道芦苇作信物", consume = false },
    { id = "bring_ore", kind = "item", item = "iron_ore", count = 2, label = "取两份寒泉洞玄冥寒铁", consume = false },
    { id = "bring_core", kind = "item", item = "ape_core", count = 1, label = "带一份山魈内丹", consume = false }
  },
  rewards = {
    { kind = "money", amount = 150 },
    { kind = "reputation", faction = "sect", amount = 20 }
  },
  on_complete = {
    set_var = { mainline_xuanming_done = true },
    relationship = { npc_gu_changfeng = 15 }
  }
}
