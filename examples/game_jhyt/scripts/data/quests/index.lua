-- data/quests/index.lua：任务注册表（M17-03 纯数据）。
--
-- 新增一条任务 = 本文件加一个 id + data/quests/<id>.lua 一份定义，无需改动
-- quest.lua / dialogue.lua / Rust 核心（验收项①）。校验与遍历都以此表为准。
return {
  "quest_herb",
  "quest_spar"
}
