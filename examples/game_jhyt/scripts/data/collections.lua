-- data/collections.lua：《江湖有田》图鉴 6 类条目清单（docs/54 §4.12，2026-10-06 1.7）。
--
-- 图鉴（收集）的**单一事实来源**：6 类 = 作物 / 鱼 / 矿 / 药材 / 菜肴 / 敌人。
-- 每类条目**引用已存在的注册表 id**（不另起新 id、不复制数值），从源头表派生，
-- 注册表增减条目时本清单自动跟随，杜绝"图鉴写死一份 id 与注册表漂移"。
--
-- 派生口径（与 docs/54 §4.12 一致）：
--   * 作物 crop = data.items 中 kind=="crop" 的**基础收获物**，排除 `_good`/`_super`
--     品质派生条目（品质派生是基础收获物的上浮价变体，不是独立标本——见
--     data/items.lua §4.1.2）。树果 kind=fruit、采集物 kind=material 不进本类。
--   * 鱼   fish = data.fish.fish 全部鱼种（物品层 kind=fish，见 data/items_fish.lua）。
--   * 矿   ore  = data/items_mine.items 全部（kind=ore/gem）+ 既有主表 iron_ore
--     （iron_ore 在 data/items.lua 里 kind=material，是 v0.2 旧条目，不重复声明）。
--   * 药材 herb = 药田种植的药材类作物（lingzhi/herb/polygonum/ginseng 等）。
--     与作物类是**重叠登记**：灵芝既是"作物收成"也是"药材标本"，两栏各自独立计数。
--   * 菜肴 dish = data/items_cooking.items 全部（kind=dish，膳房自炊产物）。
--   * 敌人 enemy = data.enemies.enemies 全部 persist_id（含 Boss 召唤槽位行——
--     忠实登记注册表"全部"；召唤物是否算独立标本留待阶段 3 战斗扩展口径裁定）。
--
-- 计数现状（2026-10-06，供 jhyt_m51_codex 断言锚点）：
--   crop=24 fish=23 ore=12 herb=11 dish=16 enemy=9（合计 95）。
-- docs/54 目标每类 ≥20：当前 herb=11 / enemy=9 未满——图鉴忠实登记**已存在**条目，
-- 缺口（药材扩种、敌人 12+3 战斗扩展）属后续波次，在交付说明标注，不在此凑数。
local M = {}
M.version = 1

local base = require("data.items")

-- 作物：kind=crop 基础收获物，剔除品质派生（_good/_super 是基础条目 5/6 字符后缀）。
local function build_crops()
  local list = {}
  for id, def in pairs(base) do
    if type(def) == "table" and def.kind == "crop" then
      if id:sub(-5) ~= "_good" and id:sub(-6) ~= "_super" then
        list[#list + 1] = id
      end
    end
  end
  table.sort(list)
  return list
end

-- 鱼：data.fish.fish 的键（鱼种 id 与物品 id 同名）。
local function build_fish()
  local fish_data = require("data.fish")
  local list = {}
  for id in pairs(fish_data.fish or {}) do
    list[#list + 1] = id
  end
  table.sort(list)
  return list
end

-- 矿：扩展矿/宝石表全部 + 主表既有 iron_ore。
local function build_ore()
  local mine = require("data.items_mine")
  local list = {}
  for id in pairs(mine.items or {}) do
    list[#list + 1] = id
  end
  list[#list + 1] = "iron_ore"
  table.sort(list)
  return list
end

-- 药材：药田种植的药材类作物（显式清单——是作物类的药材子集，语义独立成栏）。
local function build_herb()
  return {
    "chrysanthemum", -- 菊花
    "gouqi", -- 枸杞
    "herb", -- 草药
    "ginseng", -- 人参
    "lingzhi", -- 灵芝
    "mint", -- 薄荷
    "plum", -- 腊梅
    "polygonum", -- 何首乌
    "prunella", -- 夏枯草
    "winter_shen", -- 冬参
    "evergreen", -- 冬青
  }
end

-- 菜肴：膳房可做的全部菜肴（kind=dish）。
local function build_dish()
  local cooking = require("data.items_cooking")
  local list = {}
  for id in pairs(cooking.items or {}) do
    list[#list + 1] = id
  end
  table.sort(list)
  return list
end

-- 敌人：data.enemies 的全部 persist_id（含召唤槽位行，按 id 升序稳定）。
local function build_enemy()
  local enemies = require("data.enemies")
  local list = {}
  for _, e in ipairs(enemies.enemies or {}) do
    if type(e) == "table" and type(e.persist_id) == "string" then
      list[#list + 1] = e.persist_id
    end
  end
  table.sort(list)
  return list
end

-- 6 类的稳定顺序（UI 面板/验收输出按此序）与中文名。
M.order = { "crop", "fish", "ore", "herb", "dish", "enemy" }
M.name = {
  crop = "作物",
  fish = "鱼",
  ore = "矿",
  herb = "药材",
  dish = "菜肴",
  enemy = "敌人",
}

-- 每类条目清单（有序 id 数组）。
M.entries = {
  crop = build_crops(),
  fish = build_fish(),
  ore = build_ore(),
  herb = build_herb(),
  dish = build_dish(),
  enemy = build_enemy(),
}

-- 解锁奖励（每类进度达 50% / 100% 各触发一次，只发一次）。
-- 口径：发金币（state.money）；奖励**只引用既有物品/货币**，不新增 id。
-- item 字段预留（{ item=, count= }），阶段 4 接 UI 时可改发纪念物品而不改系统。
M.rewards = {
  [50] = { gold = 30 },
  [100] = { gold = 100 },
}

return M
