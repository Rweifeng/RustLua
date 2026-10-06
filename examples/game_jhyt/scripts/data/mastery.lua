-- data/mastery.lua：《江湖有田》熟练度注册表（docs/33 §7.1，2026-10-06 0.9）。
--
-- 改造计划 docs/54 §4.9/§5.4：MVP 的 3 类熟练度 0–5 升级为 **5 线 0–10 级 + 专精**。
--   * 5 条线：务农 farming / 采药 herbalism / 制药 alchemy / 武学 combat / 江湖 jianghu；
--   * 每线 0–10 级，所需累计 XP = 50/120/220/360/550/800/1100/1500/2000/2600
--     （10 级合计 2600，五线同一曲线，THRESHOLDS 顶层共享）；
--   * 每线 5 级与 10 级各选一次专精（2 选 1），共 10 个专精；专精选择进存档；
--   * MVP 只落地务农线专精的效果（精耕/药性/深耕/富饶），其余四线专精为
--     描述占位（Alpha 再接线效果），但选择链路（choose/has/存档）全量可用。
--
-- 专精 id 约束：线内唯一；slot ∈ {5, 10}；同一 slot 恰 2 个候选（2 选 1）。
return {
  version = 1,
  -- 每级所需**累计** XP（level n 的解锁阈值；level 0 = 0 XP）。
  thresholds = { 50, 120, 220, 360, 550, 800, 1100, 1500, 2000, 2600 },
  lines = {
    farming = {
      id = "farming",
      name = "务农",
      specializations = {
        { id = "jinggeng", slot = 5, name = "精耕", desc = "作物收获产量 +10%" },
        { id = "yaoxing", slot = 5, name = "药性", desc = "良好/上品品质概率 +10%/+5%" },
        { id = "gengshen", slot = 10, name = "深耕", desc = "再生作物再熟周期 -1 天（下限 2 天）" },
        { id = "furao", slot = 10, name = "富饶", desc = "作物收获产量再 +5%" }
      }
    },
    herbalism = {
      id = "herbalism",
      name = "采药",
      specializations = {
        { id = "shancha", slot = 5, name = "山茶识味", desc = "采集物保底良好（Alpha 接线）" },
        { id = "kuangcai", slot = 5, name = "筐采", desc = "单次采集量 +1（Alpha 接线）" },
        { id = "yaoyuan", slot = 10, name = "药园", desc = "药田作物成长 -1 天（Alpha 接线）" },
        { id = "linggan", slot = 10, name = "灵感", desc = "制药上品概率 +5%（Alpha 接线）" }
      }
    },
    alchemy = {
      id = "alchemy",
      name = "制药",
      specializations = {
        { id = "jingji", slot = 5, name = "精技", desc = "制作时间 -10%（Alpha 接线）" },
        { id = "shengchan", slot = 5, name = "增产", desc = "制作产出 +1（Alpha 接线）" },
        { id = "danwang", slot = 10, name = "丹王", desc = "解锁高级药剂（Alpha 接线）" },
        { id = "huoqi", slot = 10, name = "火候", desc = "药剂效果 +15%（Alpha 接线）" }
      }
    },
    combat = {
      id = "combat",
      name = "武学",
      specializations = {
        { id = "lianzhao", slot = 5, name = "练招", desc = "武学伤害 +10%（Alpha 接线）" },
        { id = "qingshen", slot = 5, name = "轻身", desc = "体力消耗 -10%（Alpha 接线）" },
        { id = "neigong", slot = 10, name = "内功", desc = "内力上限 +15（Alpha 接线）" },
        { id = "jueji", slot = 10, name = "绝技", desc = "解锁高阶招式（Alpha 接线）" }
      }
    },
    jianghu = {
      id = "jianghu",
      name = "江湖",
      specializations = {
        { id = "koucai", slot = 5, name = "口才", desc = "商店卖出价 +5%（Alpha 接线）" },
        { id = "renmai", slot = 5, name = "人脉", desc = "商店限购 +1（Alpha 接线）" },
        { id = "shenglu", slot = 10, name = "商路", desc = "解锁跨村行商（Alpha 接线）" },
        { id = "shengyu", slot = 10, name = "声誉", desc = "声望获取 +20%（Alpha 接线）" }
      }
    }
  }
}
