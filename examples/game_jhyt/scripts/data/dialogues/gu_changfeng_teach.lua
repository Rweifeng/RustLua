-- data/dialogues/gu_changfeng_teach.lua：顾长风·木桩三式（M17-03 纯数据）。
--
-- 对话 schema（dialogue.lua 依据本结构与 quest.lua 协作）：
--   id        对话标识
--   speaker   说话人 persist_id（引用 data/npcs.lua）
--   start     起始节点 id
--   nodes     节点表：node_id = { text = 文本, options = { 选项... } }
--     选项字段：
--       text      选项文本
--       cond      显示/可选条件（全部满足才出现；缺省即无条件）
--         quest + state          任务状态 equals（not_started/in_progress/deliverable/completed/failed）
--         var + eq/gte/lte/lt/gt 剧情变量数值比较
--         var + truthy/falsy     剧情变量真值判断
--         item + count           背包数量 >= count
--       effects   选中后副作用（缺省即无）
--         accept_quest / deliver_quest / fail_quest   任务状态迁移
--         set_var / add_var      剧情变量写入
--         add_item               背包发放 { {item, count}, ... }
--         money                  金钱增减
--       next      跳转节点 id（与 finish 二选一）
--       finish    true 结束对话
return {
  id = "gu_changfeng_teach",
  speaker = "npc_gu_changfeng",
  start = "greet",
  nodes = {
    greet = {
      text = "顾长风负手立于木桩旁：“江湖路远，先把刀法根基打稳。”",
      options = {
        { text = "愿随师父走一遍基础三式。", cond = { quest = "quest_spar", state = "not_started" }, effects = { accept_quest = "quest_spar" }, next = "accept" },
        { text = "再走一式。", cond = { quest = "quest_spar", state = "in_progress" }, effects = { add_var = { sword_drills = 1 } }, next = "practice" },
        { text = "三式已经走完，请师父指点。", cond = { quest = "quest_spar", state = "deliverable" }, effects = { deliver_quest = "quest_spar" }, next = "done" },
        { text = "三式已成，多谢师父。", cond = { quest = "quest_spar", state = "completed" }, next = "thanks" },
        { text = "改日再来请教。", finish = true }
      }
    },
    accept = {
      text = "顾长风：“起手、横斩、回风，合为三式。你随我一式一式地走。”",
      options = {
        { text = "起手。", effects = { add_var = { sword_drills = 1 } }, next = "practice" },
        { text = "先歇一歇。", finish = true }
      }
    },
    practice = {
      text = "顾长风：“腕要稳，步要活。再来。”",
      options = {
        { text = "再走一式。", cond = { quest = "quest_spar", state = "in_progress" }, effects = { add_var = { sword_drills = 1 } }, next = "practice" },
        { text = "三式已经走完。", cond = { quest = "quest_spar", state = "deliverable" }, effects = { deliver_quest = "quest_spar" }, next = "done" },
        { text = "今日先到这里。", finish = true }
      }
    },
    done = {
      text = "顾长风颔首：“根基已立，日后自可寸进。这口气，权当为师给你的见面礼。”",
      options = {
        { text = "谢师父。", finish = true }
      }
    },
    thanks = {
      text = "顾长风：“刀在手上，路在脚下。去罢。”",
      options = {
        { text = "告辞。", finish = true }
      }
    }
  }
}
