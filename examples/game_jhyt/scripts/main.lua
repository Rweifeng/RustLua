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
-- 季节可视化（M29 / 阶段五 P1）：按当前季节换地表贴图（四季 tileset 终于在画面上生效）。
local season_visual = require("world.season_visual")
local maps = require("data.maps")
local map_manager = require("world.map_manager")
-- M32 相机收口：2D 相机跟随玩家（玩家始终在视野正中），取景范围由场景 `GameCamera`
-- 的 `bounds` 锁在本地图矩形内（引擎每帧夹取）。见 world/camera_view.lua 头注释。
local camera_view = require("world.camera_view")
local npcs = require("data.npcs")
local characters = require("data.characters")
local places = require("data.places")
local schedule = require("world.schedule")
local npc = require("world.npc")
-- 遮挡顺序：角色（actor 层）的 Sprite order 每帧跟着脚位 Y 重写（见 world/depth.lua 头注释）。
local depth = require("world.depth")
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
-- 出货箱（docs/33 §4.1.7，0.8）：镖局收单——睡前统一结算 + 出货记录。
local shipping = require("systems.shipping")
-- 熟练度（docs/33 §7.1，0.9）：5 线 10 级 + 专精（务农/采药/制药/武学/江湖）。
local mastery = require("systems.mastery")
-- 阶段2（docs/33 §4.3–4.8、§7.3、§8.3，1.0–1.8）：畜牧/钓鱼/矿脉/料理/好感事件/
-- 任务板/图鉴/成就——各系统模块自带 capture/restore/reset/on_day_start，此处接线。
local animal = require("systems.animal")
local fishing = require("systems.fishing")
local mine = require("systems.mine")
local cooking = require("systems.cooking")
local relationship_events = require("systems.relationship_events")
local questboard = require("systems.questboard")
local collection = require("systems.collection")
local achievement = require("systems.achievement")
-- 阶段3（docs/33 §2.1/§4.10/§5.5/§7.4/§8.4/§8.5，1.9–2.4）：经脉修炼/医馆经营与疫情/
-- 黑市抉择循环/茶会社交/多结局（节气历法是 day 纯派生，无需 require 持久态模块）。
local meridian = require("systems.meridian")
local clinic = require("systems.clinic")
local black_market = require("systems.black_market")
local tea_party = require("systems.tea_party")
local endings = require("systems.endings")
local buildings_data = require("data.buildings")
local recipes_data = require("data.recipes")
-- UI/音频/引导（M19-02）：audio 负责选曲与音效，hud 画状态面板，
-- tutorial 推进新手引导，三个面板模块在切图/新档时复位。
local audio = require("core.audio")
local hud = require("ui.hud")
local tutorial = require("ui.tutorial")
local tutorial_data = require("data.tutorial")
-- 七日目标（M27）：目标表 + 运行时（日结算挂在 on_day_start，进度随 state.vars）
local goals = require("systems.goals")
local goals_data = require("data.goals")
-- 掉落与战败（M28 / 计划 G3）：loot 管"击杀掉落 + 当日待入库"（跨天入库），
-- death 管战败结算（扣未入库掉落 50% + 铜钱 5%、回山庄、状态回满）。
local loot = require("systems.loot")
local death = require("systems.death")
-- docs/48 effects 接线：主循环推进特效生命周期（固定步 60Hz → dt=1/60）。
local effects = require("systems.effects")
local drops_data = require("data.drops")
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
  -- 出货箱（docs/33 §4.1.7，0.8）：待结算清单/累计出货/当日记录随存档恢复。
  save_manager.register("shipping", shipping.capture, shipping.restore)
  -- 熟练度（docs/33 §7.1，0.9）：5 线 XP 与专精选择随存档恢复。
  save_manager.register("mastery", mastery.capture, mastery.restore)
  -- 阶段2（docs/33 §4.3–4.8、§7.3、§8.3，1.0–1.8）：畜牧/钓鱼/矿脉/料理/好感事件/
  -- 任务板/图鉴/成就各自的持久态随存档恢复。
  save_manager.register("animal", animal.capture, animal.restore)
  save_manager.register("fishing", fishing.capture, fishing.restore)
  save_manager.register("mine", mine.capture, mine.restore)
  save_manager.register("cooking", cooking.capture, cooking.restore)
  save_manager.register("relationship_events", relationship_events.capture, relationship_events.restore)
  save_manager.register("questboard", questboard.capture, questboard.restore)
  save_manager.register("collection", collection.capture, collection.restore)
  save_manager.register("achievement", achievement.capture, achievement.restore)
  -- 阶段3（docs/33 §4.10/§5.5/§7.4/§8.4/§8.5，1.9–2.4）：经脉/医馆/黑市/茶会/结局
  -- 持久态随档恢复（节气历法是 day 纯派生不入档）。
  save_manager.register("meridian", meridian.capture, meridian.restore)
  save_manager.register("clinic", clinic.capture, clinic.restore)
  save_manager.register("black_market", black_market.capture, black_market.restore)
  save_manager.register("tea_party", tea_party.capture, tea_party.restore)
  save_manager.register("endings", endings.capture, endings.restore)
  -- 音频（M19-02）：主音量随存档恢复（BGM 曲目由切图时重新选曲，不入档）。
  audio.register_save()
end

-- 加载期校验：委托 scripts/validate_data.lua（M20-01 单一事实来源）——启动、
-- 发布前检查（game/editor --check）与编辑器资源扫描共用同一份校验器清单与
-- 诊断文本（含文件/对象/字段路径），避免多处实现漂移。
-- M32 竖版：竖版版面契约（面板几何不自叠 + 设计分辨率三处一致）作为 `layout`
-- 校验器登记在 validate_data 里，因此**同一个门**就能挡住"改了几何常量把两个面板
-- 叠在一起"这类只有跑到画面上才看得见的问题。
local function validate_content()
  local report = require("validate_data").check({ strict = true })
  if report ~= "" then
    error("domain data validation failed:\n" .. report)
  end
end

local function new_game()
  -- P0：先丢弃**引擎侧**持久键。`__map` / `__map:spawn:*` / `__mapstate:*` /
  -- `__blob:*` 不属于任何 Lua provider，`state.reset()` 之类只清 Lua 表，
  -- 残留会让新档读回上一局的地图、农田、建筑、制作与采集记录
  -- （同进程内「继续 → 玩 → 返回标题 → 开始新游戏」即可复现）。
  -- 顺序固定：先清引擎键，再让各模块写自己的空快照（否则会被这次清理抹掉）。
  save.clear()
  map.state_clear()
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
  -- 农田与采集（P0 补齐）：farm 此前**没有** reset，gather.reset 已实现却从未被
  -- 调用 → 新档会继承上一局的开垦地块/作物/浇水天数与已采记录。
  farm.reset()
  require("systems.gather").reset()
  -- 背包：重新发放新档初始物品（灵芝菌种 8 / 稻种 4 / 稻米 2 / 水壶 1 + 锄头装备），
  -- 避免读档半途失败后残留上一局的物品与装备。
  inventory.setup()
  -- UI/音频/引导（M19-02）：面板与 HUD 清空、BGM 允许重起、引导从第 0 步开始。
  hud.reset()
  inventory_ui.reset()
  quest_ui.reset()
  pause_menu.reset()
  title_menu.reset()
  audio.reset()
  tutorial.reset()
  -- 七日目标（M27）：新档回到第 1 天，清空进度/结算/里程碑锚点。
  goals.reset()
  -- 掉落与战败（M28）：清空待入库袋与战败明细，随机序列回默认种子。
  loot.reset()
  death.reset()
  -- 阶段2（docs/33 §4.3–4.8、§7.3、§8.3，1.0–1.8）：新档复位各新系统持久态。
  animal.reset()
  fishing.reset()
  mine.reset()
  cooking.reset()
  relationship_events.reset()
  questboard.reset()
  collection.reset()
  achievement.reset()
  -- 阶段3（1.9–2.4）：经脉/医馆/黑市/茶会/结局新档复位（节气纯派生无状态）。
  meridian.reset()
  clinic.reset()
  black_market.reset()
  tea_party.reset()
  endings.reset()
  -- 天气与季节表现（M29）：天气抽签种子回默认、季节换图计数清零（贴图由进图时重设）。
  weather.reset()
  season_visual.reset()
  -- 相机（M32 相机收口）：清掉上一局的跟随目标，避免新档第一帧跟着已销毁的实体。
  camera_view.reset()
  audio.init()
  clock.set(1, 0)
  -- 任务板（§4.7，1.6）：day 1 播种（日常 3 单 + 季初单），与 on_day_start 同一入口，幂等。
  questboard.on_day_start()
end

-- 地图专属初始化：把玩家放到本图出生点、恢复地图 overlay（manor 的农田棋盘），
-- 并注册本图出口（map_manager 幂等）。village 没有 FarmGrid，不碰农田逻辑。
local function enter_playable_map(map_id)
  map_manager.on_enter(map_id)
  -- 相机跟随（M32 相机收口）：必须在落点之后、且每次进图都重绑——换图会重建玩家实体，
  -- 沿用旧 id 会让跟随目标指向已销毁的实体；换图后场景自带的 `Camera` 也换了 bounds，
  -- 但相机控制器与 zoom 跨场景保持（引擎语义：场景相机只是**初始**相机）。
  camera_view.follow_player()
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
  -- 季节地表（M29）：进图时按当前季节换地表贴图（未登记的地图保持基础图）。
  season_visual.refresh(map_id)
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
  -- 出货箱交互目标（docs/33 §4.1.7，0.8）：山庄门口镖局货箱；切图关闭面板。
  shipping.ensure_target()
  shipping.close_ui()
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
  -- 七日目标（M27）：enter_map 类目标的进图事件（与 tutorial 同一套接线）。
  goals.on_enter(map_id)
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
  -- M20-02：把 <project>/project.toml 的 `[audio]` 段应用到音量总线（主/BGM/
  -- 音效/界面）。文件可选，缺失时保持引擎默认；失败只记日志不中断启动。
  -- 游戏内改动由暂停菜单「设置」页显式写回（`audio.save_settings()`）。
  local settings_ok, settings_detail = audio.load_settings()
  log.info(
    "[audio] project.toml: " .. tostring(settings_ok) .. " (" .. tostring(settings_detail) .. ")"
  )
  -- M20-08：把 <project>/project.toml 的 `[input]` 段（玩家改键）应用到命名动作
  -- 映射。语义同 `audio.load_settings`：文件缺失/缺 `[input]` 段 → 保留当前绑定
  -- （即引擎默认，键盘行为与迁移前逐键一致）；版本/绑定非法 → 回退默认 + 诊断。
  local bindings_ok, bindings_detail = input.load_settings()
  log.info(
    "[input] project.toml: " .. tostring(bindings_ok) .. " (" .. tostring(bindings_detail) .. ")"
  )
  -- M20-09 / M22：同一份 `[input]` 段里的菜单导航手感（摇杆阈值 + 按住连发节奏）也在
  -- 启动应用一次，并推给引擎（引擎按毫秒执行连发，与帧率无关）；缺文件/缺键/脏值由
  -- 引擎钳制后回落默认（阈值 0.5 / 350ms / 90ms）。
  local nav = input.apply_nav_settings()
  log.info(
    "[input] nav: threshold=" .. tostring(nav.threshold)
      .. " delay_ms=" .. tostring(nav.delay_ms)
      .. " repeat_ms=" .. tostring(nav.repeat_ms)
  )
  validate_content()
  if entry == "continue" then
    enter_playable_map(map_id)
    if not save_manager.load() then
      log.warn("[save] 无有效存档，按新档开始")
      entry = "new"
      new_game()
      enter_playable_map(map_id)
    else
      -- M29 顺序修正：进图发生在读档**之前**（那时时钟还是新 VM 的第 1 天），
      -- 读档会把日期/季节与天气变量改写成存档里的值 ⇒ 必须按恢复后的日期重算
      -- 一次天气与季节地表。否则「第 30 天的档」会显示春天贴图、天气也停在第 1 天
      -- （此前季节只影响 HUD 文字，所以这个顺序问题看不出来；地表换图让它显形）。
      weather.sync()
      season_visual.refresh(map_id)
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
    "[input] project.toml: " .. tostring(bindings_ok) .. " (" .. tostring(bindings_detail) .. ")"
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
  -- 相机（M32 相机收口）：门面没有玩家实体，必须取消跟随——否则相机控制器会继续
  -- 追着一个**已销毁**的实体（引擎对死目标按 None 处理，画面会停在上一张图的坐标）。
  camera_view.reset()
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
    -- M20-02：音量分组与 project.toml 路径（只读；暂停设置页写入）。
    buses = {
      master = audio.bus_volume("master"),
      bgm = audio.bus_volume("bgm"),
      sfx = audio.bus_volume("sfx"),
      ui = audio.bus_volume("ui")
    },
    -- M32 相机收口：跟随状态与宿主回填的真实相机中心（headless 验收读它断言
    -- "玩家始终在视野正中"与"相机中心被地图 bounds 夹住"）。
    camera = camera_view.status(),
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
  -- M22：标题按钮的确认反馈音放在这里（而不是 `ui.title_menu`），因为鼠标点击走场景
  -- `UiEvents event=click` → 本函数，键盘/手柄走 `title_menu.update()` → 本函数，
  -- 三条路径共用同一个入口，音效因此完全一致。
  if name == "BtnStart" or name == "BtnContinue" or name == "BtnExit" then
    audio.ui("ui_confirm")
    -- 离开门面前收起动态 UI（高亮框/标签/提示），避免残留到下一张场景的第一帧。
    title_menu.hide()
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
    -- 特效固定步推进：播完自毁（与战斗/敌人同固定步语义）。
    effects.update(1 / 60)
    -- 收成抖动同样走固定步（M72-08）：4 帧 × 100ms，放完把那一格落回真实地块数据。
    farm.tick(1 / 60)
    local label = input.update()
    -- 主角形象：input.update 决定了朝向与"是否在走"，之后再切 walk/idle
    -- （顺序不能反，否则动画永远慢一帧）。切图后实体会重建，tick 里会重挂。
    player.tick()
    -- 位移都落定之后才排遮挡：depth.tick 逐个角色按当前 Y 重写 order（桶位没变就不写）。
    depth.tick()
    return label
  end
  -- M20-09：门面（标题界面）的手柄/键盘菜单——确认与鼠标点击同走 `on_click`，
  -- 因此「开始新游戏 / 继续江湖 / 退出江湖」三条分支行为完全一致。
  --
  -- M22 顺序修正：**先 `update()`（内部先 clear 再按输入改选中）再 `render()`**。
  -- 之前是 render → update，而 `update()` 开头的 `clear()` 会把本帧刚画的标签/提示
  -- 隐藏掉（面板类高亮此前不受 `hide` 影响，因此只表现为「文字不显示 + 旧高亮框残留」，
  -- 选中动画也就看不出来）。宿主是在 `update()` 返回后才取顶点，所以必须以此顺序结束本帧。
  local activated = title_menu.update()
  title_menu.render()
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
  -- 天气（M29）：跨天重抽一次（确定性：同档同天同结果），并据此刷新季节地表
  -- ——第 29/57/85 天季节跳变时，进图时换过的地表要跟着换。
  weather.on_day_start()
  season_visual.refresh(current)
  -- 经脉修炼（docs/33 §7.4，1.9）：体力恢复池读经脉上限（无经脉时=inventory.max_energy()=100，
  -- 与原锚点一致）；内力每日自然恢复 +30、常驻加成在旧 Buff 衰减之后重挂。
  state.energy = meridian.max_energy()
  state.labor_minutes = 0
  -- 临时 Buff 按游戏日过期（M18-04）：days 递减，归 0 移除。
  stats.on_day_start()
  meridian.on_day_start()
  -- 新的一天：商店限购刷新（当日已购计数清零）。
  shop.on_day_start()
  -- 采集点刷新（M19-01）：清掉已到刷新期的记录并落盘；未接线时 query/act 的
  -- 惰性刷新给出同样结果，这里只是让存档在新的一天立刻干净。
  local gather_ok, gather_err = pcall(function()
    require("systems.gather").on_day_start()
  end)
  if not gather_ok then
    log.warn("[main] gather.on_day_start failed: " .. tostring(gather_err))
  end
  -- 新手引导（M19-02）：睡觉步骤在跨天事件里推进。
  tutorial.on_day_start()
  -- 掉落入库（M28）：昨天/今天打到的猎物在睡前搬回仓库（"未入库"只到跨天为止），
  -- 必须在目标结算之前入账，否则"炼药/交付"类目标会看到上一帧的旧背包。
  local banked = loot.bank()
  if #banked > 0 then
    log.info("入库战利品：" .. loot.pending_text_of(banked))
  end
  -- 出货箱统一结算（docs/33 §4.1.7，0.8）：白天放入货箱的物品在睡前按 sell 价
  -- 入账并写出货记录；与掉落入库同理，必须在目标结算之前入账。
  local shipped = shipping.settle()
  if shipped and shipped.total > 0 then
    log.info("镖局收单结算：" .. tostring(shipped.total) .. " 文")
  end
  -- 七日目标（M27）：日结算 + 推进目标日（奖励与结算结果写 state.vars，随存档）。
  goals.on_day_start()
  -- 阶段2（1.0–1.8）：畜牧日产出、钓鱼临时钓点刷新、好感事件生日标注、
  -- 任务板分层换榜、成就日检查（各模块内部幂等）。
  animal.on_day_start()
  fishing.on_day_start()
  relationship_events.on_day_start()
  questboard.on_day_start()
  achievement.check_all()
  -- 阶段3（1.9–2.4）：节气纯派生重算（日志/HUD 只读）、医馆疫情状态机、
  -- 黑市窗口刷新、茶会冷却解除、多结局达标检查。
  pcall(function() require("systems.solar_terms").on_day_start() end)
  clinic.on_day_start()
  black_market.on_day_start()
  tea_party.on_day_start()
  endings.check_all()
  log.info("新的一天：" .. tostring(clock.now().day) .. " @" .. tostring(current))
  save_manager.save()
end

return state
