-- main.lua：开始画面(title) 与 多地图（山庄 main / 云泽村 village）分发（M17-01）。
--
-- 引擎事实（core/script.rs）：
--   * 场景切换保留 Lua VM：require 缓存与模块局部变量跨场景存活；
--     process_scene_transition 每次切场景后都会重跑全局 on_create。
--   * save_data（含 __map / __mapstate:* 键）跨场景保留；场景内 ECS
--     Tilemap/实体在切换时重建，地图专属 overlay 必须在 on_create 重绘。
--   * inv.create 会无条件重建背包（清空内容）→ 一次性引导只允许发生一次，
--     否则跨地图往返会清空物品；跨图 on_create 不得重跑 load（会丢当日进度）。
--
-- 生命周期：
--   * 标题 on_click 记录 entry（new/continue）并 scene.load 到目标地图；
--     continue 先 save_manager.peek_map() 读存档地图再切换（而非固定山庄）。
--   * 首次进入玩法地图的 on_create 做一次性引导（inventory.setup +
--     providers 注册 + 新档/继续档处理），随后 map_manager 放置玩家到出生点
--     —— 继续档的 restore 在出生点放置之后执行，才以存档坐标覆盖。
--   * 跨图往返（main <-> village）只走 on_enter：出生点 + 地图专属 init
--     （main 才碰农田）+ exit 目标注册，不再 setup/load。
--   * 标题画面冻结时钟（set_rate(0)），永不推日/误触发 on_day_start。
local state = require("state")
local player = require("player")
local inventory = require("inventory")
local input = require("input")
local save_manager = require("save_manager")
local farm = require("farm")
local calendar = require("calendar")
local weather = require("weather")
local maps = require("data.maps")
local map_manager = require("map_manager")
local npcs = require("data.npcs")
local places = require("data.places")
local schedule = require("schedule")
local npc = require("npc")
local quest = require("quest")
local dialogue = require("dialogue")
local shop = require("shop")

-- "new" | "continue"：标题按钮点击写入，首次进入玩法地图的 on_create 消费。
local entry
-- 一次性引导标志：inventory.setup/providers 注册/new-or-continue 处理只跑一次；
-- 场景切换保留模块局部变量，跨地图往返依赖它避免重复建背包/重复读档。
local booted = false

local function day_label()
  return "jianghu_youtian_day_" .. tostring(clock.now().day)
end

-- 已注册的玩法地图（title 未注册 → 走标题分支）；scene.reload() 为 nil 时安全。
local function is_playable(scene_id)
  return type(scene_id) == "string" and maps.get(scene_id) ~= nil
end

-- B-2 注册中心：读档时按注册表整体恢复（scene 记录的 map 跟随 map.current()）。
local function register_save_providers()
  save_manager.register("state", state.capture, state.restore)
  save_manager.register("inventory", inventory.capture, inventory.restore)
  save_manager.register("clock", function()
    local now = clock.now()
    return { day = now.day, minute = now.minute }
  end, function(data)
    if type(data) == "table" then
      clock.set(tonumber(data.day) or 1, tonumber(data.minute) or 0)
    end
  end)
  save_manager.register("player", player.capture, player.restore)
  -- scene 摘要：记录所在地图（map.current() 优先于 scene.reload()）与主地图
  -- 农田 map_state。读档时 restore 校验场景匹配后，把农田键写回 map.state；
  -- __mapstate:* 键随 save.commit 全量落盘，跨图 continue 据此回到正确地图。
  save_manager.register("scene", function()
    local current = map.current() or scene.reload()
    local farm_snapshot = farm.capture()
    local map_state = {}
    if current == "main" then
      map_state.farm_plots = farm_snapshot.plots
    end
    return {
      name = scene.reload(),
      map = current,
      map_state = map_state
    }
  end, function(data)
    if type(data) == "table" and type(data.map_state) == "table" and type(data.map_state.farm_plots) == "table" then
      map.state("farm_plots", data.map_state.farm_plots)
    end
  end)
  -- 农田地块明细：farm.init 在场景重建后从 map.state 还原，capture/restore
  -- 只保证 save blob 与 map.state 同时落盘/取回，自身不重复持有逻辑。
  save_manager.register("farm", farm.capture, farm.restore)
  -- NPC 日程状态：读档后位置/活动与本图实体一致（M17-02 验收项④）。
  save_manager.register("npc", npc.capture, npc.restore)
  -- 任务状态机记录：落档态（in_progress/completed/failed）随存档恢复；
  -- deliverable 是派生态不落档，读档后由目标进度重新推导（M17-03 验收项③④）。
  save_manager.register("quest", quest.capture, quest.restore)
  -- 商店经济：当日限购计数随存档恢复（M17-04 验收项③：交易可存档恢复）。
  save_manager.register("shop", shop.capture, shop.restore)
end

-- 加载期校验：所有领域数据在进入玩法前统一校验。错误包含文件、对象 ID 和字段路径。
local function validate_content()
  local validators = {
    { "maps", maps.validate_all },
    { "places", places.validate_all },
    { "npcs", npcs.validate_all },
    { "schedule", schedule.validate_all },
    { "quest", quest.validate_all },
    { "dialogue", dialogue.validate_all },
    { "shop", shop.validate_all }
  }
  local errors = {}
  for _, entry in ipairs(validators) do
    local ok, issues = entry[2]()
    if not ok then
      for _, issue in ipairs(issues) do
        errors[#errors + 1] = "[" .. entry[1] .. "] " .. issue
      end
    end
  end
  if #errors > 0 then
    error("domain data validation failed:\n" .. table.concat(errors, "\n"))
  end
end

local function new_game()
  state.reset()
  quest.reset()
  dialogue.reset()
  shop.reset()
  clock.set(1, 0)
end

-- 地图专属初始化：把玩家放到本图出生点、恢复地图 overlay（main 的农田棋盘），
-- 并注册本图出口（map_manager 幂等）。village 没有 FarmGrid，不碰农田逻辑。
local function enter_playable_map(map_id)
  map_manager.on_enter(map_id)
  if map_id == "main" then
    farm.init()
  end
  map_manager.ensure_exits(map_id)
  calendar.sync()
  weather.current()
  -- NPC 日程：绑定本图 GroundMap 并按当前时间/天气落位（须在 weather.current 之后）。
  npc.on_enter(map_id)
  -- 对话交互目标（幂等）：与 NPC 的 E 交互；切图丢弃未结束的对话并复位模式。
  dialogue.ensure_target()
  dialogue.reset()
  -- 商店交互目标（幂等，priority 20 高于对话）；切图关闭未结束的面板，
  -- 但保留当日限购计数（切图不算新的一天）。
  shop.ensure_target()
  shop.close_ui()
  state.set_mode("Playing")
  clock.set_rate(2)
end

-- 一次性引导：只在首次进入玩法地图（new 或 continue 的目标场景）时执行一次。
-- continue 先由 map_manager 放置出生点，再 save_manager.load() 以存档坐标/状态
-- 覆盖；读档失败优雅降级为新档。
local function boot_game(map_id)
  if booted then
    return
  end
  inventory.setup()
  register_save_providers()
  validate_content()
  if entry == "continue" then
    enter_playable_map(map_id)
    if not save_manager.load() then
      log.warn("[save] 无有效存档，按新档开始")
      entry = "new"
      new_game()
      enter_playable_map(map_id)
    end
  else
    new_game()
    enter_playable_map(map_id)
  end
  booted = true
end

function on_create()
  local current = scene.reload()
  if is_playable(current) then
    if not booted then
      boot_game(current)
    else
      -- 跨图往返：不重建背包/不读档，只重放本图出生点与 overlay。
      enter_playable_map(current)
    end
    log.info(
      "进入 " .. tostring(current) .. "：" .. tostring(maps.get(current).display)
        .. " 第 " .. tostring(clock.now().day) .. " 天 (mode=" .. tostring(entry) .. ")"
    )
    return current .. "_ready"
  end
  clock.set_rate(0) -- 标题（含首次 execute_project_script 时 scene=nil）冻结时钟
  log.info("标题画面就绪：" .. tostring(current))
  return "jianghu_youtian_ready"
end

function on_click(name)
  if scene.reload() ~= "title" then
    return
  end
  if name == "BtnStart" then
    entry = "new"
    state.set_mode("Transition")
    scene.load("main")
  elseif name == "BtnContinue" then
    entry = "continue"
    state.set_mode("Transition")
    -- M17-01：读档回到存档所在的地图（peek_map 只读盘不恢复），而非固定山庄；
    -- 无有效存档/坏档时 peek_map 返回 nil → 降级到山庄，boot 内 load 会再兜底。
    local target = save_manager.peek_map()
    scene.load(maps.get(target) and target or "main")
  elseif name == "BtnExit" then
    app.quit()
  end
end

function update()
  if is_playable(scene.reload()) then
    npc.tick()
    return input.update()
  end
  return day_label()
end

-- 引擎约定：day.sleep() 触发 on_day_end（本次存档）→ 到清晨 → on_day_start。
-- on_day_start 在**当前所在地图**上执行：只有山庄才做农田日结算（village 无
-- FarmGrid），新日期/天气/体力重置与存档对所有玩法地图生效。
-- 标题冻结时钟不会走到这里。
function on_day_start()
  local current = scene.reload()
  if not is_playable(current) then
    return
  end
  if current == "main" then
    farm.settle_day()
  end
  calendar.sync()
  weather.current()
  state.energy = 100
  state.labor_minutes = 0
  -- 新的一天：商店限购刷新（当日已购计数清零）。
  shop.on_day_start()
  log.info("新的一天：" .. tostring(clock.now().day) .. " @" .. tostring(current))
  save_manager.save()
end

return state
