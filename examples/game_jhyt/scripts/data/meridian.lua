-- data/meridian.lua：经脉修炼注册表（docs/33 §7.3，2026-10-06 1.9；
-- 改造计划 docs/54 §7.1「经脉修炼」）。
--
-- 武侠经脉成长线：消耗内力 + 冲穴材料冲穴，冲通后获得**永久**被动加成。
-- 与熟练度（§7.1，手艺）并行但叙事不同——熟练度=手艺，经脉=身体修为。
--
-- 12 条经脉（任督二脉 + 手足六经简化链）：
--   任脉 ren（根，无前置）→ 督脉 du / 手太阴肺经 feishouyin → 诸经依次解锁。
--
-- 每条字段：
--   id / name          经脉 id 与中文名；
--   difficulty         难度档 1..12（递增）：决定内力消耗、武学熟练度门槛与成功率；
--   unlock_condition   前置经脉 id（须已冲通）；根经脉为 nil；
--   passive            永久被动加成，键闭集 = attack/defense/energy_max/inner_power_max：
--                        * attack/defense      经 player.stats 生效（add_buff 按日重挂，
--                                              数值=已通经脉聚合值，战斗走 stats.query）；
--                        * energy_max         体力上限 +N（meridian.max_energy() =
--                                              inventory.max_energy() + N；跨天恢复池读它）；
--                        * inner_power_max    内力上限 +N（同上，meridian.max_inner_power()）；
--   mat                冲穴材料 { item_id = 份数 }（复用阶段 1/2 既有物品，不新增物品）。
--
-- 派生口径（systems/meridian.lua 单一事实来源，勿在此重复实现）：
--   inner_cost   = 10 + (difficulty-1)*5        —— 每次冲穴消耗内力（成败皆耗）；
--   success_rate = 0.90 - (difficulty-1)*0.05   —— 冲穴成功率（难度递减，下限 0.35）；
--   req_combat   = difficulty-1                 —— 武学熟练度等级门槛（根经脉 0 级即可）。
return {
  version = 1,
  meridians = {
    ren = {
      id = "ren",
      name = "任脉",
      description = "阴脉之海，起于胞中，上行腹胸。冲通则丹田纳气，体力上限提升。",
      difficulty = 1,
      unlock_condition = nil,
      passive = { energy_max = 10 },
      mat = { lingzhi = 1 }
    },
    du = {
      id = "du",
      name = "督脉",
      description = "阳脉之海，行于腰背正中。冲通则一身阳气贯注，拳脚之力提升。",
      difficulty = 2,
      unlock_condition = "ren",
      passive = { attack = 1 },
      mat = { ginseng = 1 }
    },
    feishouyin = {
      id = "feishouyin",
      name = "手太阴肺经",
      description = "肺主气、司卫，外合皮毛。冲通则卫气固表，防御提升。",
      difficulty = 3,
      unlock_condition = "ren",
      passive = { defense = 1 },
      mat = { lingzhi = 1 }
    },
    xinshaoyin = {
      id = "xinshaoyin",
      name = "手少阴心经",
      description = "心主血脉，为君主之官。冲通则气血充盈，内力上限提升。",
      difficulty = 4,
      unlock_condition = "du",
      passive = { inner_power_max = 10 },
      mat = { gouqi = 1 }
    },
    xinbao = {
      id = "xinbao",
      name = "手厥阴心包经",
      description = "心包代心行事，护君御邪。冲通则心壁如垣，防御提升。",
      difficulty = 5,
      unlock_condition = "xinshaoyin",
      passive = { defense = 1 },
      mat = { gouqi = 1 }
    },
    dachang = {
      id = "dachang",
      name = "手阳明大肠经",
      description = "传导之官，变化出焉。冲通则筋骨传导发力，攻击提升。",
      difficulty = 6,
      unlock_condition = "feishouyin",
      passive = { attack = 1 },
      mat = { herb = 2 }
    },
    xiaochang = {
      id = "xiaochang",
      name = "手太阳小肠经",
      description = "受盛之官，泌别清浊。冲通则表里相持，防御提升。",
      difficulty = 7,
      unlock_condition = "xinshaoyin",
      passive = { defense = 1 },
      mat = { herb = 2 }
    },
    sanjiao = {
      id = "sanjiao",
      name = "手少阳三焦经",
      description = "决渎之官，水道出焉。冲通则气机周流，内力上限提升。",
      difficulty = 8,
      unlock_condition = "xinbao",
      passive = { inner_power_max = 10 },
      mat = { polygonum = 2 }
    },
    pangguang = {
      id = "pangguang",
      name = "足太阳膀胱经",
      description = "州都之官，津液藏焉。冲通则经气敷布于背，体力上限提升。",
      difficulty = 9,
      unlock_condition = "du",
      passive = { energy_max = 10 },
      mat = { mung_bean = 2 }
    },
    weijing = {
      id = "weijing",
      name = "足阳明胃经",
      description = "胃为水谷之海，多气多血。冲通则纳谷化精，体力上限提升。",
      difficulty = 10,
      unlock_condition = "dachang",
      passive = { energy_max = 10 },
      mat = { polygonum = 2 }
    },
    danjing = {
      id = "danjing",
      name = "足少阳胆经",
      description = "中正之官，决断出焉。冲通则胆气雄壮，防御提升。",
      difficulty = 11,
      unlock_condition = "sanjiao",
      passive = { defense = 1 },
      mat = { ape_core = 1 }
    },
    pipi = {
      id = "pipi",
      name = "足太阴脾经",
      description = "仓廪之官，脾主统血。冲通则中气提挈，攻击提升。",
      difficulty = 12,
      unlock_condition = "weijing",
      passive = { attack = 1 },
      mat = { ape_core = 1 }
    }
  }
}
