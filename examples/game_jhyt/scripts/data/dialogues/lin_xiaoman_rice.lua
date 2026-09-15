-- data/dialogues/lin_xiaoman_rice.lua：林小满（云泽村·茶农）对话（M19-01 新增，M28 扩猎获订单）。
--
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
-- 本对话负责两条订单（同一 NPC 的入口必须写在一个文件里：dialogue.find_for_npc
-- 对每个 speaker 只取第一条对话）：
--   * 订单·市集稻米 order_rice（M19-01）
--   * 订单·山道猎获 order_hunt（M28，交付敌人掉落的狼皮/兽肉）
-- 全部走 quest.apply_effects（accept_quest / set_var / deliver_quest），
-- 不改 quest.lua / dialogue.lua。
--
-- 为什么不是货摊掌柜赵铁算盘：他同时挂着 shop_keeper 目标（priority 20），
-- 站到他面前按 E 只会打开商店，该 NPC 的对话不可达（docs/46 §已知限制）。
return {
  version = 1,
  id = "lin_xiaoman_rice",
  speaker = "npc_lin_xiaoman",
  start = "greet",
  nodes = {
    greet = {
      text = "林小满在茶垄间直起腰：“客官来得正好，商队托我代收两斗稻谷。”",
      options = {
        { text = "怎么收？", cond = { quest = "order_rice", state = "not_started" }, next = "intro" },
        { text = "两斗稻谷在此。", cond = { quest = "order_rice", state = "deliverable" }, effects = { deliver_quest = "order_rice" }, next = "done" },
        { text = "稻谷还在地里。", cond = { quest = "order_rice", state = "in_progress" }, next = "remind" },
        { text = "这单已经算清了。", cond = { quest = "order_rice", state = "completed" }, next = "thanks" },
        -- 订单·山道猎获（M28）：村里办席要一张狼皮与两份兽肉。
        { text = "村里还缺什么野味？", cond = { quest = "order_hunt", state = "not_started" }, next = "hunt_intro" },
        { text = "狼皮一张、兽肉两份，都在这里。", cond = { quest = "order_hunt", state = "deliverable" }, effects = { deliver_quest = "order_hunt" }, next = "hunt_done" },
        { text = "猎物还没打够。", cond = { quest = "order_hunt", state = "in_progress" }, next = "hunt_remind" },
        { text = "猎获这单也清了。", cond = { quest = "order_hunt", state = "completed" }, next = "hunt_thanks" },
        { text = "先去别处看看。", finish = true }
      }
    },
    intro = {
      text = "林小满：“两斗稻谷，四十五文，另记你一份村中人情。谷子晒干些就好。”",
      options = {
        { text = "这单我接了。", effects = { accept_quest = "order_rice", set_var = { orders_started = true } }, next = "accept" },
        { text = "再想想。", finish = true }
      }
    },
    accept = {
      text = "林小满：“爽快！谷子晒干些，别掺土。”",
      options = {
        { text = "放心。", finish = true }
      }
    },
    done = {
      text = "林小满过了过斗：“数目对上了，四十五文，收好。”",
      options = {
        { text = "成交。", finish = true }
      }
    },
    remind = {
      text = "林小满：“两斗稻谷，凑齐再来，商队催得紧。”",
      options = {
        { text = "知道了。", finish = true }
      }
    },
    thanks = {
      text = "林小满：“这单清了，下回有货再喊我。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    -- 订单·山道猎获（M28）
    hunt_intro = {
      text = "林小满：“村里要办席，缺一张狼皮、两份兽肉——演武场有灰狼，山道上还有野猪。”",
      options = {
        { text = "这单我接了。", effects = { accept_quest = "order_hunt", set_var = { orders_started = true } }, next = "hunt_accept" },
        { text = "再想想。", finish = true }
      }
    },
    hunt_accept = {
      text = "林小满：“打猎小心些。猎物打到了，先搁你身上，睡一觉再送来也不迟。”",
      options = {
        { text = "知道了。", finish = true }
      }
    },
    hunt_done = {
      text = "林小满抖开狼皮看了看：“皮子剥得干净。九十文，收好。”",
      options = {
        { text = "成交。", finish = true }
      }
    },
    hunt_remind = {
      text = "林小满：“狼皮一张、兽肉两份，一样都不能少。”",
      options = {
        { text = "就去打。", finish = true }
      }
    },
    hunt_thanks = {
      text = "林小满：“席面够了，下回请你吃酒。”",
      options = {
        { text = "告辞。", finish = true }
      }
    }
  }
}
