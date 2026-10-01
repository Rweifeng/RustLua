-- data/dialogues/lu_guizang_truth.lua：陆归藏（山庄·书房长老）对话（v0.2 新增）。
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
--
-- 只挂主线五段 mainline_truth（真相浮现），入口条件是 mainline_xuanming_done
-- （顾长风交付主线四段后由 quest.on_complete 写入的锚点变量）。
-- 交付后 on_complete 写入 mainline_truth_done，四个结局入口即解锁。
return {
  version = 1,
  id = "lu_guizang_truth",
  speaker = "npc_lu_guizang",
  start = "greet",
  nodes = {
    greet = {
      text = "陆归藏放下留书：“你师叔把线索交给你了？好。把这八两铁锭锻成山庄的门闸，司徒魁便进不来。”",
      options = {
        -- 主线五段·真相浮现：需先交付 mainline_xuanming
        { text = "徒儿领师叔之命。", cond = { quest = "mainline_truth", state = "not_started", var = "mainline_xuanming_done", truthy = true }, next = "truth_offer" },
        { text = "八两铁锭已备齐。", cond = { quest = "mainline_truth", state = "deliverable" }, effects = { deliver_quest = "mainline_truth" }, next = "truth_done" },
        { text = "铁锭还在锻。", cond = { quest = "mainline_truth", state = "in_progress" }, next = "truth_remind" },
        { text = "真相已明。", cond = { quest = "mainline_truth", state = "completed" }, next = "thanks" },
        -- 未接前置时的兜底：把玩家指引回顾长风处。
        { text = "师叔可要提点一二？", cond = { var = "mainline_xuanming_done", falsy = true }, next = "prereq" },
        { text = "改日再来请教。", finish = true }
      }
    },
    prereq = {
      text = "陆归藏：“玄冥会的线索还在你顾师叔手上。先去寻他。”",
      options = {
        { text = "徒儿便去。", finish = true }
      }
    },
    truth_offer = {
      text = "陆归藏：“铁锭八两，配山门横闸。此物一交，山庄便算有了防人的底气。”",
      options = {
        { text = "徒儿去备。", effects = { accept_quest = "mainline_truth", relationship = { npc_lu_guizang = 5 } }, next = "truth_accept" },
        { text = "容徒儿想想。", finish = true }
      }
    },
    truth_accept = {
      text = "陆归藏：“韩铁山的炉子能锻。八两铁锭，凑齐再来。”",
      options = {
        { text = "记下了。", finish = true }
      }
    },
    truth_remind = {
      text = "陆归藏：“八两铁锭，一两也不能少。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    truth_done = {
      text = "陆归藏把留书合上：“窃书者是司徒魁，非我青云门人。接下来的路你自己选——归田、承业、断念、雪恨，四条都在你手上。”",
      options = {
        { text = "徒儿省得。", finish = true }
      }
    },
    thanks = {
      text = "陆归藏：“四扇门都已打开，去寻各自主事罢。”",
      options = {
        { text = "告辞。", finish = true }
      }
    }
  }
}
