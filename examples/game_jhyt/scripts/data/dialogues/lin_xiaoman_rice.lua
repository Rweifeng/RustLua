-- data/dialogues/lin_xiaoman_rice.lua：林小满（云泽村·茶农）对话（M19-01 新增）。
--
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
-- 本对话只负责订单·市集稻米 order_rice：接取 / 催问 / 交付 / 已结清四态，
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
    }
  }
}
