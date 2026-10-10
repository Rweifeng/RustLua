-- farm.lua：《江湖有田》山庄药田耕作循环（耕地→播种→浇水→生长→收获）。
--
-- 引擎事实（按 core/script.rs + core/tilemap.rs 实测）：
--   * tile cell 均为 0-index（x 列 / y 行）；row-major tiles[y*width+x]；
--     y=0 是地图最底行，y 增大朝上（世界 +Y 向上）。
--   * FarmGrid 是 manor.scene 里第二张 Tilemap（entity.find("FarmGrid")），
--     tiles 初值全 0（空=透出 GroundMap 的夯土 DIRT），运行期本模块写 overlay。
--     M72-08 起它**只画土**，manor_farm 两列：1=犁地（新翻的松土）2=干涸（裂缝）。
--     作物贴图搬去了 CropGrid，三态里的"湿润"搬去了 FarmMoist + 顶点色 tint。
--   * CropGrid（id 31，画在 FarmMoist 之上）是 8×29 的作物图集层，一格一枚瓦，
--     瓦值 = `data/crop_art.entry(crop short id, 阶段)`（1 基图集条目号，0=空）。
--     寻址表由 tools/gen_crop_art.py 生成，与 assets/art/crops/manor_crops.png
--     和场景里的 atlas_cols/atlas_rows 同源，改一个必须重出另两个。
--   * FarmMoist 是第三张 Tilemap（id 大于 FarmGrid，故压在土之上、作物之下），只有
--     第 1 列有内容（半透明湿痕）。当日浇过水 → tile 1 + 按 watered_days 取档的
--     `tilemap.set_tint` 染色（越浇越淡）；湿痕撤掉时同格 clear_tint。
--   * grid.facing_cell(x,y,facing,size) 返回 {1=cx,2=cy} 一基数组。
--   * 地块明细数据（作物 id、已浇、有效成长日）存 map.state("farm_plots")，
--     切场景/读档后由本模块重绘到 FarmGrid；tile 只是渲染，不单独持久化。
--
-- 作物规则（对齐 docs/33 数值案）：
--   * 有效成长天数 = 已浇水天数（watered_days）；播种当天浇水不计当日成长。
--   * 当天播种、未浇不死亡；成熟后停留可采（MVP 不过熟腐烂）。
--   * 体力消耗：翻土/浇水按装备工具档位减耗（docs/33 §5.4：基础锄 8、精铁 6、
--     寒铁 5、陨铁/灵铁 4；基础壶 4、精铁 3、寒铁/陨铁/灵铁 2），播种 3 / 收获 2。
--   * 再生作物（crop.regrow_days，2026-10-05 四季矩阵扩展）：成熟收获后**地块保留作物**，
--     `cycles` 记录已收茬数（首茬为 0）；再浇 `regrow_days` 天水即复熟，可再次收获。
--     非再生作物行为不变（收获后地块清空，与 M23 口径一致）。
--   * MVP 取舍：劳作暂不推进时钟（避免与日结算耦合），只耗体力；
--     时间消耗留待内容阶段接入。
local state = require("core.state")
local player = require("player.player")
local calendar = require("world.calendar")
local weather = require("world.weather")
local farm_data = require("data.farm")
-- M72-08：农田三态的图集寻址 + 湿痕 tint 档，作物阶段图寻址。
-- 两张表都由 tools/gen_*.py 生成，运行期只读。
local farm_soil = require("data.farm_soil")
local crop_art = require("data.crop_art")
-- 工具档位（docs/33 §5.4 工具升级）：翻土/浇水按装备工具 tier 减耗。
local tools_data = require("data.tools")
-- M19-02：劳作音效（只在成功路径播放；缺文件/无设备静默降级）。
local audio = require("core.audio")
-- docs/48 effects 接线：翻土/浇水成功挂特效条带（hoe_dust / water_splash）。
local effects = require("systems.effects")
-- 熟练度（docs/33 §7.1，0.9）：务农 XP（翻土/播种/收获 +5）与品质/产量/再生钩子。
local mastery = require("systems.mastery")

local M = {}

-- FARM 矩形（须与 tools/gen_manor_scene.py 中 FARM 常量一致；0-index 含端点）
local FARM = { x0 = 14, x1 = 33, y0 = 4, y1 = 13 }
local TILE = 32

local ground_id = nil -- GroundMap（地表判定：翻土只能在 DIRT 上）
local farm_id = nil -- FarmGrid（overlay tile 渲染）
local moist_id = nil -- FarmMoist（湿度层，画在 FarmGrid 之上；可缺，缺则不画湿痕）
local crop_id = nil -- CropGrid（作物层，画在 FarmMoist 之上；可缺，缺则只看到土）
local width, height = 0, 0

-- map.state 持久键
local STATE_KEY = "farm_plots"
-- 作物定义表：键 = crop short id（lingzhi/rice），含 seed_item/harvest_item 字段。
-- p.crop 统一存 short id，与 crops 键一致（曾误存 seed_item，已修正）。
local crops = nil
-- seed_item → crop short id 反查（仅用于旧档兼容归一化）
local crops_by_seed = nil

local plots = {} -- key = "cx,cy" → { crop=short_id|nil, watered_days=n, watered_today=bool, cycles=n, fertilizer=item_id|nil, sprinkler=item_id|nil }

-- 体力消耗（docs/33 §3/§5.4）：播种/收获/施肥固定；翻土/浇水按装备工具档位减耗。
-- 档位 1 = 基础工具（消耗与 MVP 基线一致，旧档行为不变）；第 3 档封顶下限
-- （翻土 4、浇水 2），第 4 档不再降，避免"零成本劳动"。
local ENERGY_COST = { plant = 3, harvest = 2, fertilize = 2 }
local TIER_COST = {
  till = { [1] = 8, [2] = 6, [3] = 5, [4] = 4, [5] = 4 },
  water = { [1] = 4, [2] = 3, [3] = 2, [4] = 2, [5] = 2 },
}

-- 装备工具档位对应的劳动消耗；工具未注册/无 tier 一律按基础档兜底。
local function tier_cost(action, tool_id)
  local def = tools_data and tools_data[tool_id]
  local tier = (def and def.tier) or 1
  return TIER_COST[action][tier] or TIER_COST[action][1]
end

-- 地块已施肥料的效果（docs/33 §4.1.3）：p.fertilizer 存肥料 item id，效果由
-- data/items.lua 的 effect 字段决定（quality/growth/yield）——数据驱动闭环，
-- registry_check 已强制该字段三选一，新增肥料无需改本文件。
local function fertilizer_effect(p)
  if not (p and p.fertilizer) then
    return nil
  end
  local def = require("data.items")[p.fertilizer]
  return def and def.effect or nil
end

-- 成熟所需浇水天数：再生作物已收过茬（cycles > 0）时按 regrow_days 复熟，
-- 否则按 growth_days 首茬成熟。非再生作物恒用 growth_days（行为不变）。
-- 骨肥（effect=growth）：所需浇水天数 -1，下限 1（首茬与再生茬同享）。
-- 深耕专精（mastery 0.9）：再生茬再熟周期再 -1 天，下限 2（首茬不受影响）。
local function growth_need(p, crop)
  local need
  if crop and crop.regrow_days and p.cycles and p.cycles > 0 then
    need = math.max(2, crop.regrow_days - mastery.farming_bonus().regrow_cut)
  else
    need = (crop and crop.growth_days) or 1
  end
  if fertilizer_effect(p) == "growth" and need > 1 then
    need = need - 1
  end
  return need
end

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

-- 装备族判定：高阶工具（hoe_iron 等）与基础工具（hoe）同族，装备任意档位都算
-- "锄头在手"。family 取 data/tools.lua 的 family 字段（与基础工具 id 一致）。
local function equipped_tool(expected_family)
  local equipped = inv.equip_list(state.inventory) or {}
  local tool_id = equipped.tool
  local def = tool_id and tools_data[tool_id]
  local family = (def and def.family) or tool_id
  if family ~= expected_family then
    state.last_action = "need_" .. expected_family
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
    watered_today = p.watered_today == true,
    cycles = p.cycles or 0,
    fertilizer = p.fertilizer,
    sprinkler = p.sprinkler,
    tree_days = p.tree_days,
    fruited_today = p.fruited_today == true
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

-- 由地块数据推导 FarmGrid 的土面瓦值（0=无 overlay，透出地表夯土）。
-- 三态里这里只占两态：**犁地 / 干涸**。湿润是 FarmMoist 的叠痕 + tint，不画第三列
-- ——一列土 + 一层可撤的半透明，比再铺一张全幅贴图省体积，也让"今天浇过"能撤。
-- 判据：当日浇过（水还在）或从没浇过（新翻的）都算松土；浇过几天又断了水才显裂缝。
local function tile_for(p)
  if not p then
    return 0
  end
  if p.watered_today or (p.watered_days or 0) == 0 then
    return farm_soil.tile.tilled
  end
  return farm_soil.tile.dry
end

-- 由地块数据推导湿度层 tile 值（1=当日已浇的湿痕，0=无）。
-- 空垦地块也算：M72-08 起 FarmGrid 第 2 列是"干涸裂缝"，湿痕不再是它的重复表达。
local function moist_for(p)
  if p and p.watered_today then
    return 1
  end
  return 0
end

-- 作物阶段（1 基，对应 data/crop_art.lua 每行的前 N 槽）。
-- 非果树：把 watered_days 从 0 线性铺到成熟所需天数 need，两端钉死在 1 与 N——
-- 阶段数 N 由图集给（粮食 4、灵药 5），**不改** need/growth_days 的任何数值口径。
-- 果树：按 tree_days 三段（苗 / 幼树 / 成树），3 态是 docs/58 果树列的口径。
local function crop_stage(p, crop)
  local rec = crop_art.crops[p.crop]
  if not rec then
    return 0
  end
  if crop and crop.tree then
    local need = crop.tree_days or 15
    local days = p.tree_days or 0
    if days >= need then
      return 3
    elseif days * 3 >= need then
      return 2
    end
    return 1
  end
  local need = growth_need(p, crop)
  if need <= 0 then
    return rec.stages
  end
  local grown = math.min(p.watered_days or 0, need)
  return 1 + math.floor(grown * (rec.stages - 1) / need)
end

-- 某地块在 CropGrid 上的瓦值（0=空，透出土面）。
local function crop_for(p)
  if not (p and p.crop) then
    return 0
  end
  local crop = crops and crops[p.crop]
  return crop_art.entry(p.crop, crop_stage(p, crop))
end

-- 画一格（三层各写各的；层缺失就跳过那层）。redraw_all 与收成动画都走这里，
-- 保证"单格改写"和"全量重绘"对同一份地块数据给出同样的像素。
local function draw_cell(cx, cy)
  local p = plot_at(cx, cy)
  tilemap.set_cell(farm_id, cx, cy, tile_for(p))
  if moist_id and entity.is_alive(moist_id) and tilemap.get(moist_id) then
    local m = moist_for(p)
    tilemap.set_cell(moist_id, cx, cy, m)
    -- tint 跟着湿痕走：撤痕必须同时撤色，否则下一种在这个位置的作物会莫名发蓝。
    if m == 0 then
      tilemap.clear_tint(moist_id, cx, cy)
    else
      local rgba = farm_soil.moist_tints[farm_soil.moist_tint_id(p.watered_days or 0)]
      tilemap.set_tint(moist_id, cx, cy, rgba[1], rgba[2], rgba[3], rgba[4])
    end
  end
  if crop_id and entity.is_alive(crop_id) and tilemap.get(crop_id) then
    tilemap.set_cell(crop_id, cx, cy, crop_for(p))
  end
end

-- 全量重绘三层（切场景恢复 / 日结算后调用）
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
  ensure_crops()
  for cx = FARM.x0, FARM.x1 do
    for cy = FARM.y0, FARM.y1 do
      draw_cell(cx, cy)
    end
  end
end

-- 收成动画（M72-08）：成熟格在 CropGrid 上连打四格（成熟 → 三帧抖动），每帧 100ms
-- （docs/51 §2.2 快动作口径），放完按真实地块数据重画那一格。
-- **只有收成动**：docs/51 §2.3 明说生长阶段切换保持静态——生长是"每天看一眼"的慢事，
-- 每换一档就抖一下，读不出长了什么，只读出烦。
local HARVEST_FRAME_SEC = 0.1
M.HARVEST_FRAME_SEC = HARVEST_FRAME_SEC
local harvest_anim = {} -- plot_key → { cx, cy, frames = {瓦值}, index = 下一帧, left = 剩余秒 }

local function start_harvest_anim(cx, cy, crop_short, mature_entry)
  if not crop_id or mature_entry == 0 then
    return
  end
  local frames = { mature_entry }
  for i = 0, crop_art.shake_slots - 1 do
    local entry = crop_art.shake_entry(crop_short, i)
    if entry > 0 then
      frames[#frames + 1] = entry
    end
  end
  if #frames < 2 then
    return
  end
  tilemap.set_cell(crop_id, cx, cy, frames[1])
  harvest_anim[plot_key(cx, cy)] = { cx = cx, cy = cy, frames = frames, index = 2, left = HARVEST_FRAME_SEC }
end

-- 推进收成动画（main.lua update 每帧调，固定 1/60 步）。
function M.tick(dt)
  if not next(harvest_anim) then
    return
  end
  if not (crop_id and entity.is_alive(crop_id) and tilemap.get(crop_id)) then
    harvest_anim = {}
    return
  end
  for key, anim in pairs(harvest_anim) do
    anim.left = anim.left - (dt or HARVEST_FRAME_SEC)
    if anim.left > 0 then
    else
      anim.left = anim.left + HARVEST_FRAME_SEC
      if anim.index <= #anim.frames then
        tilemap.set_cell(crop_id, anim.cx, anim.cy, anim.frames[anim.index])
        anim.index = anim.index + 1
      else
        harvest_anim[key] = nil
        draw_cell(anim.cx, anim.cy)
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

-- 温室（地热药圃，docs/33 §4.1.6）：查询山庄 greenhouse 建筑等级（0 = 未建）。
-- 1 级解锁温室田四季可种；2 级（药圃深耕）温室田收获 +1。
local function greenhouse_level()
  local buildings = require("systems.building")
  local list = buildings.list()
  for _, rec in ipairs(list) do
    if rec.building == "greenhouse" then
      return rec.level or 1
    end
  end
  return 0
end

local function plant_crop(p, cx, cy)
  local seed_id, crop = seed_for_selected()
  if not seed_id then
    state.last_action = "no_seed"
    return false
  end
  local lt = land_type(cx, cy)
  if lt == "greenhouse" then
    -- 温室田（docs/33 §4.1.6）：接受任何土地类型的作物；未建地热药圃时
    -- 仍受季节限制（当季作物可种），建 1 级后四季可种（跨季不再 wrong_season）
    if greenhouse_level() < 1 and not season_allowed(crop, calendar.current()) then
      state.last_action = "wrong_season"
      return false
    end
  else
    if not season_allowed(crop, calendar.current()) then
      state.last_action = "wrong_season"
      return false
    end
    if crop.land ~= lt then
      state.last_action = "wrong_land"
      return false
    end
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
  mastery.add_xp("farming", 5) -- 熟练度（0.9）：播种计务农 XP
  state.last_action = "plant_" .. seed_id
  audio.sfx("sfx_plant")
  return true
end

-- 收获品质判定（docs/33 §4.1.2，2026-10-05 0.4）：普通/良好/上品 70/25/5；
-- 草药肥（effect=quality）保底良好——普通档并入良好（良好 95% / 上品 5%）。
-- 熟练度（0.9）：品质概率随务农等级成长（良好 +2L、上品 +0.5L），药性专精
-- 额外 +10%/+5%；基础 L=0 时与 0.4 固定值完全一致。
-- 返回 "normal"/"good"/"super"。
local function quality_for(p)
  local bonus = mastery.farming_bonus()
  if fertilizer_effect(p) == "quality" then
    local super_chance = bonus.herb_quality and 0.10 or 0.05
    return math.random() < super_chance and "super" or "good"
  end
  local roll = math.random()
  local good = bonus.good / 100
  local super = bonus.super / 100
  if roll < (1 - good - super) then
    return "normal"
  elseif roll < (1 - super) then
    return "good"
  end
  return "super"
end

-- 由地块 key（"cx,cy"）解析坐标；解析失败返回 nil。
local function parse_key(key)
  local cx, cy = tostring(key):match("^(-?%d+),(-?%d+)$")
  if not cx then
    return nil, nil
  end
  return tonumber(cx), tonumber(cy)
end

-- 洒水器覆盖判定（docs/33 §4.1.4）：shape = cross（十字 4 邻）/ square（方阵），
-- radius = 1（3×3）/ 2（5×5）。覆盖范围含自身所在格（自身无作物时无效果）。
local function sprinkler_covers(sprinkler_id, cx, cy, tx, ty)
  local def = require("data.items")[sprinkler_id]
  local spec = def and def.sprinkler or nil
  if not spec then
    return false
  end
  local dx, dy = math.abs(cx - tx), math.abs(cy - ty)
  local radius = spec.radius or 1
  if dx > radius or dy > radius then
    return false
  end
  if spec.shape == "cross" then
    -- 十字：只浇上下左右（曼哈顿距离 1），不浇斜角
    return (dx + dy) == 1
  end
  return true -- square
end

-- 清晨自动浇水（docs/33 §4.1.4）：对每个洒水器格，把覆盖范围内已种且未浇的
-- 地块标为当日已浇（等效当日手动浇水，次日结算 +1 成长）。雨天不失效。
-- 返回是否有地块被浇（供 settle_day 落盘/重绘判断）。
local function sprinkler_water()
  local watered = false
  for key, p in pairs(plots) do
    if p.sprinkler then
      local cx, cy = parse_key(key)
      if cx then
        for qkey, q in pairs(plots) do
          if q.crop and not q.watered_today then
            local qx, qy = parse_key(qkey)
            if qx and sprinkler_covers(p.sprinkler, cx, cy, qx, qy) then
              q.watered_today = true
              watered = true
            end
          end
        end
      end
    end
  end
  return watered
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
    -- 果树（docs/33 §4.1.5）：不浇水/不施肥/无品质档；成树后每年秋季每天
    -- 结 1 果（当日已采提示）；跨年保留（年年结果）。tree_days 由 settle_day
    -- 每日 +1（浇水无关），成熟前/非果期/已采当天都直接返回（不会落到浇水）。
    if crop.tree then
      if (p.tree_days or 0) < (crop.tree_days or 15) then
        state.last_action = "tree_growing"
        return state.last_action
      end
      if calendar.current() ~= (crop.fruit_season or "autumn") then
        state.last_action = "tree_not_fruiting"
        return state.last_action
      end
      if p.fruited_today then
        state.last_action = "tree_picked"
        return state.last_action
      end
      if not spend_energy(ENERGY_COST.harvest) then
        return state.last_action
      end
      inv.add(state.inventory, crop.harvest_item, crop.yield or 1)
      p.fruited_today = true
      add_labor(2)
      mastery.add_xp("farming", 5) -- 熟练度（0.9）：树收果计务农 XP
      plot_put(cx, cy, p)
      state.last_action = "harvest_" .. crop.harvest_item
      audio.sfx("sfx_harvest")
      redraw_all()
      -- 果树同样给抖动 + 闪光：一天一颗果，没有反馈就等于"没采到"。
      start_harvest_anim(cx, cy, p.crop, crop_for(p))
      effects.spawn("harvest_sparkle", (cx + 0.5) * TILE, (cy + 0.5) * TILE)
      return state.last_action
    end
    -- 成熟 → 收获（再生作物保留地块进入下一茬，非再生作物清空地块）
    if p.watered_days >= growth_need(p, crop) then
      if not spend_energy(ENERGY_COST.harvest) then
        return state.last_action
      end
      -- 动画要从"成熟那一格"起手，所以必须在改地块数据之前取到作物与瓦值
      -- （收获会把 p.crop 清空，事后摇不动）。
      local harvest_crop = p.crop
      local mature_entry = crop_for(p)
      local yield_count = crop.yield or 1
      -- 熟练度专精（0.9）：精耕 +10%、富饶 +5%（floor 取整，先于灵肥/温室叠加）
      local yield_pct = mastery.farming_bonus().yield_pct
      if yield_pct > 0 then
        yield_count = yield_count + math.floor((crop.yield or 1) * yield_pct / 100)
      end
      -- 温室 2 级（药圃深耕，docs/33 §4.1.6）：温室田作物收获 +1（先于灵肥叠加）
      if land_type(cx, cy) == "greenhouse" and greenhouse_level() >= 2 then
        yield_count = yield_count + 1
      end
      -- 灵肥（effect=yield，docs/33 §4.1.3）：每茬产量 +floor(yield × 0.5)
      if fertilizer_effect(p) == "yield" then
        yield_count = yield_count + math.floor((crop.yield or 1) * 0.5)
      end
      -- 品质（docs/33 §4.1.2）：普通=基础条目；良好/上品=派生条目
      -- （<id>_good / <id>_super，不进商店，上品留作专属用途）。
      -- last_action 保持基础收获名，品质体现在背包条目（HUD/既有断言不漂移）。
      local quality = quality_for(p)
      local harvest_item = crop.harvest_item
      if quality == "good" then
        harvest_item = crop.harvest_item .. "_good"
      elseif quality == "super" then
        harvest_item = crop.harvest_item .. "_super"
      end
      inv.add(state.inventory, harvest_item, yield_count)
      if crop.regrow_days then
        -- 再生作物：地块保留作物与肥料（多茬同享），进入再生倒计时（cycles +1）
        p.cycles = (p.cycles or 0) + 1
        p.watered_days = 0
        p.watered_today = false
      else
        p.crop = nil
        p.watered_days = 0
        p.watered_today = false
        -- 非再生作物收获清空地块 ⇒ 肥料随之消失（docs/33 §4.1.3）
        p.fertilizer = nil
      end
      add_labor(3)
      mastery.add_xp("farming", 5) -- 熟练度（0.9）：收获计务农 XP
      plot_put(cx, cy, p)
      state.last_action = "harvest_" .. crop.harvest_item
      audio.sfx("sfx_harvest")
      redraw_all()
      -- redraw_all 已把这一格画成"收完的样子"，动画随后覆盖它：先抖四格再落回真值。
      -- 再生作物落回的是"退回苗期"的作物，非再生落回空土——都不需要额外美术状态。
      start_harvest_anim(cx, cy, harvest_crop, mature_entry)
      effects.spawn("harvest_sparkle", (cx + 0.5) * TILE, (cy + 0.5) * TILE)
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
    local equipped = inv.equip_list(state.inventory) or {}
    if not spend_energy(tier_cost("water", equipped.tool)) then
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

  -- 已垦（含浇过水）→ 拆除洒水器（优先）> 施肥 > 放置洒水器 > 播种
  if p then
    -- 洒水器格：E 拆除（收回背包，地块回到已垦空地）
    if p.sprinkler then
      inv.add(state.inventory, p.sprinkler, 1)
      p.sprinkler = nil
      add_labor(3)
      plot_put(cx, cy, p)
      state.last_action = "remove_sprinkler"
      audio.sfx("sfx_hoe")
      redraw_all()
      return state.last_action
    end
    if state.selected_fertilizer and not p.fertilizer then
      -- 消耗 1 个肥料（docs/33 §4.1.3：买肥→用肥，经济闭环）
      if inv.count(state.inventory, state.selected_fertilizer) <= 0 then
        state.last_action = "no_fertilizer"
        return state.last_action
      end
      if not spend_energy(ENERGY_COST.fertilize) then
        return state.last_action
      end
      inv.remove(state.inventory, state.selected_fertilizer, 1)
      p.fertilizer = state.selected_fertilizer
      add_labor(5)
      plot_put(cx, cy, p)
      state.last_action = "fertilize_" .. state.selected_fertilizer
      audio.sfx("sfx_plant")
      redraw_all()
      return state.last_action
    end
    -- 放置洒水器（选中且背包有货，docs/33 §4.1.4）：占格，不可播种/施肥
    if state.selected_sprinkler then
      if inv.count(state.inventory, state.selected_sprinkler) <= 0 then
        state.last_action = "no_sprinkler"
        return state.last_action
      end
      inv.remove(state.inventory, state.selected_sprinkler, 1)
      p.sprinkler = state.selected_sprinkler
      state.selected_sprinkler = nil -- 放置即消耗，清选中避免后续误判放置分支
      add_labor(5)
      plot_put(cx, cy, p)
      state.last_action = "place_" .. p.sprinkler
      audio.sfx("sfx_plant")
      redraw_all()
      return state.last_action
    end
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
  local equipped = inv.equip_list(state.inventory) or {}
  if not spend_energy(tier_cost("till", equipped.tool)) then
    return state.last_action
  end
  p = { crop = nil, watered_days = 0, watered_today = false }
  add_labor(15)
  mastery.add_xp("farming", 5) -- 熟练度（0.9）：翻土计务农 XP
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
    local crop = p.crop and crops[p.crop] or nil
    if crop and crop.tree then
      -- 果树（docs/33 §4.1.5）：每日自然成长（不依赖浇水），重置当日已采标记
      p.tree_days = (p.tree_days or 0) + 1
      p.fruited_today = false
      changed = true
    elseif p.crop and p.watered_today then
      p.watered_days = p.watered_days + 1
      changed = true
    end
    if p.watered_today then
      reset_water = true
    end
    p.watered_today = false
  end
  -- 洒水器（docs/33 §4.1.4，0.5）：新一天清晨自动浇水（等效当日手动浇水；
  -- 雨天免费浇水语义保留——洒水器不因天气失效）。浇水改变了 watered_today，
  -- 必须一并落盘/重绘，否则跨场景与重进会丢"已浇"标记。
  if sprinkler_water() then
    reset_water = true
  end
  if changed or reset_water then
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
  harvest_anim = {} -- 换图/重进场景：上一张图里没放完的抖动直接丢（旧 CropGrid 实体已回收）
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
  crop_id = entity.find("CropGrid")
  local g = ground_id and tilemap.get(ground_id)
  local f = farm_id and tilemap.get(farm_id)
  local w = (g and g.width) or (f and f.width) or 0
  local h = (g and g.height) or (f and f.height) or 0
  if w == 0 or h == 0 then
    log.warn("[farm] 未找到 GroundMap/FarmGrid，农田功能不可用")
    width, height = 0, 0
    farm_id = nil
    moist_id = nil
    crop_id = nil
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
          watered_today = data.watered_today == true,
          cycles = tonumber(data.cycles) or 0,
          fertilizer = type(data.fertilizer) == "string" and data.fertilizer or nil,
          sprinkler = type(data.sprinkler) == "string" and data.sprinkler or nil,
          tree_days = tonumber(data.tree_days) or 0,
          fruited_today = data.fruited_today == true
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
      watered_today = p.watered_today == true,
      cycles = p.cycles or 0,
      fertilizer = p.fertilizer,
      sprinkler = p.sprinkler,
      tree_days = p.tree_days,
      fruited_today = p.fruited_today == true
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
          watered_today = p.watered_today == true,
          cycles = tonumber(p.cycles) or 0,
          fertilizer = type(p.fertilizer) == "string" and p.fertilizer or nil,
          sprinkler = type(p.sprinkler) == "string" and p.sprinkler or nil,
          tree_days = tonumber(p.tree_days) or 0,
          fruited_today = p.fruited_today == true
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
  harvest_anim = {} -- 旧局的抖动不能继续往新图的格子上写
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
      if crop and p.watered_days >= growth_need(p, crop) then
        mature = mature + 1
      end
    end
  end
  return { planted = planted, mature = mature, tilled = total - planted }
end

return M
