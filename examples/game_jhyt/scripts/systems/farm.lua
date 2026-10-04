-- farm.lua：《江湖有田》山庄药田耕作循环（耕地→播种→浇水→生长→收获）。
--
-- 引擎事实（按 core/script.rs + core/tilemap.rs 实测）：
--   * tile cell 均为 0-index（x 列 / y 行）；row-major tiles[y*width+x]；
--     y=0 是地图最底行，y 增大朝上（世界 +Y 向上）。
--   * FarmGrid 是 manor.scene 里第二张 Tilemap（entity.find("FarmGrid")），
--     tiles 初值全 0（空=透出 GroundMap 的夯土 DIRT），运行期本模块写 overlay。
--     manor_farm tileset 列：1=翻土 2=已耕已浇(深色) 3=苗期 4=生长期 5=成熟。
--   * FarmMoist 是第三张 Tilemap（id 大于 FarmGrid，故压在作物之上），只有第 1 列
--     有内容（半透明湿痕）。有作物的地块 tile 值由 watered_days 决定、当日浇水不改
--     tile，所以"今天浇过水"这件事只能由这一层表达（watered_today → 1）。
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
local state = require("core.state")
local player = require("player.player")
local calendar = require("world.calendar")
local weather = require("world.weather")
local farm_data = require("data.farm")
-- M19-02：劳作音效（只在成功路径播放；缺文件/无设备静默降级）。
local audio = require("core.audio")
-- docs/48 effects 接线：翻土/浇水成功挂特效条带（hoe_dust / water_splash）。
local effects = require("systems.effects")

local M = {}

-- FARM 矩形（须与 tools/gen_manor_scene.py 中 FARM 常量一致；0-index 含端点）
local FARM = { x0 = 14, x1 = 33, y0 = 4, y1 = 13 }
local TILE = 32

local ground_id = nil -- GroundMap（地表判定：翻土只能在 DIRT 上）
local farm_id = nil -- FarmGrid（overlay tile 渲染）
local moist_id = nil -- FarmMoist（湿度层，画在 FarmGrid 之上；可缺，缺则不画湿痕）
local width, height = 0, 0

-- map.state 持久键
local STATE_KEY = "farm_plots"
-- 作物定义表：键 = crop short id（lingzhi/rice），含 seed_item/harvest_item 字段。
-- p.crop 统一存 short id，与 crops 键一致（曾误存 seed_item，已修正）。
local crops = nil
-- seed_item → crop short id 反查（仅用于旧档兼容归一化）
local crops_by_seed = nil

local plots = {} -- key = "cx,cy" → { crop=short_id|nil, watered_days=n, watered_today=bool }

local ENERGY_COST = { till = 8, plant = 3, water = 4, harvest = 2 }

local function plot_key(cx, cy)
  return tostring(cx) .. "," .. tostring(cy)
end

local function in_farm(cx, cy)
  return cx >= FARM.x0 and cx <= FARM.x1 and cy >= FARM.y0 and cy <= FARM.y1
end

local function land_type(cx, cy)
  for _, land in ipairs(farm_data.land or {}) do
    if cx >= land.x0 and cx <= land.x1 and cy >= land.y0 and cy <= land.y1 then
      return land.type
    end
  end
  return nil
end

local function equipped_tool(expected)
  local equipped = inv.equip_list(state.inventory) or {}
  if equipped.tool ~= expected then
    state.last_action = "need_" .. expected
    return false
  end
  return true
end

local function add_labor(minutes)
  state.labor_minutes = (state.labor_minutes or 0) + minutes
end

-- 惰性装载作物定义表（crops/crops_by_seed）：restore 可能在非山庄地图
-- （如 continue 直接进云泽村）先于 farm.init 运行 —— init 只绑定 FarmGrid，
-- 而地块数据本身与场景无关，不能因为作物表未初始化而丢 short id。
-- 必须在 seed_for_selected 之前声明（词法作用域，前向引用会退化为全局 nil）。
local function ensure_crops()
  if crops then
    return
  end
  crops = require("data.crops")
  crops_by_seed = {}
  for id, def in pairs(crops or {}) do
    if def and def.seed_item then
      crops_by_seed[def.seed_item] = id
    end
  end
end

-- 播种：按 selected_seed 反查作物（Alpha 扩种后不再硬编码两种种子；
-- crops_by_seed 由 ensure_crops 按 crops 全表构建）。回退取背包里第一个
-- 有库存的种子（稳定字母序），保证未主动选种时行为确定。
local function seed_for_selected()
  local selected = state.selected_seed
  ensure_crops()
  local crop = selected and crops_by_seed[selected] and crops[crops_by_seed[selected]]
  if crop and inv.count(state.inventory, crop.seed_item) > 0 then
    return crop.seed_item, crop
  end
  local ids = {}
  for id in pairs(crops or {}) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  for _, id in ipairs(ids) do
    local c = crops[id]
    if c and inv.count(state.inventory, c.seed_item) > 0 then
      state.selected_seed = c.seed_item
      return c.seed_item, c
    end
  end
  return nil
end

-- 由作物定义定位其 crop short id；未注册返回 nil。
local function crop_short_id(crop_def)
  for id, def in pairs(crops or {}) do
    if def == crop_def then
      return id
    end
  end
  return nil
end

-- 由地块存值（short id 或历史 seed_item）解析 crop short id；无法识别返回 nil。
local function crop_id_for(crop)
  if type(crop) ~= "string" then
    return nil
  end
  ensure_crops()
  if crops[crop] then
    return crop
  end
  return crops_by_seed and crops_by_seed[crop]
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

-- 由地块数据推导湿度层 tile 值（1=当日已浇的湿痕，0=无）。
-- 只管有作物的地块：空地（已垦未种）FarmGrid 第 2 列本身就是"湿土"，再叠一层是重复表达。
local function moist_for(p)
  if p and p.crop and p.watered_today then
    return 1
  end
  return 0
end

-- 全量重绘 FarmGrid（切场景恢复 / 日结算后调用）
-- 防御：farm_id 是模块级缓存，跨场景切换后旧实体 id 可能已被回收/复用
-- （M18-04 章节 j 实测 `entity has no tilemap`）。判活 + tilemap.get 双重校验，
-- 失效即视为「本图无农田」直接跳过重绘。
local function redraw_all()
  if not farm_id then
    return
  end
  if not entity.is_alive(farm_id) or not tilemap.get(farm_id) then
    return
  end
  local has_moist = moist_id and entity.is_alive(moist_id) and tilemap.get(moist_id)
  for cx = FARM.x0, FARM.x1 do
    for cy = FARM.y0, FARM.y1 do
      local p = plot_at(cx, cy)
      tilemap.set_cell(farm_id, cx, cy, tile_for(p))
      if has_moist then
        tilemap.set_cell(moist_id, cx, cy, moist_for(p))
      end
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
local function season_allowed(crop, season)
  for _, allowed in ipairs(crop.season or {}) do
    if allowed == season then
      return true
    end
  end
  return false
end

local function plant_crop(p, cx, cy)
  local seed_id, crop = seed_for_selected()
  if not seed_id then
    state.last_action = "no_seed"
    return false
  end
  if not season_allowed(crop, calendar.current()) then
    state.last_action = "wrong_season"
    return false
  end
  if crop.land ~= land_type(cx, cy) then
    state.last_action = "wrong_land"
    return false
  end
  if not spend_energy(ENERGY_COST.plant) then
    return false
  end
  inv.remove(state.inventory, seed_id, 1)
  -- p.crop 存 crop short id（与 crops 键一致，勿再存 seed_item）
  p.crop = crop_short_id(crop)
  p.watered_days = 0
  p.watered_today = false
  add_labor(10)
  state.last_action = "plant_" .. seed_id
  audio.sfx("sfx_plant")
  return true
end

-- 面向当前玩家面前的格执行劳作（E 键）。facing: up/down/left/right
function M.target(facing)
  local pid = player.ensure()
  if not pid then
    return nil, "no_player"
  end
  if not farm_id then
    return nil, "no_farm"
  end
  local pos = transform.get(pid)
  local cell = grid.facing_cell(pos.x, pos.y, facing or state.facing or "down", TILE)
  local cx, cy = cell[1], cell[2]
  if not grid.in_bounds(cx, cy, width, height) then
    return { x = cx, y = cy }, "out_of_bounds"
  end
  if not in_farm(cx, cy) or not is_dirt(cx, cy) then
    return { x = cx, y = cy }, "not_farmland"
  end
  return { x = cx, y = cy }
end

function M.can_interact(facing)
  local target, reason = M.target(facing)
  if not target then
    return false, reason
  end
  if reason then
    return false, reason
  end
  return true
end

-- 药田邻域（含外扩一格）：面前格或玩家所在的格落在里面，farm 目标才算"在场"。
local function near_farm(cx, cy)
  return cx >= FARM.x0 - 1 and cx <= FARM.x1 + 1
    and cy >= FARM.y0 - 1 and cy <= FARM.y1 + 1
end

function M.query(facing)
  local target, reason = M.target(facing)
  if not target then
    return { distance = math.huge, facing = 0 }, reason
  end
  -- 离药田很远时**不产生候选**：原实现无条件给候选（reason=not_farmland），于是玩家在
  -- 山庄门口按 E，HUD 反馈是"不是药田"——与"面前没有可交互目标"相比纯属误导，还会把
  -- 真正的目标（出口）挤成"兜底原因"（实机 2026-10-03：门口按 E 不能传送 + 反馈是"不是药田"）。
  local near = near_farm(target.x, target.y)
  if not near then
    local pid = player.ensure()
    local pos = pid and transform.get(pid) or nil
    if pos then
      local cell = grid.world_to_cell(pos.x, pos.y, TILE)
      near = near_farm(cell[1], cell[2])
    end
  end
  if not near then
    return nil, reason
  end
  return {
    cell = target,
    distance = 1,
    facing = 1,
    reason = reason
  }
end

function M.act(facing)
  local target, reason = M.target(facing)
  if not target then
    return reason
  end
  if reason then
    state.last_action = reason
    return state.last_action
  end
  local cx, cy = target.x, target.y

  local p = plot_at(cx, cy)
  if p and p.crop then
    local crop = crops[p.crop]
    -- 无法识别的作物（旧档坏数据）：视为已垦空地，允许重播覆盖
    if not crop then
      p.crop = nil
      p.watered_days = 0
      p.watered_today = false
      plot_put(cx, cy, p)
      redraw_all()
      state.last_action = "till"
      audio.sfx("sfx_hoe")
      effects.spawn("hoe_dust", (cx + 0.5) * TILE, (cy + 0.5) * TILE)
      return state.last_action
    end
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
      add_labor(3)
      plot_put(cx, cy, p)
      state.last_action = "harvest_" .. crop.harvest_item
      audio.sfx("sfx_harvest")
      redraw_all()
      return state.last_action
    end
    -- 未成熟 → 浇水
    if weather.is_rainy() then
      p.watered_today = true
      plot_put(cx, cy, p)
      state.last_action = "rain_watered"
      redraw_all()
      return state.last_action
    end
    if p.watered_today then
      state.last_action = "already_watered"
      return state.last_action
    end
    if not equipped_tool("watering_can") then
      return state.last_action
    end
    if not spend_energy(ENERGY_COST.water) then
      return state.last_action
    end
    p.watered_today = true
    add_labor(5)
    plot_put(cx, cy, p)
    state.last_action = "water_" .. p.crop
    audio.sfx("sfx_water")
    effects.spawn("water_splash", (cx + 0.5) * TILE, (cy + 0.5) * TILE)
    -- 主角形象：浇水（非循环条带，播完由 player.tick 落回 walk/idle）
    player.play("water")
    redraw_all()
    return state.last_action
  end

  -- 已垦（含浇过水）→ 播种（空 DIRT 也可直接播，一步到位）
  if p then
    if plant_crop(p, cx, cy) then
      plot_put(cx, cy, p)
      redraw_all()
    end
    return state.last_action
  end

  -- 空 DIRT 但无 overlay 记录 → 翻土
  if not equipped_tool("hoe") then
    return state.last_action
  end
  if not spend_energy(ENERGY_COST.till) then
    return state.last_action
  end
  p = { crop = nil, watered_days = 0, watered_today = false }
  add_labor(15)
  plot_put(cx, cy, p)
  state.last_action = "till"
  audio.sfx("sfx_hoe")
  effects.spawn("hoe_dust", (cx + 0.5) * TILE, (cy + 0.5) * TILE)
  -- 主角形象：挥锄（翻土）
  player.play("hoe")
  redraw_all()
  return state.last_action
end

-- 日结算：main.lua on_day_start 在存档前调用。
-- 前一天浇过水的作物有效成长 +1，重置 watered_today；仅当确有成长时才写盘。
-- （空垦地块正常不会带 watered_today；若存档脏数据带此标记则只重绘不落盘。）
function M.settle_day()
  local changed = false
  local reset_water = false
  for key, p in pairs(plots) do
    if p.crop and p.watered_today then
      p.watered_days = p.watered_days + 1
      changed = true
    end
    if p.watered_today then
      reset_water = true
    end
    p.watered_today = false
  end
  if changed then
    map.state(STATE_KEY, plots)
  end
  if changed or reset_water then
    redraw_all()
  end
  return changed
end

-- 初始化/恢复（manor.scene on_create 之后调用）。
-- 读 map.state("farm_plots") 恢复地块明细并重绘 overlay。
function M.init()
  crops = require("data.crops")
  crops_by_seed = {}
  for id, def in pairs(crops or {}) do
    if def and def.seed_item then
      crops_by_seed[def.seed_item] = id
    end
  end
  ground_id = entity.find("GroundMap")
  farm_id = entity.find("FarmGrid")
  moist_id = entity.find("FarmMoist")
  local g = ground_id and tilemap.get(ground_id)
  local f = farm_id and tilemap.get(farm_id)
  local w = (g and g.width) or (f and f.width) or 0
  local h = (g and g.height) or (f and f.height) or 0
  if w == 0 or h == 0 then
    log.warn("[farm] 未找到 GroundMap/FarmGrid，农田功能不可用")
    width, height = 0, 0
    farm_id = nil
    moist_id = nil
    return false
  end
  width, height = w, h
  plots = {}
  local saved = map.state(STATE_KEY)
  if type(saved) == "table" then
    for key, data in pairs(saved) do
      if type(data) == "table" then
        plots[key] = {
          crop = crop_id_for(data.crop),
          watered_days = tonumber(data.watered_days) or 0,
          watered_today = data.watered_today == true
        }
      end
    end
  end
  redraw_all()
  return true
end

function M.capture()
  local snapshot = {}
  for key, p in pairs(plots) do
    snapshot[key] = {
      crop = p.crop,
      watered_days = p.watered_days or 0,
      watered_today = p.watered_today == true
    }
  end
  return { plots = snapshot, state_key = STATE_KEY }
end

function M.restore(data)
  if type(data) == "table" and type(data.plots) == "table" then
    plots = {}
    for key, p in pairs(data.plots) do
      if type(p) == "table" then
        plots[key] = {
          crop = crop_id_for(p.crop),
          watered_days = tonumber(p.watered_days) or 0,
          watered_today = p.watered_today == true
        }
      end
    end
    map.state(STATE_KEY, plots)
  end
  redraw_all()
end

-- 新档复位（P0：新游戏不得继承上一局农田）。与 building/crafting 的 reset 同构：
-- 清空内存地块 + 主动写一份空快照到 map.state，再重绘 overlay（把上一局画在
-- FarmGrid 上的瓦片抹回空）。
--
-- 此前 `farm` 没有 reset，「开始新游戏」只复位了 state/quest/... 等模块，农田
-- 明细留在 map.state("farm_plots") 里 → 同进程内「继续 → 玩 → 返回标题 →
-- 开始新游戏」会读回旧档的开垦地块、作物与浇水天数。
function M.reset()
  plots = {}
  map.state(STATE_KEY, {})
  redraw_all()
end

-- 只读查询：该格是否已有开垦/作物记录（M18-02 建筑占用判定用；不改任何状态）。
function M.is_plot(cx, cy)
  local p = plots[plot_key(cx, cy)]
  return p ~= nil
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
