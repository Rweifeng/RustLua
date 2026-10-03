-- data/dialogues/su_wanqing_sect.lua：苏晚晴（山庄·东厢执事）对话（v0.2 新增）。
-- 结构说明见 data/dialogues/gu_changfeng_teach.lua 头注释。
--
-- 只挂结局·承业（ending_sect）：需要 mainline_truth_done 才出现，
-- 交付即锁死 ending = "sect"。
--
-- M34-02 追加：② 中秋灯会——只在中秋三天出现的对话，给好感与莲子，不给声望，
-- 也不碰 ending 变量（结局指针仍只由 ending_sect 写）。
return {
  version = 1,
  id = "su_wanqing_sect",
  speaker = "npc_su_wanqing",
  start = "greet",
  nodes = {
    greet = {
      text = "苏晚晴捧出掌门信物：“山庄不可一日无主。你若肯接手，便备三颗回气丹以壮行色。”",
      options = {
        { text = "晚晴，我愿意承此衣钵。", cond = { quest = "ending_sect", state = "not_started", var = "mainline_truth_done", truthy = true }, next = "sect_offer" },
        { text = "三颗回气丹已备好。", cond = { quest = "ending_sect", state = "deliverable" }, effects = { deliver_quest = "ending_sect" }, next = "sect_done" },
        { text = "丹药还在炉上。", cond = { quest = "ending_sect", state = "in_progress" }, next = "sect_remind" },
        { text = "从今往后，我是青云掌门。", cond = { quest = "ending_sect", state = "completed" }, next = "ending_epilogue" },
        -- 前置未满足：把玩家指向陆归藏。
        { text = "山庄大事，还需从长计议。", cond = { var = "mainline_truth_done", falsy = true }, next = "prereq" },
        -- ② 中秋灯会（M34-02）：只在中秋三天出现；办过一次就写锚点收回这句话。
        { text = "灯会可要人手？弟子来搭把手。", cond = { festival = "midautumn", var = "midautumn_lantern", falsy = true }, next = "lantern_offer" },
        { text = "改日再谈。", finish = true }
      }
    },
    prereq = {
      text = "苏晚晴摇头：“玄冥会之事未明，掌门之位不可轻授。先去寻陆师叔。”",
      options = {
        { text = "告辞。", finish = true }
      }
    },
    sect_offer = {
      text = "苏晚晴把信物放在案上：“三颗回气丹为你压袖。齐了便来东厢。”",
      options = {
        { text = "便有劳你。", effects = { accept_quest = "ending_sect", relationship = { npc_su_wanqing = 5 } }, next = "sect_accept" },
        { text = "容我三思。", finish = true }
      }
    },
    sect_accept = {
      text = "苏晚晴：“气丹须是自家炼的，外人买的作不得数。”",
      options = {
        { text = "省得。", finish = true }
      }
    },
    sect_remind = {
      text = "苏晚晴：“三颗回气丹，一颗也不能少。”",
      options = {
        { text = "好。", finish = true }
      }
    },
    sect_done = {
      text = "苏晚晴将掌门信物递上，玄鹤在山门击了三下磬：“自今日起，青云山庄属你。”",
      options = {
        { text = "谢过晚晴。", finish = true }
      }
    },
    ending_epilogue = {
      text = "苏晚晴：“掌门日理万机，晚晴在书房候你。”",
      options = {
        { text = "辛苦你了。", finish = true }
      }
    },
    -- ② 中秋灯会节点（M34-02）
    lantern_offer = {
      text = "苏晚晴把一盏未糊的灯笼递过来：“今夜灯会，山庄各处的灯都要人点。你若得闲，便一起走一遭。”",
      options = {
        { text = "今晚我来点灯。", effects = { set_var = { midautumn_lantern = true }, relationship = { npc_su_wanqing = 8 }, add_item = { { item = "lotus_seed", count = 3 } } }, next = "lantern_done" },
        { text = "今夜另有事。", finish = true }
      }
    },
    lantern_done = {
      text = "苏晚晴把三颗莲子塞进你手心：“灯亮了，山庄看着才像个家。这点莲子你收着，泡水安神。”",
      options = {
        { text = "多谢晚晴。", finish = true }
      }
    }
  }
}
