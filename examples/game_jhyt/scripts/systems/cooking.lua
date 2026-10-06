-- systems/cooking.lua：《江湖有田》料理与厨房 薄层（docs/33 §4.7 / docs/54 §4.6，1.3）。
--
-- 职责边界（不要重造 crafting）：
--   * 「消耗材料 → 按游戏分钟制作 → 收取产出」整条链路由 systems/crafting.lua 承载
--     （station="kitchen" 的配方全在 data/recipes.lua，膳房建筑 data/buildings.lua
--     station="kitchen" 已存在）。本模块**不**接管制作计时/扣料/收取；
--   * 本模块只做「菜谱图鉴数据源 + 做几道菜的统计」：crafting.collect 成功后
--     由 on_collect 记账（做过几道菜、解锁了哪些菜谱），供 UI 图鉴与 headless 断言；
--   * capture/restore/reset 随存档；validate_all 自洽校验（计数之和 == 总数）。
--
-- 接线（main.lua 补丁，由 Organizer 集成）：
--   * save_manager.register("cooking", cooking.capture, cooking.restore)；
--   * new_game() 末尾 cooking.reset()；
--   * on_day_start() 里 cooking.on_day_start()（当前无日结算，保留为未来
--     「当日特供菜/节令菜」等日级效果挂点）。
--   * crafting.collect 成功后 pcall(require('systems.cooking').on_collect,
--     job.output_item, job.recipe) 记账（headless 样例内显式调用同一函数走真实链路）。
local recipe_data = require("data.recipes")

local M = {}

local STATE_VERSION = 1

local stats = {
  cooked_total = 0,
  dishes = {} -- dish_item_id -> 累计做过份数
}

local function reset_state()
  stats = { cooked_total = 0, dishes = {} }
end

-- 制作完成记账（crafting.collect 成功后调用）。只认 station="kitchen" 的配方产出，
-- 避免把药庐/铁匠产物误记成菜。成功返回 true, output_item。
function M.on_collect(output_item, recipe_id)
  if type(output_item) ~= "string" or output_item == "" then
    return false, "bad_output"
  end
  local recipe = recipe_id and recipe_data.get(recipe_id) or nil
  if type(recipe) ~= "table" or recipe.station ~= "kitchen" then
    return false, "not_kitchen_recipe"
  end
  stats.cooked_total = (tonumber(stats.cooked_total) or 0) + 1
  stats.dishes[output_item] = (tonumber(stats.dishes[output_item]) or 0) + 1
  return true, output_item
end

-- 某道菜累计做过的份数（未做过为 0）。
function M.times(dish_id)
  return tonumber(stats.dishes[dish_id]) or 0
end

-- 累计做过的菜总份数。
function M.total()
  return tonumber(stats.cooked_total) or 0
end

-- 菜谱图鉴：已经做过至少一次（=已解锁）的菜肴 output item id，稳定排序。
function M.discovered()
  local out = {}
  for id in pairs(stats.dishes) do
    out[#out + 1] = id
  end
  table.sort(out)
  return out
end

-- 膳房全量菜谱 id（图鉴全量列表，按 data.recipes.for_station 的稳定排序）。
function M.recipe_book()
  local out = {}
  for _, id in ipairs(recipe_data.for_station("kitchen")) do
    out[#out + 1] = id
  end
  return out
end

-- 跨天挂点（main.on_day_start）：当前无每日结算逻辑，返回 0（无到期项）；
-- 保留为未来节令/特供菜等日级效果的挂点。
function M.on_day_start()
  return 0
end

function M.reset()
  reset_state()
end

function M.capture()
  local dishes = {}
  for id, count in pairs(stats.dishes) do
    dishes[id] = tonumber(count) or 0
  end
  return {
    version = STATE_VERSION,
    cooked_total = tonumber(stats.cooked_total) or 0,
    dishes = dishes
  }
end

function M.restore(data)
  if type(data) ~= "table" or tonumber(data.version) ~= STATE_VERSION then
    return false, "bad_version"
  end
  stats.cooked_total = tonumber(data.cooked_total) or 0
  stats.dishes = {}
  if type(data.dishes) == "table" then
    for id, count in pairs(data.dishes) do
      local n = tonumber(count)
      if type(id) == "string" and id ~= "" and n and n > 0 then
        stats.dishes[id] = math.floor(n)
      end
    end
  end
  return true
end

-- 自洽：dishes 计数之和必须等于 cooked_total（否则快照/记账漂移）。
function M.validate_all()
  local issues = {}
  local sum = 0
  for _, count in pairs(stats.dishes) do
    sum = sum + (tonumber(count) or 0)
  end
  if sum ~= (tonumber(stats.cooked_total) or 0) then
    issues[#issues + 1] = "systems/cooking.lua: cooked_total("
      .. tostring(stats.cooked_total) .. ") != sum(dishes)(" .. tostring(sum) .. ")"
  end
  return #issues == 0, issues
end

return M
