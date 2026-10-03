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
    -- M32 相机收口追加："camera_view"（world/camera_view.lua）：每张玩法地图在场景里
    -- 声明的 `Camera` 契约——恰好 1 台、正交投影、`zoom=2`（对标星露谷视觉规格，见
    -- docs/51 §2.2 修订）、且 `bounds` 逐值等于该场景
    -- GroundMap 的真实地图矩形。漂移的表现是"相机取景跑出地图边缘"，但不会有任何报错。
    { "camera_view", require("world.camera_view"), "world.camera_view" }
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
