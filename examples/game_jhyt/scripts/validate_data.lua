-- validate_data.lua：领域数据统一校验入口（M20-01）。
--
-- 单一事实来源：启动（main.lua 的 validate_content）、发布前检查
-- （`game --check` / `editor --check`）与编辑器资源扫描共用本模块，
-- 保证三处诊断结果一致，且规则只写一次。
--
-- 契约：
--   local report = require("validate_data").check()
--   * report == ""               数据全部通过；
--   * report ~= ""               以换行分隔的问题列表，每条含来源模块与
--                                文件/对象/字段路径（由各模块 validate_all 给出）。
--   * check({ strict = true })   额外做「模块可加载性」检查（require 失败也报错）。
local M = {}

-- 校验器清单：{ 诊断前缀, 校验模块, 模块 require 路径 }。新增领域数据校验只在此处登记。
-- M19-02 追加 "tutorial"（data/tutorial.lua 的引导步骤表）。
-- M21 追加 "appearances"（主角形象清单；会交叉校验 data/characters.lua 的 role=player）。
-- M23 追加：
--   * "registries"（data/registry_check.lua）：items/tools/crops/farm 是**纯表注册表**
--     （返回注册表本身，`pairs()` 会被 player/inventory.lua 当物品定义注册），校验器
--     不能写在它们自己文件里，见该模块头部注释；
--   * "gather"、"enemy_art"：两个模块**早就有** validate_all，此前漏登记 →
--     它们的校验从来没有在启动/`--check` 里跑过（已由 jhyt_m23_data_validation 的
--     「登记覆盖」断言守住，避免再漏）。
-- 第三个字段是 `require` 路径：`M.registered()` 与验收样例靠它做"清单覆盖"断言。
local function validators()
  return {
    { "maps", require("data.maps"), "data.maps" },
    { "places", require("data.places"), "data.places" },
    { "npcs", require("data.npcs"), "data.npcs" },
    { "characters", require("data.characters"), "data.characters" },
    { "appearances", require("data.appearances"), "data.appearances" },
    { "schedule", require("world.schedule"), "world.schedule" },
    { "quest", require("world.quest"), "world.quest" },
    { "dialogue", require("world.dialogue"), "world.dialogue" },
    { "shop", require("systems.shop"), "systems.shop" },
    { "skills", require("data.skills"), "data.skills" },
    { "enemies", require("data.enemies"), "data.enemies" },
    -- M25 追加："combat"（data/combat.lua）：受击反馈的全局调参（玩家无敌帧、
    -- 敌人默认抵抗）。招式侧的攻击效果（hitstun/knockback）在 data/skills.lua 校验，
    -- 敌人侧的抵抗覆盖在 data/enemies.lua 校验。
    { "combat", require("data.combat"), "data.combat" },
    { "relationships", require("data.relationships"), "data.relationships" },
    { "buildings", require("data.buildings"), "data.buildings" },
    { "recipes", require("data.recipes"), "data.recipes" },
    { "tutorial", require("data.tutorial"), "data.tutorial" },
    -- M27 追加："goals"（data/goals.lua）：七日目标表（7 天 × 每天 3–4 条，
    -- 条件是既有系统的只读查询，日结算挂在每天最后一条 day_start 上）。
    { "goals", require("data.goals"), "data.goals" },
    -- M28 追加："drops"（data/drops.lua）：敌人掉落表。掉落物必须真实存在于
    -- items/tools，区间与概率合法，且不允许"写了记录但什么都不产出"。
    { "drops", require("data.drops"), "data.drops" },
    -- M29 追加："season_art"（data/season_art.lua）：四季地表贴图登记表。
    -- 结构不变量（实体名/占位符个数/季节闭集）在 Lua 侧校验；贴图文件是否真的存在
    -- 由验收样例查文件系统（Lua 没有列目录能力）。
    { "season_art", require("data.season_art"), "data.season_art" },
    { "registries", require("data.registry_check"), "data.registry_check" },
    { "gather", require("data.gather"), "data.gather" },
    { "enemy_art", require("data.enemy_art"), "data.enemy_art" },
    -- M32 追加："layout"（ui/layout_check.lua）：版面契约（2026-09-27 转横屏）。除面板几何自洽
    -- （矩形在设计画布内、面板互不重叠）之外，还守**设计分辨率契约**——
    -- `scenes/*.scene` 的 `meta ui_reference_width/height` 与 `project.toml` 的
    -- `window_width/height` 必须都等于 `ui/layout.lua` 的 1280×720（横屏）。
    -- 这条契约横跨 Lua 与场景/工程文件，任何一处漂移都会让"场景 UI 与 Lua 面板
    -- 各按一套尺寸排"，画面上错位却没有任何报错。
    { "layout", require("ui.layout_check"), "ui.layout_check" },
    -- docs/48 收尾追加："portrait"（ui/portrait.lua，15 张立绘映射）与
    -- "skin"（ui/skin.lua，ui 换肤注册表，75 键）：validate_all 校验数量/路径格式/重复。
    { "portrait", require("ui.portrait"), "ui.portrait" },
    { "skin", require("ui.skin"), "ui.skin" },
    -- M59 追加："prefab"（ui/prefab.lua）：UI 预设（`assets/ui/prefabs/*.ui.json`）的加载与版面不变量。
    -- 这一条必须在启动门里跑：预设是**数据文件**，改坏了不会有语法错误，表现只会是
    -- "某个面板少一块/压一行"，而数据文件恰恰是最容易随手改的那一份。
    -- 读文件走 `data.load`（VFS），不走 `io`——web/微信宿主没有 `io`（`ui/layout_check.lua`
    -- 的教训：登记一个需要 `io` 的校验器会让两条 web 发布形态的启动当场失败）。
    { "prefab", require("ui.prefab"), "ui.prefab" },
    -- M32 相机收口追加："camera_view"（world/camera_view.lua）：每张玩法地图在场景里
    -- 声明的 `Camera` 契约——恰好 1 台、正交投影、`zoom=2`（对标星露谷视觉规格，见
    -- docs/51 §2.2 修订）、且 `bounds` 逐值等于该场景
    -- GroundMap 的真实地图矩形。漂移的表现是"相机取景跑出地图边缘"，但不会有任何报错。
    { "camera_view", require("world.camera_view"), "world.camera_view" },
    -- M34-02 追加："festivals"（data/festivals.lua）：节气事件表（1.5 起 8 条，每季 2）。这里守的是
    -- **时间轴自洽**——同一天不能有两个节气（重叠窗口是纯数据错误，运行时只会静默取第一条）、
    -- 四季每季至少一个事件、效果键必须在白名单内且有对应的消费方模块。
    { "festivals", require("data.festivals"), "data.festivals" },
    -- 0.9 追加："mastery"（data/mastery.lua + systems/mastery.lua）：熟练度 5 线
    -- 10 级 + 专精。守的是结构不变量——阈值表恰 10 档、线条目合法、专精 id 全局唯一、
    -- 每线 5/10 级槽位各恰 2 个候选（2 选 1）。
    { "mastery", require("systems.mastery"), "systems.mastery" },
    -- 1.0 追加："animal"（data/animals.lua + systems/animal.lua）：畜牧。守 6 种动物
    -- 定义合法（畜舍归属/产物 id 存在/周期为正）、畜舍建筑登记在 buildings 里。
    { "animal", require("systems.animal"), "systems.animal" },
    -- 1.1 追加："fishing"（data/fish.lua + systems/fishing.lua）：钓鱼。守 4 水域/
    -- 23 鱼种结构不变量（难度 1..5、归属水域存在、data/fish.sell == items_fish 同名物品 sell）。
    { "fishing", require("systems.fishing"), "systems.fishing" },
    -- 1.2 追加："mine"（data/mine.lua + systems/mine.lua）：青云矿脉 30 层。守恰 30 层、
    -- 电梯层落在 5/10/15/20/25/30、矿石/宝石池引用合法、difficulty 1..10、
    -- 运行期 current 必须已解锁且入口层恒解锁。
    { "mine", require("systems.mine"), "systems.mine" },
    -- 1.3 追加："cooking"（data/items_cooking.lua + systems/cooking.lua）：料理统计薄层。
    { "cooking", require("systems.cooking"), "systems.cooking" },
    -- 1.4 追加："relationship_events"（data/relationship_events.lua + systems/relationship_events.lua）：
    -- ≥30 条好感事件，npc/门槛/奖励物品引用完整性。
    { "relationship_events", require("systems.relationship_events"), "systems.relationship_events" },
    -- 1.6 追加："questboard"（data/questboard.lua + systems/questboard.lua）：任务板 3 层。
    -- 守三层规则（daily/weekly/quarterly 刷新锚点）、22 模板 objective 只引用既有物品。
    { "questboard", require("systems.questboard"), "systems.questboard" },
    -- 1.7 追加："collection"（data/collections.lua + systems/collection.lua）：图鉴 6 类。
    -- 守 6 类齐全非空、类内 id 无重复、作物/药材/菜肴 id 在物品表真实存在、奖励档 50/100 齐全。
    { "collection", require("systems.collection"), "systems.collection" },
    -- 1.8 追加："achievements"（data/achievements.lua + systems/achievement.lua）：成就 30。
    -- 守 id 唯一、谓词类型白名单、奖励只引用既有物品。
    { "achievements", require("systems.achievement"), "systems.achievement" },
    -- 1.9 追加："meridian"（data/meridian.lua + systems/meridian.lua）：12 条经脉。
    -- 守结构不变量——恰 12 条、难度 1..12 唯一、恰 1 根、前置链无环、
    -- 被动键闭集（attack/defense/energy_max/inner_power_max）、冲穴材料已注册。
    { "meridian", require("systems.meridian"), "systems.meridian" },
    -- 2.0 追加："clinic"（data/clinic.lua 病种处方 + 疫情表）：医馆经营与疫情事件。
    { "clinic", require("data.clinic"), "data.clinic" },
    -- 2.1 追加："solarterm"（data/solar_terms.lua）：二十四节气注册表。守时间轴自洽——
    -- 24 个节气每季恰 6 个、季内日 1..28 不撞日、修饰键在白名单内、festival_link 指向真实节日。
    { "solarterm", require("data.solar_terms"), "data.solar_terms" },
    -- 2.2 追加："black_market"（data/black_market.lua + systems/black_market.lua）：黑市商品池/抉择事件/声望档位。
    { "black_market", require("systems.black_market"), "systems.black_market" },
    -- 2.3 追加："tea_party"（systems/tea_party.lua，委托 data/tea_party.lua 校验话题池/
    -- 门槛/茶品白名单/结算常数的引用完整性）。
    { "tea_party", require("systems.tea_party"), "systems.tea_party" },
    -- 2.4 追加："endings"（data/endings.lua + systems/endings.lua）：多结局 5 条。
    -- 守 id 唯一、谓词白名单（reputation/mastery_level/day）、阵营/熟练度线引用存在、门槛落在可达区间。
    { "endings", require("systems.endings"), "systems.endings" },
    -- 3.3 补登记（m23「登记覆盖」门：凡定义 validate_all 的模块必须登记，否则校验永不运行）：
    -- 系统侧缺 3 个（shipping 返回字符串自检、clinic、solar_terms），数据侧缺 6 个
    -- （achievements/endings/black_market/tea_party/relationship_events/mine 的 data 模块
    -- 各有 validate_all，此前只登记了它们的 systems 孪生）。
    { "shipping", require("systems.shipping"), "systems.shipping" },
    { "clinic_sys", require("systems.clinic"), "systems.clinic" },
    { "solarterm_sys", require("systems.solar_terms"), "systems.solar_terms" },
    { "achievements_data", require("data.achievements"), "data.achievements" },
    { "endings_data", require("data.endings"), "data.endings" },
    { "black_market_data", require("data.black_market"), "data.black_market" },
    { "tea_party_data", require("data.tea_party"), "data.tea_party" },
    { "relationship_events_data", require("data.relationship_events"), "data.relationship_events" },
    { "mine_data", require("data.mine"), "data.mine" }
  }
end

-- 已登记校验器的只读视图（验收样例用它断言"带 validate_all 的模块都登记了"）。
function M.registered()
  local out = {}
  for _, entry in ipairs(validators()) do
    out[#out + 1] = { name = entry[1], path = entry[3] or "" }
  end
  return out
end

-- 返回问题列表（已排序稳定：按前缀）；无问题时为空表。
function M.issues(options)
  options = options or {}
  local issues = {}
  for _, entry in ipairs(validators()) do
    local name = entry[1]
    local module = entry[2]
    if type(module) ~= "table" or type(module.validate_all) ~= "function" then
      if options.strict then
        issues[#issues + 1] = "[" .. name .. "] 缺少 validate_all 校验入口"
      end
    else
      local ok, valid, list = pcall(module.validate_all)
      if not ok then
        issues[#issues + 1] = "[" .. name .. "] 校验执行失败: " .. tostring(valid)
      elseif valid ~= true then
        for _, issue in ipairs(list or {}) do
          issues[#issues + 1] = "[" .. name .. "] " .. tostring(issue)
        end
      end
    end
  end
  table.sort(issues)
  return issues
end

-- 契约入口：返回 ""（通过）或换行分隔的问题列表。
function M.check(options)
  local issues = M.issues(options)
  if #issues == 0 then
    return ""
  end
  return table.concat(issues, "\n")
end

return M
