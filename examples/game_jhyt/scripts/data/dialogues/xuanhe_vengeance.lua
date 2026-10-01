-- data/dialogues/xuanhe_vengeance.lua：玄鹤（山庄·山门云游长老）对话（v0.2 新增）。
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
--
-- 只挂结局·雪恨（ending_vengeance）：需要 mainline_truth_done 才出现，
-- 交付即锁死 ending = "vengeance"。
return {
  version = 1,
  id = "xuanhe_vengeance",
  speaker = "npc_xuanhe",
  start = "greet",
  nodes = {
    greet = {
      text = "玄鹤拂尘一摆：“司徒魁三月之内必登山门。你若了此因果，便带山魈内丹与三张狼皮来山门候他。”",
      options = {
        { text = "弟子愿往。", cond = { quest = "ending_vengeance", state = "not_started", var = "mainline_truth_done", truthy = true }, next = "ven_offer" },
        { text = "内丹与狼皮俱在。", cond = { quest = "ending_vengeance", state = "deliverable" }, effects = { deliver_quest = "ending_vengeance" }, next = "ven_done" },
        { text = "还在凑路费。", cond = { quest = "ending_vengeance", state = "in_progress" }, next = "ven_remind" },
        { text = "山门已清。", cond = { quest = "ending_vengeance", state = "completed" }, next = "ending_epilogue" },
        { text = "徒儿尚未理清真相。", cond = { var = "mainline_truth_done", falsy = true }, next = "prereq" },
        { text = "改日再听长老吩咐。", finish = true }
      }
    },
    prereq = {
      text = "玄鹤：“真相未明就下山门，是送死。先去寻你陆师叔。”",
      options = {
        { text = "弟子省得。", finish = true }
      }
    },
    ven_offer = {
      text = "玄鹤在山石上画了个“魁”字：“内丹为凭，狼皮作粮。凑齐了再来。”",
      options = {
        { text = "弟子便去准备。", effects = { accept_quest = "ending_vengeance", relationship = { npc_xuanhe = 5 } }, next = "ven_accept" },
        { text = "容徒儿再想想。", finish = true }
      }
    },
    ven_accept = {
      text = "玄鹤：“山魈在寒泉洞最深处，狼在青云山道上。此去须带够伤药。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    ven_remind = {
      text = "玄鹤：“内丹一张口、狼皮三张，少一样都不行。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    ven_done = {
      text = "玄鹤把拂尘一横：“内丹归山门，狼皮作路费。三月之后，司徒魁登门那日，为师在此等你。”",
      options = {
        { text = "弟子必到。", finish = true }
      }
    },
    ending_epilogue = {
      text = "玄鹤望着山下云海：“因果已了。江湖上还你的名号，你自己收。”",
      options = {
        { text = "谢长老。", finish = true }
      }
    }
  }
}
