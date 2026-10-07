-- title_menu.lua：主入口门面（`main.scene` + M63b 起的 `assets/prefabs/title_menu.prefab`）
-- 的手柄/键盘菜单导航（M20-09）。
--
-- 背景：三个按钮（BtnStart / BtnContinue / BtnExit）只有
-- `UiEvents event=click;function=on_click` —— 也就是**只有鼠标点击**通路，
-- 手柄与键盘在标题界面完全没有反应。本模块补齐这一层：
--
--   * 焦点环（`ui.panel_nav`）持有选中项，W/S、方向键 / DPad / 左摇杆 / 数字键直选都能改
--     选中；导航手感（摇杆阈值、按住重复）复用 `ui.input.ui_step()`，与游戏内面板
--     完全一致（不另写一份，避免手感分叉）；
--   * 选中项以金色高亮框 + 左侧箭头画在按钮上（`ui.panel` + `ui.text_at`，与暂停菜单
--     同款动态 UI 做法）；**三行标签文字也由本模块每帧画**，实体层（预设）里那三份 `Text`
--     逐名隐藏——两层同时渲染会在同一位置叠出重影（M22 追加修复，见 `M.render`）；
--   * 确认（`confirm`：空格 / 手柄 A）与鼠标点击统一走场景脚本的 `on_click(name)`，
--     两条路径**同一个入口**，因此行为完全一致；
--   * 顶部显示当前设备对应的操作说明（`ui.hints`），与游戏内 HUD 同源；
--   * 取消键（键盘 Esc / 手柄 B）在门面无副作用：**只有选中「退出江湖」再确认，
--     或点窗口右上角的关闭按钮**才会结束进程（宿主侧的「Esc 关窗」已移除）。
local nav = require("ui.panel_nav")
local hints = require("ui.hints")
-- M30：文字统一走 `ui.ui_text`（裁剪 + 登记，验收遍历登记表逐条量宽）。
local ui_text = require("ui.ui_text")
-- M32 竖版：按钮矩形与设计宽度的唯一来源（`ui/layout.lua`），与预设里的按钮 `UiLayout` 对齐。
local layout = require("ui.layout")
-- docs/48 ui 换肤接线：菜单装饰组（四角/竹分隔/印章/按钮悬停态）。
local skin = require("ui.skin")
-- `ui.input` 与 main.lua 同层：模块加载期不能 require（会与 main → title_menu 成环），
-- 首次使用时惰性取一次（此时 Lua 的 require 缓存里已有它）。
local input_module = nil

local function game_input()
  if not input_module then
    input_module = require("ui.input")
  end
  return input_module
end

local M = {}

local FOCUS = "title:menu"

-- M63b：门面的**版面**（层级、锚点、矩形、贴图与九宫格边距、字号与颜色）搬进
-- `assets/prefabs/title_menu.prefab`，运行期实例化进活世界；留在脚本里的仍然是"取值"
-- 与"每帧变化的东西"：选中项、高亮框、选中箭头、hover 贴图、顶部操作说明，以及三行
-- 按钮标签的**文字本身**（今天由脚本画，场景/预设里那三份 `Text` 逐名隐藏）。
--
-- 装饰类（四角/竹分隔/印章）也**没有**搬进预设：动态层整体画在实体层**之下**
-- （`frame_data.rs`：`push_ui_panels/images/text` 之后才 `push_scene_ui`），把它们搬进去
-- 就是让它们从此盖住背景图——那是改画面，不是搬家。层序这一笔的账见 `docs/67` §2。
local PRESET_PATH = "assets/prefabs/title_menu.prefab"
local menu_handle = nil
-- 「门面激活」标志：reset（进门面）置 true、hide（离开门面）置 false。
-- M71 修复：`scene.load` 是异步排队，过渡帧里 `scene.reload()` 尚未返回目标场景，
-- `main.lua` 的 `is_playable` 判定会误走门面分支 → `render()` 的 `preset_spawn`
-- （preset_alive 因 hide 已归零而返回 false）把刚 hide 撤掉的实例**重新 spawn**，
-- 于是切进山庄后 title_menu 的 18 个实体与 HUD 重叠。render 在非激活态直接返回，
-- 过渡帧不再重铺；回到门面由 `reset()` 重新置活并铺实例。

-- 实例是否还属于**当前这个世界**。句柄非 nil 不等于实例还在：`scene.load` 会把 ECS 世界
-- 整个重建（`transition.rs`），运行期实例的实体随之全部失效，引擎那边也按同一口径把实例
-- 登记表清空（M63c），于是旧 handle 在 `prefab.root` 上是**报错**而不是返回 nil。
-- 只认 `menu_handle ~= nil` 的后果：回到门面时不再铺（整屏 UI 消失，一行日志没有），
-- 而 `M.hide()` 会拿死句柄去 `prefab.despawn`（直接抛错，M20-09c 的红的就是这一处）。
local function preset_alive()
  if not menu_handle then
    return false
  end
  local ok, alive = pcall(function() return prefab.root(menu_handle) ~= nil end)
  return ok and alive == true
end

-- 实例化门面预设：一个世界里只 spawn 一次（`M.reset()` 因此是幂等的）。
--
-- "只一次"是画面契约不是洁癖：实体层内部的绘制先后 = 实体槽位**升序**
-- （`world.for_each::<UiImage>` 走 `TypedStorage::order`），而 `spawn()` 会优先复用回收槽
-- （LIFO）。同一个世界里撤掉再重铺，这一批节点拿到的槽位顺序就和文件行序脱钩了——
-- 最坏的情况是那张 1280×720 无 alpha 的背景图排到最后，整块门面被盖成一张背景。
-- 离开门面时 `M.hide()` 撤实例；换场景是**新世界**，实例已经没了，回到门面时重新铺一份，
-- 那一批节点从干净的空闲表里按行序拿槽，顺序与首次启动一致（m63 ④ 钉的就是这个形状）。
local function preset_spawn()
  if not preset_alive() then
    menu_handle = prefab.spawn(PRESET_PATH)
  end
  return menu_handle
end

-- 与预设里按钮实体的 `UiLayout` 对齐（M63b 前住在 `main.scene`，**横屏 1280×720**，M32；2026-09-27）：
-- 三块木牌按钮竖排在中部（y 380 / 460 / 540），`layout.PANEL.title_btn` 给出统一矩形，
-- 每项只差 `title_btn_gap` 的纵向偏移。`label_x` 是**文字实体**（预设里 entity 9/11/13，
-- 迁移前是场景的 8/10/12）的左边缘：那份 `Text` 今天逐名藏着、字由脚本每帧画，脚本只在它的
-- 左侧留白里补选中箭头，因此两个坐标必须一起改（`tools/gen_main_menu_scene.py` 的 `BUTTONS`）。
local BTN = layout.PANEL.title_btn
local BTN_STEP = BTN.h + layout.METRIC.title_btn_gap
local BTN_LABEL_FONT = 24.0
-- 标签在按钮里水平居中：按真实字宽算左边缘（`ui.measure_text` 与渲染同口径），
-- 不再用"每字 25px"的估算——估宽会让「开始新游戏」与「继续江湖」看起来偏左/偏右。
local function label_left(label)
  local width = 0
  local ok, measured = pcall(ui.measure_text, label, BTN_LABEL_FONT)
  if ok and type(measured) == "table" and tonumber(measured.width) then
    width = tonumber(measured.width)
  end
  return math.floor(BTN.x + (BTN.w - width) / 2 + 0.5)
end

local OPTIONS = {
  {
    id = "BtnStart",
    label_text = "开始新游戏",
    scene_label = "BtnStartLabel",
    panel = { BTN.x, BTN.y, BTN.x + BTN.w, BTN.y + BTN.h },
  },
  {
    id = "BtnContinue",
    label_text = "继续江湖",
    scene_label = "BtnContinueLabel",
    panel = { BTN.x, BTN.y + BTN_STEP, BTN.x + BTN.w, BTN.y + BTN_STEP + BTN.h },
  },
  {
    id = "BtnExit",
    label_text = "退出江湖",
    scene_label = "BtnExitLabel",
    panel = { BTN.x, BTN.y + BTN_STEP * 2, BTN.x + BTN.w, BTN.y + BTN_STEP * 2 + BTN.h },
  },
}
-- 实体层（预设里那三份 `Text`）的左上角 = 标签居中位置 + 垂直居中（文字高按 30px 计，与它一致）。
for _, option in ipairs(OPTIONS) do
  local x1, y1, x2, y2 = option.panel[1], option.panel[2], option.panel[3], option.panel[4]
  option.label = { label_left(option.label_text), y1 + (((y2 - y1) - 30) // 2) }
end

-- 选中箭头及其左边距（像素）：箭头画在标签左边缘往左 `ARROW_GAP` 处，
-- 原先这里是 `"> " .. text` 的前缀位置，宽度约 19px，留 22px 不会压到标签。
local ARROW = ">"
local ARROW_GAP = 22

-- 选中项（1 基，与焦点环同步）。
local selected = 1
-- 选中高亮的颜色（暖金，与按钮贴图同色系）。
local HIGHLIGHT = { 0.85, 0.72, 0.42, 0.30 }

-- ---------- 选中反馈（M22） ----------
--
-- 手柄/键盘导航原先只有「高亮框换位置」这一个静态变化，按下去没有任何反馈，
-- 手感上像「没选中」。这里补两件事：
--   * 换项时高亮框**弹一下**（外扩 + 提亮，`POP_SECONDS` 内 ease-out 收回来）；
--   * 选中项**呼吸**（很轻的透明度起伏），让「当前选中哪一个」在静止时也一眼可见。
-- 都用固定步 `time.delta()` 推进，因此与渲染帧率无关，也和引擎固定步一致。
local POP_SECONDS = 0.14
local PULSE_SECONDS = 2.4
local pop_timer = 0
local pulse_phase = 0

-- 当前动画取值（`render` 与 headless 断言同源，避免「测试断言的公式和画面不一致」）。
-- `pop` 1 → 0（刚换项时为 1）；`grow_x/grow_y` 高亮框外扩像素；`alpha` 最终透明度。
function M.anim()
  local pop = POP_SECONDS > 0 and math.max(0, math.min(1, pop_timer / POP_SECONDS)) or 0
  local ease = 1 - (1 - pop) * (1 - pop)
  local breathe = 0.5 + 0.5 * math.cos(pulse_phase / PULSE_SECONDS * math.pi * 2)
  return {
    pop = pop,
    ease = ease,
    grow_x = 6 + 10 * ease,
    grow_y = 4 + 6 * ease,
    alpha = HIGHLIGHT[4] + 0.30 * ease + 0.06 * breathe,
  }
end

local function restart_pop()
  pop_timer = POP_SECONDS
end

local function advance_anim(delta)
  if delta and delta > 0 then
    pop_timer = math.max(0, pop_timer - delta)
    pulse_phase = (pulse_phase + delta) % PULSE_SECONDS
  end
end

function M.count()
  return #OPTIONS
end

function M.index()
  local index = nav.index(FOCUS)
  if index then
    selected = index
  end
  return selected
end

-- 当前选中项 id（`BtnStart` / `BtnContinue` / `BtnExit`）。
function M.selected_id()
  return OPTIONS[M.index()].id
end

-- 移动选中项（方向字符串 "up"/"down"）；返回是否移动。
function M.move_dir(direction)
  local before = M.index()
  local index = nav.move(FOCUS, direction)
  if not index then
    return false
  end
  selected = index
  if index ~= before then
    restart_pop()
  end
  return true
end

function M.select(index)
  local before = M.index()
  if nav.select(FOCUS, index) then
    selected = index
    if index ~= before then
      restart_pop()
    end
    return true
  end
  return false
end

local function clear()
  for _, option in ipairs(OPTIONS) do
    -- `ui.hide` 自 M22 起同时隐藏同 id 的动态面板与文本，因此高亮框、选中箭头一起收起
    --（此前它只认文本 → 非选中项的高亮框会残留）。`title:label:*` 是修复前脚本
    -- 更老版本用的另一套自画标签 id（已废弃），这里一并收起，兼容热重载留下的旧状态。
    ui.hide("title:hl:" .. option.id)
    ui.hide("title:arrow:" .. option.id)
    ui.hide("title:label:" .. option.id)
  end
  ui.hide("title:hint")
end

-- 收起本模块画的所有动态 UI（离开门面时调用；场景切换也会清空动态 UI，这里是显式兜底）。
-- 顺带把门面预设实例撤掉：`hide()` 只在**离开门面**时调用（`main.lua` 的三个按钮分支），
-- 撤干净后 `menu_handle` 归零，下次进门面重新 spawn。
-- 撤之前先验实例还在（`preset_alive`）：换场景会重建世界，实例连同引擎的登记表一起没了，
-- 此时再 despawn 就是拿旧句柄去删别的东西；而归零无论如何都要做，否则回来时不再铺。
function M.hide()
  menu_active = false
  clear()
  if preset_alive() then
    prefab.despawn(menu_handle)
  end
  menu_handle = nil
  return true
end

-- 每帧输入：返回被触发的按钮 id（调用方转交 `on_click`）或 nil。
-- 顺序：W/S → 方向键/DPad/摇杆/数字直选 → 确认（A/空格），与游戏内面板一致。
function M.update()
  clear()
  local game = game_input()
  advance_anim(select(1, time.delta()))
  -- 取消键（键盘 Esc / 手柄 B）在标题界面**不做任何事**：退出只走「选中退出江湖 +
  -- 确认」或窗口的关闭按钮，避免一键误关进程。
  -- W/S（`move_up`/`move_down`）也要能导航：顶部提示写的是「W/S 或 方向键 选择」，
  -- 而面板（对话/商店/背包/任务/暂停）都是「先看 `move_*`，再退回 `ui_step()`」——
  -- 标题菜单此前只看 `ui_step()`（= `ui_up`/`ui_down`，默认绑方向键/DPad/左摇杆），
  -- 于是 W/S 在门面上**完全没反应**（提示与行为不一致）。
  -- 用 `else`：左摇杆同时驱动 `move_*` 与 `ui_*`，一次边沿只能走一步（避免双步）。
  if input.action_pressed("move_up") then
    M.move_dir("up")
  elseif input.action_pressed("move_down") then
    M.move_dir("down")
  else
    local step = game.ui_step()
    if step == "up" then
      M.move_dir("up")
    elseif step == "down" then
      M.move_dir("down")
    end
  end
  game.select_by_digit(function(index)
    M.select(index + 1)
  end, #OPTIONS)
  if input.action_pressed("confirm") then
    return M.selected_id()
  end
  return nil
end

-- 绘制选中高亮 + 选中箭头 + 顶部操作说明。必须在实体层 UI 之后画。
--
-- 标签文字**在这里画**（三项都画），实体层那三份 `Text`（M63b 前是 `main.scene` 的 entity
-- 8/10/12，现在是预设的 9/11/13）被下面的 `ui.scene_text_visible(...,false)` 逐名藏掉。
-- 这条口径是 M22 那次重影修出来的：脚本此前把同一串文字又画了一遍（选中项还多一个 `> ` 前缀），
-- 于是按钮上出现两层文字：不选中项**整体错开 22px**（`label_x` 的 -22 是给箭头预留的左边距）、选中项
-- 因为 `> ` 的前进宽度只有约 19px 而**只错开 3px**（看起来像重影/加粗）。两层的颜色
-- 也不同（脚本纯白 vs 实体层米色），所以肉眼是「一明一暗两遍字」。
-- 现在的单一来源 = 脚本这一层；实体层那份只是留着数据（`docs/66` §4 记的 2px x 差与米色就
-- 存在它身上），选中箭头画在它左边缘之外（位置就是原先 `> ` 所在处）。
function M.render()
  -- 过渡帧防重铺：hide 撤掉后（离开门面）render 一律不画、不 spawn；
  -- 回到门面由 reset() 重新置活。
  if not menu_active then
    return false
  end
  local index = M.index()
  local anim = M.anim()
  ui_text.begin("title")
  -- 按钮标签统一由脚本绘制（实体层那三份 `Text`（BtnStartLabel 等）一并隐藏）：
  -- 实体层与脚本层若同时渲染会在同一位置叠出「两个重叠文字」（编辑器 1280×720 实测，
  -- 实体层文字 y=397 vs 旧脚本 y=380 错位 17px）。隐藏那份后每项只有一层文字。
  for _, option in ipairs(OPTIONS) do
    if option.scene_label then
      ui.scene_text_visible(option.scene_label, false)
    end
  end
  -- docs/48 ui 换肤接线：菜单装饰组（画在实体层 UI 之上；vignette/ink/logo/灯笼/按钮底/卷轴
  -- 这些底图 M63b 起住在预设里，以前在 `main.scene`）。
  -- 选中按钮 = 悬停态：btn_wuxia_hover 覆盖在 9s 按钮底上（选中即悬停，交互语义真实）。
  local sel_opt = OPTIONS[index]
  if sel_opt then
    skin.panel("title:btn_hover", sel_opt.panel[1], sel_opt.panel[2],
      sel_opt.panel[3] - sel_opt.panel[1], sel_opt.panel[4] - sel_opt.panel[2], "menu_btn_wuxia_hover")
  end
  -- 四角装饰：围绕 logo 区（384,80)-(896,336) 外扩 6px，40×40。
  skin.icon("title:orn_tl", 378, 74, 40, 40, "menu_ornament_corner")
  skin.icon("title:orn_tr", 858, 74, 40, 40, "menu_ornament_corner_tr")
  skin.icon("title:orn_bl", 378, 302, 40, 40, "menu_ornament_corner_bl")
  skin.icon("title:orn_br", 858, 302, 40, 40, "menu_ornament_corner_br")
  -- 竹分隔线：按钮区（y380-604）与贴士区之间（y600 起）。
  skin.icon("title:divider", 460, 606, 360, 10, "menu_divider_bamboo")
  -- 江湖印章：右上角（footer 在右下，右上留白）。
  skin.icon("title:seal", 1176, 20, 64, 64, "menu_seal_jianghu")
  for position, option in ipairs(OPTIONS) do
    local x1, y1, x2, y2 = option.panel[1], option.panel[2], option.panel[3], option.panel[4]
    if position == index then
      -- 换项时外扩 + 提亮（M22）：`grow_*` 是外扩像素，`alpha` 含呼吸项。
      ui.panel(
        "title:hl:" .. option.id,
        x1 - anim.grow_x,
        y1 - anim.grow_y,
        (x2 - x1) + anim.grow_x * 2,
        (y2 - y1) + anim.grow_y * 2,
        math.min(1.0, HIGHLIGHT[1] + 0.15 * anim.ease),
        math.min(1.0, HIGHLIGHT[2] + 0.15 * anim.ease),
        math.min(1.0, HIGHLIGHT[3] + 0.12 * anim.ease),
        math.min(1.0, anim.alpha)
      )
      -- 老版本由脚本自画的 `title:label:*`（已删除）若因热重载仍留着，一并收起，
      -- 否则它会作为第二层文字重新盖到脚本这一层上（正是这次修掉的重影）。
      ui.hide("title:label:" .. option.id)
      -- 选中箭头随弹跳轻微左移，与高亮框一起「推出来」；`ARROW_GAP` 让箭头落在
      -- 标签左边缘之外，与文字互不重叠。
      local arrow_x = option.label[1] - ARROW_GAP - 3.0 * anim.ease
      ui_text.draw_exact("title", "title:arrow:" .. option.id, arrow_x, option.label[2], 24.0, 48.0, ARROW)
      -- 选中态 hover 贴图（menu_btn_wuxia_hover）中心不透明，会盖住底下那层文字
      -- （默认选中第 1 项时「开始新游戏」整行不可见，实机表现为"按钮字体没显示"），
      -- 所以**选中项的文字必须画在 hover 之上**——位置取实体层那份 `Text` 的同点
      -- （x = 居中左缘、y = 它 `UiLayout` 的顶），切换选中时文字不跳。
      ui_text.draw("title", "title:label:" .. option.id,
        option.label[1], option.label[2], BTN_LABEL_FONT, BTN.w, option.label_text)
    else
      ui.hide("title:hl:" .. option.id)
      ui.hide("title:arrow:" .. option.id)
      -- 未选中项没有 hover 遮挡，但同样由脚本画：实体层那三份 `Text` 在渲染入口就被
      -- 整批藏掉了（上面的 `scene_text_visible(...,false)`），不画就是**三行都没字**。
      ui_text.draw("title", "title:label:" .. option.id,
        option.label[1], option.label[2], BTN_LABEL_FONT, BTN.w, option.label_text)
    end
  end
  ui_text.draw("title", "title:hint", layout.ROW.top_x, layout.ROW.top_y, 13.0, layout.row_width(), M.hint_line())
  return true
end

-- 顶部操作说明：按当前设备二选一（与 HUD 同一套文案来源）。
function M.hint_line()
  local confirm = hints.binding_labels({ "confirm" })
  local parts = {
    hints.is_gamepad() and "左摇杆/DPad 选择" or "W/S 或 方向键 选择",
  }
  if confirm then
    parts[#parts + 1] = confirm .. " 确认"
  end
  -- 不提示「Esc/B 退出」：取消键在门面无副作用，退出只走确认「退出江湖」或窗口关闭按钮。
  return table.concat(parts, " · ")
end

-- 进入/离开门面时复位（场景切换会清空动态 UI，选中项也回到第一项）。
function M.reset()
  menu_active = true
  selected = 1
  pop_timer = 0
  pulse_phase = 0
  -- 只在门面场景铺预设实例：`boot_game → new_game()` 的历史全局复位也会调 `reset()`，
  -- 若在山庄等玩法场景里 spawn，title 实体层（UiLayout 每帧由引擎绘制）会一直叠在 HUD 上
  -- （M71 重叠 bug 的第二层根因）。进门面（on_create 门面分支）时 `scene.reload()` 为 "main"。
  if scene.reload() == "main" then
    preset_spawn()
  end
  clear()
  nav.open(FOCUS, #OPTIONS, 1)
  nav.select(FOCUS, 1)
  return true
end

return M
