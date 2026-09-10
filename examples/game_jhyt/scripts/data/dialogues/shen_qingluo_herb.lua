-- data/dialogues/shen_qingluo_herb.lua：沈青萝·为医馆采药（M17-03 纯数据）。
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
return {
  id = "shen_qingluo_herb",
  speaker = "npc_shen_qingluo",
  start = "greet",
  nodes = {
    greet = {
      text = "沈青萝放下药碾：“医馆正缺灵芝入药，客官可愿搭把手？”",
      options = {
        { text = "愿为医馆采三株灵芝。", cond = { quest = "quest_herb", state = "not_started" }, effects = { accept_quest = "quest_herb" }, next = "accept" },
        { text = "灵芝还在采，稍待。", cond = { quest = "quest_herb", state = "in_progress" }, next = "remind" },
        { text = "三株灵芝已备齐。", cond = { quest = "quest_herb", state = "deliverable" }, effects = { deliver_quest = "quest_herb" }, next = "done" },
        { text = "再来看看药馆。", cond = { quest = "quest_herb", state = "completed" }, next = "thanks" },
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
    }
  }
}
