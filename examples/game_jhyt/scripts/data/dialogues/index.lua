-- data/dialogues/index.lua：对话注册表（M17-03 纯数据，M19-01 追加订单/主线入口）。
--
-- 新增一段对话 = 本文件加一个 id + data/dialogues/<id>.lua 一份定义，无需改动
-- dialogue.lua / quest.lua / Rust 核心（验收项①）。校验与遍历都以此表为准。
--
-- 注意：dialogue.find_for_npc 对每个 speaker 只取**第一条**匹配的对话，
-- 因此同一个 NPC 的多个任务入口必须写在同一份对话文件的 nodes 里
-- （M19-01 的订单/主线入口分别扩展了沈青萝/顾长风的既有对话，
-- 林小满的稻米订单是本文件新增的一条对话）。
--
-- 为什么稻米订单不给货摊掌柜赵铁算盘：他同时挂着 shop_keeper 交互目标
-- （priority 20 > 对话 10），站到他面前按 E 只会打开商店，该 NPC 的对话
-- 在当前设计下不可达（docs/46 §已知限制）。
return {
  version = 1,
  ids = {
    "gu_changfeng_teach",
    "shen_qingluo_herb",
    "lin_xiaoman_rice"
  }
}
