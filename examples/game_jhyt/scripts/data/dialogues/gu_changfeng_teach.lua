-- data/dialogues/gu_changfeng_teach.lua：顾长风（山庄·退隐镖师）对话（M17-03，M19-01 扩展）。
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
--         relationship           关系增量 { npc_id = delta }（M18-04）
--         reputation             声望增量 { faction = delta }（M18-04）
--       next      跳转节点 id（与 finish 二选一）
--       finish    true 结束对话
--
-- M19-01 扩展：同一文件里再挂三个入口（find_for_npc 每个 speaker 只取第一条对话）：
--   ① 木桩三式 quest_spar（既有）
--   ② 主线一段 mainline_arrival（接纳 + 记录顾长风讲规矩）
--   ③ 订单·山庄丸药 order_pill
-- v0.2 扩展：同一文件再挂两个入口
--   ④ 主线四段 mainline_xuanming（需要 mainline_seal 已完成才出现）
--   ⑤ 结局·断念 ending_renounce（需要 mainline_truth_done 才出现）
return {
  version = 1,
  id = "gu_changfeng_teach",
  speaker = "npc_gu_changfeng",
  start = "greet",
  nodes = {
    greet = {
      text = "顾长风负手立于木桩旁：“江湖路远，先把刀法根基打稳。”",
      options = {
        -- ① 木桩三式（既有）
        { text = "愿随师父走一遍基础三式。", cond = { quest = "quest_spar", state = "not_started" }, effects = { accept_quest = "quest_spar", relationship = { npc_gu_changfeng = 3 } }, next = "accept" },
        { text = "再走一式。", cond = { quest = "quest_spar", state = "in_progress" }, effects = { add_var = { sword_drills = 1 } }, next = "practice" },
        { text = "三式已经走完，请师父指点。", cond = { quest = "quest_spar", state = "deliverable" }, effects = { deliver_quest = "quest_spar" }, next = "done" },
        { text = "三式已成，多谢师父。", cond = { quest = "quest_spar", state = "completed" }, next = "thanks" },
        -- ② 主线一段
        { text = "弟子新来乍到，请师父吩咐。", cond = { quest = "mainline_arrival", state = "not_started" }, next = "arrival_offer" },
        { text = "药田翻好了，规矩也已记下。", cond = { quest = "mainline_arrival", state = "deliverable" }, effects = { deliver_quest = "mainline_arrival" }, next = "arrival_done" },
        { text = "药田还没翻完。", cond = { quest = "mainline_arrival", state = "in_progress" }, next = "arrival_remind" },
        -- ③ 订单·山庄丸药
        { text = "庄中可有要办的差事？", cond = { quest = "order_pill", state = "not_started" }, next = "pill_intro" },
        { text = "回复药炼好了，请过目。", cond = { quest = "order_pill", state = "deliverable" }, effects = { deliver_quest = "order_pill" }, next = "pill_done" },
        { text = "丸药还在炉上。", cond = { quest = "order_pill", state = "in_progress" }, next = "pill_remind" },
        { text = "丸药已经交讫。", cond = { quest = "order_pill", state = "completed" }, next = "pill_thanks" },
        -- ④ 主线·玄冥初现（mainline_seal 落印后才出现）
        { text = "师父可还记得旧事？", cond = { quest = "mainline_xuanming", state = "not_started", var = "mainline_done", truthy = true }, next = "xuanming_offer" },
        { text = "实证已备齐，请师父过目。", cond = { quest = "mainline_xuanming", state = "deliverable" }, effects = { deliver_quest = "mainline_xuanming" }, next = "xuanming_done" },
        { text = "三份物证还在各处。", cond = { quest = "mainline_xuanming", state = "in_progress" }, next = "xuanming_remind" },
        { text = "玄冥会线索已交讫。", cond = { quest = "mainline_xuanming", state = "completed" }, next = "xuanming_thanks" },
        -- ⑤ 结局·断念（mainline_truth_done 之后才出现）
        { text = "弟子想放下这一切。", cond = { quest = "ending_renounce", state = "not_started", var = "mainline_truth_done", truthy = true }, next = "renounce_offer" },
        { text = "五份行气散留在剑坪了。", cond = { quest = "ending_renounce", state = "deliverable" }, effects = { deliver_quest = "ending_renounce" }, next = "renounce_done" },
        { text = "行气散还没配足。", cond = { quest = "ending_renounce", state = "in_progress" }, next = "renounce_remind" },
        { text = "弟子已断此念。", cond = { quest = "ending_renounce", state = "completed" }, next = "ending_epilogue" },
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
    },
    -- ② 主线一段节点
    arrival_offer = {
      text = "顾长风抬眼看了看药田：“既入山庄，先识农桑，再谈刀剑。翻一块地，回来听我讲规矩。”",
      options = {
        -- 两条选项都接取任务并给同一个副作用：dialogue.choose 用相对索引对齐
        -- 下一节点，若只给第 1 条加副作用，玩家按 W/S 选第 2 条就会漏掉 set_var。
        -- 因此副作用写在 arrival_accept 的选择上（唯一入口），此处只负责接取。
        { text = "弟子遵命。", effects = { accept_quest = "mainline_arrival", relationship = { npc_gu_changfeng = 3 } }, next = "arrival_accept" },
        { text = "容后再议。", finish = true }
      }
    },
    arrival_accept = {
      -- set_var 放在这里而不是 arrival_offer：dialogue.choose 用相对索引，
      -- 若写在上一个节点，按 W/S 选第 2 条就绕过了副作用（实测踩过）。
      text = "顾长风：“药田就在南边。锄头在你背上，去罢。”",
      options = {
        { text = "是。", effects = { set_var = { gu_taught = true } }, finish = true }
      }
    },
    arrival_remind = {
      text = "顾长风：“地翻好了，规矩我自然讲给你听。”",
      options = {
        { text = "弟子这就去。", finish = true }
      }
    },
    arrival_done = {
      text = "顾长风：“翻土知节气，练刀知进退。山庄的门，算你进来了。”",
      options = {
        { text = "谢师父指点。", finish = true }
      }
    },
    -- ③ 订单·山庄丸药节点
    pill_intro = {
      text = "顾长风：“庄中弟子要下山办事，缺一丸回复药。你会种药，也会用炉？”",
      options = {
        { text = "愿为山庄炼一丸。", effects = { accept_quest = "order_pill", set_var = { orders_started = true }, relationship = { npc_gu_changfeng = 2 } }, next = "pill_accept" },
        { text = "再想想。", finish = true }
      }
    },
    pill_accept = {
      text = "顾长风：“药庐一放，灵芝两株，文火两个时辰。这是八十文的买价。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    pill_done = {
      text = "顾长风接过丸药闻了闻：“药香不散，火候到了。这八十文，还有一包行气散，一并拿去。”",
      options = {
        { text = "多谢师父。", finish = true }
      }
    },
    pill_remind = {
      text = "顾长风：“丸药未成，急不得。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    pill_thanks = {
      text = "顾长风：“药已入库，你去忙罢。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- ④ 主线·玄冥初现节点
    xuanming_offer = {
      text = "顾长风沉吟：“师父当年留书提及一个名字——玄冥会。村后巷有织品碎片、山道贼寨有芦苇信物、寒泉洞有玄冥寒铁，再加一颗山魈内丹为凭。你可愿替他查下去？”",
      options = {
        { text = "弟子领命。", effects = { accept_quest = "mainline_xuanming", relationship = { npc_gu_changfeng = 5 } }, next = "xuanming_accept" },
        { text = "容徒儿想想。", finish = true }
      }
    },
    xuanming_accept = {
      text = "顾长风：“四件物事俱在，再来寻我。切记：织品与芦苇不必交出，只取一份即可。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    xuanming_remind = {
      text = "顾长风：“线索未齐，不可妄动。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    xuanming_done = {
      text = "顾长风把四件物事排成一列：“织品是司徒魁的，芦苇是贼寨传令的，寒铁与内丹是寒泉洞的。此事已明，去寻陆师叔罢。”",
      options = {
        { text = "弟子便去。", finish = true }
      }
    },
    xuanming_thanks = {
      text = "顾长风：“玄冥会的线头已交给你陆师叔，去书房寻他。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- ⑤ 结局·断念节点
    renounce_offer = {
      text = "顾长风望着剑坪：“放下比拿起更难。你若真要抽身，就把五份行气散留在此地，权当给乡邻的最后一段缘。”",
      options = {
        { text = "弟子遵命。", effects = { accept_quest = "ending_renounce", relationship = { npc_gu_changfeng = 5 } }, next = "renounce_accept" },
        { text = "容徒儿三思。", finish = true }
      }
    },
    renounce_accept = {
      text = "顾长风：“药在炉上，路在脚下。凑齐五份便来剑坪。”",
      options = {
        { text = "是。", finish = true }
      }
    },
    renounce_remind = {
      text = "顾长风：“五份行气散，一份也不能少。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    renounce_done = {
      text = "顾长风接过五只药囊，久久不语，终于把剑穗解下：“自今往后，江湖恩怨与你无涉。”",
      options = {
        { text = "谢师父成全。", finish = true }
      }
    },
    ending_epilogue = {
      text = "顾长风：“剑坪风大，回屋罢。”",
      options = {
        { text = "弟子告退。", finish = true }
      }
    }
  }
}
