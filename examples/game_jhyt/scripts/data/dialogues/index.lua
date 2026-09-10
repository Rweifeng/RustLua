-- data/dialogues/index.lua：对话注册表（M17-03 纯数据）。
--
-- 新增一段对话 = 本文件加一个 id + data/dialogues/<id>.lua 一份定义，无需改动
-- dialogue.lua / quest.lua / Rust 核心（验收项①）。校验与遍历都以此表为准。
return {
  "gu_changfeng_teach",
  "shen_qingluo_herb"
}
