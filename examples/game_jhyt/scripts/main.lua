-- main.lua：主入口门面(main) 与 多地图（山庄 manor / 云泽村 village）分发（M17-01）。
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
--   * 跨图往返（manor <-> village）只走 on_enter：出生点 + 地图专属 init
--     （manor 才碰农田）+ exit 目标注册，不再 setup/load。
--   * 标题画面冻结时钟（set_rate(0)），永不推日/误触发 on_day_start。
local state = require("core.state")
local player = require("player.player")
local inventory = require("player.inventory")
local input = require("ui.input")
local save_manager = require("core.save_manager")
local farm = require("systems.farm")
local calendar = require("world.calendar")
local weather = require("world.weather")
local maps = require("data.maps")
local map_manager = require("world.map_manager")
local npcs = require("data.npcs")
local characters = require("data.characters")
local places = require("data.places")
local schedule = require("world.schedule")
local npc = require("world.npc")
local quest = require("world.quest")
local dialogue = require("world.dialogue")
local shop = require("systems.shop")
-- 战斗（M18-01）：combat 必须先于 enemy 加载——enemy.lua 在加载期向 combat
-- 注册目标协议（is_enemy/hit/reset），避免循环依赖。
local combat = require("systems.combat")
local enemy = require("systems.enemy")
-- 属性与关系/声望（M18-04）：stats 三层属性模型、relationship 关系/声望运行时、
-- relationships_data 纯数据注册表（validate_content 校验入口）。
local stats = require("player.stats")
local relationship = require("player.relationship")
local relationships_data = require("data.relationships")
local skills_data = require("data.skills")
local enemies_data = require("data.enemies")
-- 建筑与制作（M18-02）：building 占用/放置/拆除/升级、crafting 制作设施、
-- buildings_data/recipes_data 纯数据注册表（validate_content 校验入口）。
local building = require("systems.building")
local crafting = require("systems.crafting")
local buildings_data = require("data.buildings")
local recipes_data = require("data.recipes")
-- UI/音频/引导（M19-02）：audio 负责选曲与音效，hud 画状态面板，
-- tutorial 推进新手引导，三个面板模块在切图/新档时复位。
local audio = require("core.audio")
local hud = require("ui.hud")
local tutorial = require("ui.tutorial")
local tutorial_data = require("data.tutorial")
local inventory_ui = require("ui.inventory_ui")
local quest_ui = require("ui.quest_ui")
local pause_menu = require("ui.pause_menu")
-- M20-09：门面（title）菜单的手柄/键盘导航。场景按钮本身只有鼠标点击通路
-- （UiEvents event=click），这里补上焦点环 + 确认，并把确认转交给同一个 on_click。
local title_menu = require("ui.title_menu")

-- "new" | "continue"：标题按钮点击写入，首次进入玩法地图的 on_create 消费。
local entry
-- 一次性引导标志：inventory.setup/providers 注册/new-or-continue 处理只跑一次；
-- 场景切换保留模块局部变量，跨地图往返依赖它避免重复建背包/重复读档。
local booted = false

local function day_label()
  return "jianghu_youtian_day_" .. tostring(clock.now().day)
end

-- 已注册的玩法地图（门面 main 未注册 → 走门面分支）；scene.reload() 为 nil 时安全。
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
    if current == "manor" then
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
  -- 战斗账本：玩家气血/战败态随存档恢复（引擎 health 表是内存态不持久化）。
  save_manager.register("combat", combat.capture, combat.restore)
  -- 敌人账本：hp/存活/位置随存档恢复（M18-01）。
  save_manager.register("enemy", enemy.capture, enemy.restore)
  -- 临时 Buff 账本（M18-04）：只存 buffs（actor→数组），基础/装备层是派生态；
  -- 旧档缺 provider 数据时 restore 被跳过/传缺字段表都得到默认（清空）。
  save_manager.register("stats", stats.capture, stats.restore)
  -- 关系/声望/幂等奖励锚点（M18-04）：分数随存档恢复并重新 clamp 到范围。
  save_manager.register("relationship", relationship.capture, relationship.restore)
  -- 建筑占用/等级（M18-02）：map.state("buildings") 的事实来源 + 运行期镜像；
  -- restore 只写内存与地图快照，视觉由 building.on_enter 重放（实体判活）。
  save_manager.register("building", building.capture, building.restore)
  -- 制作任务（M18-02）：进行中任务随存档恢复，剩余时间由 clock 重新推导。
  save_manager.register("crafting", crafting.capture, crafting.restore)
  -- 音频（M19-02）：主音量随存档恢复（BGM 曲目由切图时重新选曲，不入档）。
  audio.register_save()
end

-- 加载期校验：委托 scripts/validate_data.lua（M20-01 单一事实来源）——启动、
-- 发布前检查（game/editor --check）与编辑器资源扫描共用同一份校验器清单与
-- 诊断文本（含文件/对象/字段路径），避免多处实现漂移。
local function validate_content()
  local report = require("validate_data").check({ strict = true })
  if report ~= "" then
    error("domain data validation failed:\n" .. report)
  end
end

local function new_game()
  state.reset()
  quest.reset()
  dialogue.reset()
  shop.reset()
  combat.reset()
  enemy.reset()
  -- 属性 Buff 与关系/声望清零（M18-04）：新档默认值。
  stats.reset()
  relationship.reset()
  -- 建筑占用与制作任务清零（M18-02）。
  building.reset()
  crafting.reset()
  -- UI/音频/引导（M19-02）：面板与 HUD 清空、BGM 允许重起、引导从第 0 步开始。
  hud.reset()
  inventory_ui.reset()
  quest_ui.reset()
  pause_menu.reset()
  audio.reset()
  tutorial.reset()
  audio.init()
  clock.set(1, 0)
end

-- 地图专属初始化：把玩家放到本图出生点、恢复地图 overlay（manor 的农田棋盘），
-- 并注册本图出口（map_manager 幂等）。village 没有 FarmGrid，不碰农田逻辑。
local function enter_playable_map(map_id)
  map_manager.on_enter(map_id)
  if map_id == "manor" then
    farm.init()
  end
  -- 建筑（M18-02）：重放本图占用/等级 → 重生视觉与阻挡；crafting 装载本图
  -- 制作任务并注册设施交互目标（幂等）。必须在 farm.init 之后——占用判定
  -- 依赖农田地块明细（farm.is_plot）。
  building.set_job_guard(function(key)
    return crafting.has_job(key)
  end)
  building.on_enter(map_id)
  crafting.on_enter(map_id)
  crafting.ensure_target()
  map_manager.ensure_exits(map_id)
  calendar.sync()
  weather.current()
  -- NPC 日程：绑定本图 GroundMap 并按当前时间/天气落位（须在 weather.current 之后）。
  npc.on_enter(map_id)
  -- 敌人：绑定本图 GroundMap 并生成/落位（敌人只在演武场灰盒注册，M18-01）。
  enemy.on_enter(map_id)
  -- 对话交互目标（幂等）：与 NPC 的 E 交互；切图丢弃未结束的对话并复位模式。
  dialogue.ensure_target()
  dialogue.reset()
  -- 商店交互目标（幂等，priority 20 高于对话）；切图关闭未结束的面板，
  -- 但保留当日限购计数（切图不算新的一天）。
  shop.ensure_target()
  shop.close_ui()
  -- UI 面板（M19-02）：切图清掉背包/任务/暂停面板（模式复位见下）。
  inventory_ui.reset()
  quest_ui.reset()
  pause_menu.reset()
  hud.reset()
  state.set_mode("Playing")
  -- 音频（M19-02）：按地图选曲；同一曲目跨图不重启。
  audio.play_bgm(map_id)
  -- 新手引导（M19-02）：进入地图事件（goto_village 步骤）。
  tutorial.on_enter(map_id)
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
  -- M20-02：把 <project>/settings.toml 的 `[audio]` 段应用到音量总线（主/BGM/
  -- 音效/界面）。文件可选，缺失时保持引擎默认；失败只记日志不中断启动。
  -- 游戏内改动由暂停菜单「设置」页显式写回（`audio.save_settings()`）。
  local settings_ok, settings_detail = audio.load_settings()
  log.info(
    "[audio] settings.toml: " .. tostring(settings_ok) .. " (" .. tostring(settings_detail) .. ")"
  )
  -- M20-08：把 <project>/settings.toml 的 `[input]` 段（玩家改键）应用到命名动作
  -- 映射。语义同 `audio.load_settings`：文件缺失/缺 `[input]` 段 → 保留当前绑定
  -- （即引擎默认，键盘行为与迁移前逐键一致）；版本/绑定非法 → 回退默认 + 诊断。
  local bindings_ok, bindings_detail = input.load_settings()
  log.info(
    "[input] settings.toml: " .. tostring(bindings_ok) .. " (" .. tostring(bindings_detail) .. ")"
  )
  -- M20-09：同一份 `[input]` 段里的菜单导航手感（摇杆阈值 + 按住重复节奏）也在
  -- 启动应用一次；缺文件/缺键/脏值由引擎钳制后回落默认（阈值 0.5 / 16 帧 / 8 帧）。
  local nav = input.apply_nav_settings()
  log.info(
    "[input] nav: threshold=" .. tostring(nav.threshold)
      .. " delay=" .. tostring(nav.delay_frames)
      .. " repeat=" .. tostring(nav.repeat_frames)
  )
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
  -- M20-09：门面（title）也要用玩家改键与导航手感——原先这两项只在 boot_game
  -- （进入玩法地图）时应用，导致标题菜单仍走默认绑定。这里幂等补一次：
  -- 文件缺失/非法时 `load_settings` 返回 false 且保留当前绑定，不打扰玩家。
  local bindings_ok, bindings_detail = input.load_settings()
  log.info(
    "[input] settings.toml: " .. tostring(bindings_ok) .. " (" .. tostring(bindings_detail) .. ")"
  )
  input.apply_nav_settings()
  -- 标题菜单（手柄/键盘导航）：复位到第一项并重建焦点环。
  title_menu.reset()
  clock.set_rate(0) -- 门面（含首次 execute_project_script 时 scene=nil）冻结时钟
  -- 标题 BGM（M19-02）：同一曲目重复 on_create 不重启。
  audio.play_title()
  log.info("主入口门面就绪：" .. tostring(current))
  return "jianghu_youtian_ready"
end

-- ---------- UI 钩子（M19-02） ----------
-- 暂停菜单的「读取进度 / 返回标题」需要重放本图 overlay 或复位一次性引导标志，
-- 这些状态是 main.lua 的模块局部量，故以全局函数暴露给 pause_menu.lua
-- （缺失时 pause_menu 会降级，不影响运行）。

-- 读档后重放当前地图（农田 overlay、建筑实体、NPC 落位）。
function jianghu_reload_map()
  local current = map.current() or scene.reload()
  if not is_playable(current) then
    return false
  end
  enter_playable_map(current)
  return true
end

-- 返回主入口门面：复位一次性引导标志，使下次从门面进入时重新走 boot_game
-- （新档/继续档判定），而不是沿用上一局运行态。
-- 注意：工程早期有一个独立的开始画面场景，现已被删除，门面固定是 project.toml 里
-- startup_scene 指向的那张（scenes/main.scene），所以这里直接加载 main。
-- 不要在注释里写已删除的场景路径：编辑器的依赖扫描会把文本里的 *.scene 当成依赖，
-- 于是删掉文件后会报"断链 依赖 ... 缺失"。
function jianghu_return_to_title()
  entry = nil
  booted = false
  state.set_mode("Playing")
  return scene.load("main")
end

-- 门面/存档自检用只读信息（headless 验收）。
function jianghu_status()
  return {
    scene = scene.reload(),
    map = map.current(),
    entry = entry,
    booted = booted,
    tutorial_steps = #(tutorial_data.steps or {}),
    bgm = audio.current_bgm(),
    volume = audio.volume(),
    -- M20-02：音量分组与 settings.toml 路径（只读；暂停设置页写入）。
    buses = {
      master = audio.bus_volume("master"),
      bgm = audio.bus_volume("bgm"),
      sfx = audio.bus_volume("sfx"),
      ui = audio.bus_volume("ui")
    },
    settings_path = audio.settings_path()
  }
end

-- ===========================================================================
-- M21 主角形象系统：给 UI / 控制台 / 验收驱动的公开入口
-- 形象本体与序列帧在 data/characters.lua，清单在 data/appearances.lua，
-- 运行期挂载在 player.lua；这里只做"转发 + 状态汇总"。
-- ===========================================================================

-- 切换主角形象（青衫少侠 / 青衣女侠 / 将来的自定义）。
-- 返回 ok, err；成功时立刻重挂单帧 + 序列帧，并写进 state.appearance（随存档持久化）。
function jianghu_set_appearance(id)
  return player.set_appearance(id)
end

-- 主角形象现状（UI 显示与 headless 断言用）。
function jianghu_appearance()
  local status = player.status()
  status.options = player.appearance_options()
  return status
end

function on_click(name)
  if scene.reload() ~= "main" then
    return
  end
  if name == "BtnStart" then
    entry = "new"
    state.set_mode("Transition")
    scene.load("manor")
  elseif name == "BtnContinue" then
    entry = "continue"
    state.set_mode("Transition")
    -- M17-01：读档回到存档所在的地图（peek_map 只读盘不恢复），而非固定山庄；
    -- 无有效存档/坏档时 peek_map 返回 nil → 降级到山庄，boot 内 load 会再兜底。
    local target = save_manager.peek_map()
    scene.load(maps.get(target) and target or "manor")
  elseif name == "BtnExit" then
    app.quit()
  end
end

function update()
  if is_playable(scene.reload()) then
    npc.tick()
    -- 战斗固定步推进（帧号/判定窗）→ 敌人 AI → 玩家输入。
    -- 攻击在注册后第 windup 步开始判定，下一 tick 起算，顺序确定。
    combat.tick()
    enemy.tick()
    local label = input.update()
    -- 主角形象：input.update 决定了朝向与"是否在走"，之后再切 walk/idle
    -- （顺序不能反，否则动画永远慢一帧）。切图后实体会重建，tick 里会重挂。
    player.tick()
    return label
  end
  -- M20-09：门面（标题界面）的手柄/键盘菜单——确认与鼠标点击同走 `on_click`，
  -- 因此「开始新游戏 / 继续江湖 / 退出江湖」三条分支行为完全一致。
  title_menu.render()
  local activated = title_menu.update()
  if activated then
    on_click(activated)
  end
  return day_label()
end

-- 引擎约定：day.sleep() 触发 on_day_end（本次存档）→ 到清晨 → on_day_start。
-- on_day_start 在**当前所在地图**上执行：只有山庄才做农田日结算（village 无
-- FarmGrid），新日期/天气/体力重置与存档对所有玩法地图生效。
-- 主入口门面冻结时钟不会走到这里。
function on_day_start()
  local current = scene.reload()
  if not is_playable(current) then
    return
  end
  if current == "manor" then
    farm.settle_day()
  end
  calendar.sync()
  weather.current()
  state.energy = 100
  state.labor_minutes = 0
  -- 临时 Buff 按游戏日过期（M18-04）：days 递减，归 0 移除。
  stats.on_day_start()
  -- 新的一天：商店限购刷新（当日已购计数清零）。
  shop.on_day_start()
  -- 采集点刷新（M19-01）：清掉已到刷新期的记录并落盘；未接线时 query/act 的
  -- 惰性刷新给出同样结果，这里只是让存档在新的一天立刻干净。
  pcall(function()
    require("systems.gather").on_day_start()
  end)
  -- 新手引导（M19-02）：睡觉步骤在跨天事件里推进。
  tutorial.on_day_start()
  log.info("新的一天：" .. tostring(clock.now().day) .. " @" .. tostring(current))
  save_manager.save()
end

return state
