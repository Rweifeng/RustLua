-- data/dialogues/shen_qingluo_herb.lua：沈青萝（云泽村·医馆掌柜）对话（M17-03，M19-01 扩展）。
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
--
-- M18-04：接取选项带关系副作用（relationship = { npc_id = 增量 }）。
-- M19-01：在同一份对话里合并三类入口（dialogue.find_for_npc 对每个 speaker
--         只取第一条对话，所以同一 NPC 的任务入口必须写在一个文件里）：
--   ① 医馆采药 quest_herb（既有）
--   ② 订单·药庐灵芝 order_herb
--   ③ 主线二段 mainline_orders（三笔订单任结一笔后出现）与主线三段 mainline_seal
return {
  version = 1,
  id = "shen_qingluo_herb",
  speaker = "npc_shen_qingluo",
  start = "greet",
  nodes = {
    greet = {
      text = "沈青萝放下药碾：“医馆正缺灵芝入药，客官可愿搭把手？”",
      options = {
        -- ① 医馆采药
        { text = "愿为医馆采三株灵芝。", cond = { quest = "quest_herb", state = "not_started" }, effects = { accept_quest = "quest_herb", relationship = { npc_shen_qingluo = 5 } }, next = "accept" },
        { text = "灵芝还在采，稍待。", cond = { quest = "quest_herb", state = "in_progress" }, next = "remind" },
        { text = "三株灵芝已备齐。", cond = { quest = "quest_herb", state = "deliverable" }, effects = { deliver_quest = "quest_herb" }, next = "done" },
        { text = "再来看看药馆。", cond = { quest = "quest_herb", state = "completed" }, next = "thanks" },
        -- ② 订单·药庐灵芝
        { text = "听说药庐常年收灵芝？", cond = { quest = "order_herb", state = "not_started" }, next = "order_intro" },
        { text = "灵芝备好了，共两株。", cond = { quest = "order_herb", state = "deliverable" }, effects = { deliver_quest = "order_herb" }, next = "order_done" },
        { text = "两株灵芝还在路上。", cond = { quest = "order_herb", state = "in_progress" }, next = "order_remind" },
        { text = "这单买卖已结清了。", cond = { quest = "order_herb", state = "completed" }, next = "order_thanks" },
        -- ③ 主线二段：三笔订单任结一笔后出现（顺序与筛选见下方 mainline 选项）
        { text = "村里还有别的活计吗？", cond = { quest = "mainline_orders", state = "not_started", var = "order_herb_done", truthy = true }, next = "orders_intro" },
        { text = "村里还有别的活计吗？", cond = { quest = "mainline_orders", state = "not_started", var = "order_rice_done", truthy = true }, next = "orders_intro" },
        { text = "村里还有别的活计吗？", cond = { quest = "mainline_orders", state = "not_started", var = "order_pill_done", truthy = true }, next = "orders_intro" },
        { text = "这把药材，请沈掌柜验看。", cond = { quest = "mainline_orders", state = "deliverable" }, effects = { deliver_quest = "mainline_orders" }, next = "orders_done" },
        { text = "药材还在采，稍待。", cond = { quest = "mainline_orders", state = "in_progress" }, next = "orders_remind" },
        { text = "药材验过了。", cond = { quest = "mainline_orders", state = "completed" }, next = "orders_thanks" },
        -- ③ 主线三段：三笔订单全清后出现
        { text = "三笔买卖都清了。", cond = { quest = "mainline_seal", state = "not_started", var = "mainline_orders_done", truthy = true }, next = "seal_offer" },
        { text = "请验我炼的行气散。", cond = { quest = "mainline_seal", state = "deliverable" }, effects = { deliver_quest = "mainline_seal" }, next = "seal_final" },
        { text = "药还没炼成。", cond = { quest = "mainline_seal", state = "in_progress" }, next = "seal_remind" },
        { text = "药香已落印。", cond = { quest = "mainline_seal", state = "completed" }, next = "seal_thanks" },
        { text = "改日再说。", finish = true }
      }
    },
    accept = {
      text = "沈青萝：“有劳。灵芝多长在田间，采满三株便来寻我。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    remind = {
      text = "沈青萝：“不急，采满三株灵芝再来便是。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    done = {
      text = "沈青萝接过灵芝，笑意温和：“多谢客官。些许谢礼，务必收下。”",
      options = {
        { text = "客气了。", finish = true }
      }
    },
    thanks = {
      text = "沈青萝：“灵芝已入药，馆中病家都念着你的好。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- ② 订单节点
    order_intro = {
      text = "沈青萝：“正是。两株灵芝一味药，六十文，现结。”",
      options = {
        { text = "这单我接了。", effects = { accept_quest = "order_herb", set_var = { orders_started = true }, relationship = { npc_shen_qingluo = 2 } }, next = "order_accept" },
        { text = "再想想。", finish = true }
      }
    },
    order_accept = {
      text = "沈青萝：“爽快。两株灵芝，随时送来。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    order_done = {
      text = "沈青萝点数入库：“两株灵芝，成色极好。六十文在此。”",
      options = {
        { text = "收下了。", finish = true }
      }
    },
    order_remind = {
      text = "沈青萝：“两株灵芝，凑齐再来。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    order_thanks = {
      text = "沈青萝：“眼下药材不缺，改日再谈买卖。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- ③ 主线二段：三笔订单任结一笔后出现
    orders_intro = {
      text = "沈青萝：“你既在村里走动了，我这儿还有一桩事：采来的药材，先给我验看一把。”",
      options = {
        { text = "应该的。", effects = { accept_quest = "mainline_orders", relationship = { npc_shen_qingluo = 2 } }, next = "orders_accept" },
        { text = "稍后再说。", finish = true }
      }
    },
    orders_accept = {
      text = "沈青萝：“一把药材即可。路上行走，处处是学问。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    orders_done = {
      text = "沈青萝把药材在日光下照了照：“火候到了。这一把，算你的入门礼。”",
      options = {
        { text = "多谢。", finish = true }
      }
    },
    orders_remind = {
      text = "沈青萝：“药篓还空着？采一把来便是。”",
      options = {
        { text = "这就去。", finish = true }
      }
    },
    orders_thanks = {
      text = "沈青萝：“药材验过了，你去忙别的罢。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- ③ 主线三段：三笔订单全清且手上有一份自家炼的行气散
    seal_offer = {
      text = "沈青萝：“三笔买卖都清了？再亮一亮你自家炼的行气散。”",
      options = {
        { text = "请验看。", effects = { accept_quest = "mainline_seal", relationship = { npc_shen_qingluo = 2 } }, next = "seal_accept" },
        { text = "还差些火候。", finish = true }
      }
    },
    seal_accept = {
      text = "沈青萝：“药香一落，这方子就算立住了。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    seal_final = {
      text = "沈青萝一丸落印：“药香既成，云泽村与山庄都记你一份人情。”",
      options = {
        { text = "承让。", finish = true }
      }
    },
    seal_remind = {
      text = "沈青萝：“药未成，莫急着落印。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    seal_thanks = {
      text = "沈青萝：“药香已落印，往后这方子有你一份。”",
      options = {
        { text = "告辞。", finish = true }
      }
    }
  }
}
