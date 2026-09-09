-- farm.lua：《江湖有田》山庄药田耕作循环（耕地→播种→浇水→生长→收获）。
--
-- 引擎事实（按 core/script.rs + core/tilemap.rs 实测）：
--   * tile cell 均为 0-index（x 列 / y 行）；row-major tiles[y*width+x]；
--     y=0 是地图最底行，y 增大朝上（世界 +Y 向上）。
--   * FarmGrid 是 main.scene 里第二张 Tilemap（entity.find("FarmGrid")），
--     tiles 初值全 0（空=透出 GroundMap 的夯土 DIRT），运行期本模块写 overlay。
--     manor_farm tileset 列：1=翻土 2=已耕已浇(深色) 3=苗期 4=生长期 5=成熟。
--   * grid.facing_cell(x,y,facing,size) 返回 {1=cx,2=cy} 一基数组。
--   * 地块明细数据（作物 id、已浇、有效成长日）存 map.state("farm_plots")，
--     切场景/读档后由本模块重绘到 FarmGrid；tile 只是渲染，不单独持久化。
--
-- 作物规则（对齐 docs/33 数值案）：
--   * 有效成长天数 = 已浇水天数（watered_days）；播种当天浇水不计当日成长。
--   * 当天播种、未浇不死亡；成熟后停留可采（MVP 不过熟腐烂）。
--   * 体力消耗：翻土 8 / 播种 3 / 浇水 4 / 收获 2（数值案 §3）。
--   * MVP 取舍：劳作暂不推进时钟（避免与日结算耦合），只耗体力；
--     时间消耗留待内容阶段接入。
local state = require("state")
local player = require("player")
local inventory = require("inventory")

local M = {}

-- FARM 矩形（须与 tools/gen_manor_scene.py 中 FARM 常量一致；0-index 含端点）
local FARM = { x0 = 14, x1 = 33, y0 = 4, y1 = 13 }
local TILE = 32

local ground_id = nil -- GroundMap（地表判定：翻土只能在 DIRT 上）
local farm_id = nil -- FarmGrid（overlay tile 渲染）
local width, height = 0, 0

-- map.state 持久键
local STATE_KEY = "farm_plots"
-- 作物数据（seed_item → crop 定义）
local crops = nil

local plots = {} -- key = "cx,cy" → { crop=seed_item|nil, watered_days=n, watered_today=bool }

local ENERGY_COST = { till = 8, plant = 3, water = 4, harvest = 2 }

local function plot_key(cx, cy)
  return tostring(cx) .. "," .. tostring(cy)
end

local function in_farm(cx, cy)
  return cx >= FARM.x0 and cx <= FARM.x1 and cy >= FARM.y0 and cy <= FARM.y1
end

-- 读取某个地块（不预创建；nil 表示从未开垦）
local function plot_at(cx, cy)
  return plots[plot_key(cx, cy)]
end

-- 创建已垦地块并落盘
local function plot_put(cx, cy, p)
  plots[plot_key(cx, cy)] = p
  local saved = map.state(STATE_KEY)
  if type(saved) ~= "table" then
    saved = {}
  end
  saved[plot_key(cx, cy)] = {
    crop = p.crop,
    watered_days = p.watered_days or 0,
    watered_today = p.watered_today == true
  }
  map.state(STATE_KEY, saved)
end

-- 是否 DIRT（GroundMap tile==2，可开垦）
local function is_dirt(cx, cy)
  if not ground_id then
    return false
  end
  return (tilemap.get_cell(ground_id, cx, cy) or 0) == 2
end

-- 由地块数据推导 overlay tile 值（0=无 overlay，透出地表）
local function tile_for(p)
  if not p then
    return 0
  end
  if not p.crop then
    return p.watered_today and 2 or 1 -- 已垦：湿土/干土
  end
  local crop = crops[p.crop]
  local growth = crop and crop.growth_days or 1
  if p.watered_days >= growth then
    return 5 -- 成熟
  elseif p.watered_days >= 1 then
    return 4 -- 生长期
  end
  return 3 -- 苗期（含播种当天）
end

-- 全量重绘 FarmGrid（切场景恢复 / 日结算后调用）
local function redraw_all()
  if not farm_id then
    return
  end
  for cx = FARM.x0, FARM.x1 do
    for cy = FARM.y0, FARM.y1 do
      local v = tile_for(plot_at(cx, cy))
      tilemap.set_cell(farm_id, cx, cy, v)
    end
  end
end

-- 消耗体力：不足返回 false 且给出提示
local function spend_energy(cost)
  if state.energy < cost then
    state.last_action = "tired"
    return false
  end
  state.energy = state.energy - cost
  return true
end

-- 播种（需要背包有种子），返回成功与否
local function plant_crop(p)
  local seed_id = nil
  for id, crop in pairs(crops) do
    if inv.count(state.inventory, crop.seed_item) > 0 then
      seed_id = crop.seed_item
      break
    end
  end
  if not seed_id then
    state.last_action = "no_seed"
    return false
  end
  if not spend_energy(ENERGY_COST.plant) then
    return false
  end
  inv.remove(state.inventory, seed_id, 1)
  p.crop = seed_id
  p.watered_days = 0
  p.watered_today = false
  state.last_action = "plant_" .. seed_id
  return true
end

-- 面向当前玩家面前的格执行劳作（E 键）。facing: up/down/left/right
function M.act(facing)
  local pid = player.ensure()
  if not pid then
    return "no_player"
  end
  if not farm_id then
    return "no_farm"
  end
  local pos = transform.get(pid)
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  local cx, cy = cell[1], cell[2]
  if not grid.in_bounds(cx, cy, width, height) then
    state.last_action = "out_of_bounds"
    return state.last_action
  end
  if not in_farm(cx, cy) or not is_dirt(cx, cy) then
    state.last_action = "not_farmland"
    return state.last_action
  end

  local p = plot_at(cx, cy)
  if p and p.crop then
    local crop = crops[p.crop]
    -- 成熟 → 收获
    if p.watered_days >= crop.growth_days then
      if not spend_energy(ENERGY_COST.harvest) then
        return state.last_action
      end
      local yield_count = crop.yield or 1
      inv.add(state.inventory, crop.harvest_item, yield_count)
      p.crop = nil
      p.watered_days = 0
      p.watered_today = false
      plot_put(cx, cy, p)
      state.last_action = "harvest_" .. crop.harvest_item
      redraw_all()
      return state.last_action
    end
    -- 未成熟 → 浇水
    if not spend_energy(ENERGY_COST.water) then
      return state.last_action
    end
    p.watered_today = true
    plot_put(cx, cy, p)
    state.last_action = "water_" .. p.crop
    redraw_all()
    return state.last_action
  end

  -- 已垦（含浇过水）→ 播种（空 DIRT 也可直接播，一步到位）
  if p then
    if plant_crop(p) then
      plot_put(cx, cy, p)
      redraw_all()
    end
    return state.last_action
  end

  -- 空 DIRT 但无 overlay 记录 → 翻土
  if not spend_energy(ENERGY_COST.till) then
    return state.last_action
  end
  p = { crop = nil, watered_days = 0, watered_today = false }
  plot_put(cx, cy, p)
  state.last_action = "till"
  redraw_all()
  return state.last_action
end

-- 日结算：main.lua on_day_start 在存档前调用。
-- 前一天浇过水的作物有效成长 +1，重置 watered_today，重绘 overlay。
function M.settle_day()
  local changed = false
  for key, p in pairs(plots) do
    if p.crop and p.watered_today then
      p.watered_days = p.watered_days + 1
      changed = true
    end
    p.watered_today = false
    changed = true
  end
  if changed then
    map.state(STATE_KEY, plots)
    redraw_all()
  end
  return true
end

-- 初始化/恢复（main.scene on_create 之后调用）。
-- 读 map.state("farm_plots") 恢复地块明细并重绘 overlay。
function M.init()
  crops = require("data.crops")
  ground_id = entity.find("GroundMap")
  farm_id = entity.find("FarmGrid")
  local g = ground_id and tilemap.get(ground_id)
  local f = farm_id and tilemap.get(farm_id)
  local w = (g and g.width) or (f and f.width) or 0
  local h = (g and g.height) or (f and f.height) or 0
  if w == 0 or h == 0 then
    log.warn("[farm] 未找到 GroundMap/FarmGrid，农田功能不可用")
    width, height = 0, 0
    farm_id = nil
    return false
  end
  width, height = w, h
  plots = {}
  local saved = map.state(STATE_KEY)
  if type(saved) == "table" then
    for key, data in pairs(saved) do
      if type(data) == "table" then
        plots[key] = {
          crop = data.crop,
          watered_days = tonumber(data.watered_days) or 0,
          watered_today = data.watered_today == true
        }
      end
    end
  end
  redraw_all()
  return true
end

-- 快照提供器（save_manager 注册）：map.state 已随存档落盘，
-- 此处仅作显式一致性镜像，供 save 整体 capture 无副作用返回。
function M.capture()
  return { plots = plots }
end

function M.restore()
  -- 地图状态在 init 阶段从 map.state 恢复；restore 仅在 save_manager.load()
  -- 的场景重建后补齐 overlay。真实明细源为 map.state，不二次覆写。
  redraw_all()
end

-- 测试/调试入口：返回地块统计
function M.stats()
  local total, planted, mature = 0, 0, 0
  for _, p in pairs(plots) do
    total = total + 1
    if p.crop then
      planted = planted + 1
      local crop = crops[p.crop]
      if crop and p.watered_days >= crop.growth_days then
        mature = mature + 1
      end
    end
  end
  return { planted = planted, mature = mature, tilled = total - planted }
end

return M
