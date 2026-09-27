local state = require("core.state")
local player = require("player.player")
local interaction = require("systems.interaction")
local dialogue = require("world.dialogue")
local shop = require("systems.shop")
local combat = require("systems.combat")
local building = require("systems.building")
local relationship = require("player.relationship")
-- M19-02：HUD/提示、新手引导、三个面板（背包/任务/暂停）与音频。
local audio = require("core.audio")
local hud = require("ui.hud")
local tutorial = require("ui.tutorial")
-- M27：七日目标（与 tutorial 同一套事件接线——每帧喂一次 last_action）。
local goals = require("systems.goals")
local inventory_ui = require("ui.inventory_ui")
local quest_ui = require("ui.quest_ui")
local pause_menu = require("ui.pause_menu")
local virtual_pad = require("ui.virtual_pad")

-- input.lua：《江湖有田》输入分发（M16 → M20-08）。
--
-- M20-08：从「物理键直读」迁移到**命名动作**（引擎 `core/input.rs` 的
-- `ActionMap`），玩家可以在编辑器「项目设置 → 输入」或 settings.toml 的
-- `[input]` 段整体改键。迁移原则是**逐键等价**：
--
--   * 键盘默认绑定与迁移前完全一致：WASD 移动、E 交互/装备、J 普攻、K 武学、
--     G 送礼、T 睡觉、Q 修炼（建造模式下切换建筑）、I 背包、L 任务、P 暂停、
--     B 建造模式、F 放置（商店里是切换买卖页）、X 拆除、U 使用/升级、
--     R 战败重开、C 选种子、1-9 直选、方向键菜单导航、空格确认、Esc 取消。
--   * 手柄默认绑定（M20-06 设备契约）：左摇杆 = 移动（上为正）与菜单导航、
--     DPad = 菜单导航、A = confirm、B = cancel；其余动作可用编辑器捕获改绑。
--   * `main.lua` 启动时调用 `input.load_settings()` 应用玩家改键；文件缺失/未改键
--     时保持默认，因此既有 headless 回归（直接喂物理键）语义不变。
--
-- 数字输入（键盘按键 / DPad / 摇杆）都有「按下」边沿：菜单导航一次按下 = 一步；
-- 摇杆（模拟量）额外支持「首次立即 + 按住连发」——M22 起这两个语义都由**引擎**按
-- 毫秒执行（`input.action_repeat`，见 `ui_step`），因此 144Hz 与 60Hz 手感一致。

local M = {}

local MOVE_SPEED = 140 -- 世界单位/秒（32px 格 → 约 4.4 格/秒）

-- 导航手感（摇杆阈值 + 按住连发节奏）由 `ui.hints` 持有并可被工程 settings.toml 的
-- `[input]` 段覆盖（`nav_threshold` / `nav_delay_ms` / `nav_repeat_ms`，旧的帧键仍可读）；
-- 启动时 `apply_nav_settings()` 把它推给引擎（`input.set_nav_settings`），引擎负责
-- 轴判定阈值与连发计时，Lua 侧不再自己数帧。
--
-- 设备判定（键鼠/手柄两套说明）与「绑定 → 可读名字」也在 `ui.hints`：面板自己画
-- 底部提示时需要同一套逻辑，而它们不能 require 本模块（`input → dialogue` 会成环）。
local hints = require("ui.hints")

-- 把 `input.nav_settings()`（= 工程 settings.toml 的 [input] 段）应用到导航手感。
function M.apply_nav_settings()
  return hints.apply_nav_settings()
end

-- 当前导航手感（诊断/测试读取；不改状态）。
function M.nav_settings()
  return hints.nav_settings()
end

-- 本帧的菜单导航方向（"up"/"down"/"left"/"right"）或 nil。
--
-- M22：统一走 `input.action_repeat`——
--   * 首次按下（键盘方向键 / DPad / 摇杆越阈值）立即返回一步；
--   * 摇杆继续保持时，由引擎在 delay_ms 后每 repeat_ms 再给一步（时间制）；
--   * 数字键/DPad 按住**不连发**（一次按下 = 一步），保持 M20-08 的既有手感。
-- 引擎缺该 API（旧引擎）时回落到 `action_pressed`（只有边沿，没有连发）。
local function ui_step()
  local repeat_action = input.action_repeat
  local step = repeat_action or input.action_pressed
  if step("ui_up") then
    return "up"
  end
  if step("ui_down") then
    return "down"
  end
  if step("ui_left") then
    return "left"
  end
  if step("ui_right") then
    return "right"
  end
  return nil
end

-- 按移动轴合成方向并直驱刚体速度。
-- 面向（state.facing）记录最后一次有效轴向，供 farm.act 计算面前格。
-- 键盘输入下 `action_axis` 仍是 ±1/0，与迁移前的逐键逻辑完全一致；
-- 长度 >1（斜向）时按比例缩放到 1，摇杆则保留模拟量（0..1）。
local function update_movement()
  local dx = input.action_axis("move_left", "move_right")
  local dy = input.action_axis("move_down", "move_up")

  if dx ~= 0 or dy ~= 0 then
    local len = math.sqrt(dx * dx + dy * dy)
    if len > 1 then
      dx, dy = dx / len, dy / len
    end
    if dx > 0 then
      state.facing = "right"
    elseif dx < 0 then
      state.facing = "left"
    elseif dy > 0 then
      state.facing = "up"
    elseif dy < 0 then
      state.facing = "down"
    end
    player.set_velocity(dx * MOVE_SPEED, dy * MOVE_SPEED)
    state.last_action = "move"
  else
    player.stop()
  end
end

-- 数字键 1..9 直选（0 基序号）：走命名动作 `slot_1..slot_9`（可改绑）；
-- 返回是否命中。
local function select_by_digit(select_fn, limit)
  for i = 1, limit do
    if input.action_pressed("slot_" .. tostring(i)) then
      select_fn(i - 1)
      return true
    end
  end
  return false
end

-- 供其它菜单复用（M20-09 标题菜单 `ui.title_menu`）：同一套「方向/DPad 一步 +
-- 摇杆按住重复 + 数字直选」语义，避免标题菜单另写一份手感不一致的导航。
function M.ui_step()
  return ui_step()
end

function M.select_by_digit(select_fn, limit)
  return select_by_digit(select_fn, limit)
end

-- 面板自愈：mode 停在某个 UI 态但面板并未打开（读档/切图后残留、旧档里
-- 保存的 mode）时复位 Playing，避免输入被永久吞掉（M16-01 状态机语义）。
local function heal_mode()
  local stuck = (state.is_mode("Inventory") and not inventory_ui.ui_active())
    or (state.is_mode("Quest") and not quest_ui.ui_active())
    or (state.is_mode("Pause") and not pause_menu.ui_active())
    or (state.is_mode("Shop") and not shop.ui_active())
    or (state.is_mode("Dialogue") and not dialogue.is_open())
  if stuck then
    state.set_mode("Playing")
    return true
  end
  return false
end

-- 每帧收尾：推进引导 → 绘制 HUD → 返回单行调试文案。
-- 顶部键位提示从**当前绑定**推导（改键后提示同步变化，见 M.key_hints）。
local function frame()
  tutorial.on_action(state.last_action)
  -- M27：目标计数只认"动作变化"（goals 内部做边沿检测），
  -- 因此同一个 last_action 不会在后续每一帧重复计数。
  goals.on_action(state.last_action)
  hud.draw(M.key_hints())
  virtual_pad.draw()
  return hud.text()
end

local function update_context()
  if input.action_pressed("pause") then
    pause_menu.open()
  elseif input.action_pressed("inventory") then
    inventory_ui.open()
  elseif input.action_pressed("quest") then
    quest_ui.open()
  end
end

-- 对话态：冻结移动与移动端交互，Esc/B 关闭、数字键选择选项、W/S 或方向键/
-- DPad/摇杆选择、空格/A 确认当前项。必须在 `not Playing` 早退之前分发，
-- 否则对话输入会被吞掉（M17-03 风险项）。
local function update_dialogue()
  player.stop()
  if input.action_pressed("cancel") then
    dialogue.close()
  else
    if input.action_pressed("move_up") then
      dialogue.move(-1)
    elseif input.action_pressed("move_down") then
      dialogue.move(1)
    else
      local step = ui_step()
      if step == "up" then
        dialogue.move_dir("up")
      elseif step == "down" then
        dialogue.move_dir("down")
      end
    end
    for i = 1, dialogue.option_limit() do
      if input.action_pressed("slot_" .. tostring(i)) then
        dialogue.choose(i)
        break
      end
    end
    -- 手柄确认（A）/空格：选择当前高亮项（`choose()` 缺省即当前项）。
    if state.is_mode("Dialogue") and input.action_pressed("confirm") then
      dialogue.choose()
    end
  end
  -- 换节点后重绘；若已结束（close 复位 Playing）则不再绘制。
  if state.is_mode("Dialogue") then
    dialogue.render()
  end
  return frame()
end

-- 商店态：冻结移动，Esc/B 关闭、F 切换买卖、+/- 调整数量、数字键选择、
-- 空格/A 确认，W/S 或方向键/DPad/摇杆选择。同样必须在 `not Playing` 早退之前分发。
local function update_shop()
  player.stop()
  if input.action_pressed("cancel") then
    shop.close_ui()
  else
    if input.action_pressed("place") then
      shop.toggle_tab()
    end
    if input.action_pressed("volume_up") then
      shop.adjust(1)
    elseif input.action_pressed("volume_down") then
      shop.adjust(-1)
    end
    if input.action_pressed("move_up") then
      shop.move(-1)
    elseif input.action_pressed("move_down") then
      shop.move(1)
    else
      local step = ui_step()
      if step == "up" then
        shop.move_dir("up")
      elseif step == "down" then
        shop.move_dir("down")
      end
    end
    select_by_digit(function(index)
      shop.select(index + 1)
    end, shop.row_limit())
    if input.action_pressed("confirm") then
      shop.confirm()
    end
  end
  -- 换页/交易后重绘；若已关闭（close_ui 复位 Playing）则不再绘制。
  if state.is_mode("Shop") then
    shop.render()
  end
  return frame()
end

-- 背包态（M19-02）：W/S/A/D、方向键/DPad/摇杆或数字键选择，U/空格 使用，
-- E 装备，C 选种子，I/Esc/B 关闭。音效在 inventory_ui 内按结果播放
-- （成功确认 / 失败取消）。
local function update_inventory()
  player.stop()
  if input.action_pressed("cancel") or input.action_pressed("inventory") then
    inventory_ui.close()
  else
    if input.action_pressed("move_up") then
      inventory_ui.move(-1)
    elseif input.action_pressed("move_down") then
      inventory_ui.move(1)
    elseif input.action_pressed("move_left") then
      inventory_ui.move(-inventory_ui.row_limit() / 2)
    elseif input.action_pressed("move_right") then
      inventory_ui.move(inventory_ui.row_limit() / 2)
    else
      local step = ui_step()
      if step == "up" then
        inventory_ui.move_dir("up")
      elseif step == "down" then
        inventory_ui.move_dir("down")
      elseif step == "left" then
        inventory_ui.move_dir("left")
      elseif step == "right" then
        inventory_ui.move_dir("right")
      end
    end
    select_by_digit(function(index)
      inventory_ui.select(index)
    end, 9)
    if input.action_pressed("use") or input.action_pressed("confirm") then
      inventory_ui.use()
    end
    if input.action_pressed("interact") then
      inventory_ui.equip()
    end
    if input.action_pressed("seed") then
      inventory_ui.choose_seed()
    end
  end
  if state.is_mode("Inventory") then
    inventory_ui.render()
  end
  return frame()
end

-- 任务日志态（M19-02，只读）：W/S、方向键/DPad/摇杆或数字键选择，
-- L/Q/Esc/B 关闭。
local function update_quest()
  player.stop()
  if input.action_pressed("cancel")
    or input.action_pressed("quest")
    or input.action_pressed("practice") then
    quest_ui.close()
  else
    if input.action_pressed("move_up") then
      quest_ui.move(-1)
    elseif input.action_pressed("move_down") then
      quest_ui.move(1)
    elseif input.action_pressed("move_left") then
      quest_ui.move(-quest_ui.row_limit())
    elseif input.action_pressed("move_right") then
      quest_ui.move(quest_ui.row_limit())
    else
      local step = ui_step()
      if step == "up" then
        quest_ui.move_dir("up")
      elseif step == "down" then
        quest_ui.move_dir("down")
      end
    end
    select_by_digit(function(index)
      quest_ui.select(index)
    end, 9)
  end
  if state.is_mode("Quest") then
    quest_ui.render()
  end
  return frame()
end

-- 暂停菜单态（M19-02/M20-02）：W/S 或方向键/DPad/摇杆选择、空格/A 确认、
-- +/- 音量、Esc/B/P 继续。设置页打开时 Esc 返回主菜单、空格保存/确认
-- （P 仍直接继续）。移动与世界由状态机冻结（早期 return，不调用
-- update_movement/npc.tick 之外的逻辑）。
local function update_pause()
  player.stop()
  if input.action_pressed("cancel") then
    if pause_menu.settings_active() then
      pause_menu.settings_close()
    else
      pause_menu.close()
    end
  elseif input.action_pressed("pause") then
    pause_menu.close()
  else
    local direction = nil
    if input.action_pressed("move_up") then
      direction = "up"
    elseif input.action_pressed("move_down") then
      direction = "down"
    else
      direction = ui_step()
    end
    if direction == "up" then
      pause_menu.move(-1)
    elseif direction == "down" then
      pause_menu.move(1)
    end
    select_by_digit(function(index)
      pause_menu.select(index)
    end, pause_menu.row_limit())
    if input.action_pressed("volume_up") then
      pause_menu.adjust_volume(1)
    elseif input.action_pressed("volume_down") then
      pause_menu.adjust_volume(-1)
    end
    if input.action_pressed("confirm") then
      pause_menu.confirm()
    end
  end
  if state.is_mode("Pause") then
    pause_menu.render()
  end
  return frame()
end

-- ---------- 键鼠 / 手柄两套操作说明（M20-09） ----------
--
-- 实现在 `ui.hints`（面板自己画底部提示时也要用同一套逻辑，而它们不能 require
-- 本模块：`input → dialogue` 会成环）。这里保留同名入口，内部全部转发。
-- 顶部提示按**最近使用的设备**二选一：推摇杆/按手柄键 → 手柄说明；动键盘/鼠标 →
-- 键鼠说明；两者都没动时保持上一次（首次按键鼠显示）。

-- 当前生效的操作说明设备。
function M.device()
  return hints.device()
end

-- 测试/调试用：强制某套说明（`"keyboard"`/`"gamepad"`），`nil` 恢复自动检测。
function M.set_device(device)
  return hints.set_device(device)
end

-- 指定设备的单行操作说明（该设备无绑定 → 空串）。
function M.hint_for_device(device)
  return hints.hint_for_device(device)
end

-- 单行键位提示（当前设备优先；无绑定回退另一套；都为空给兜底文案）。
function M.key_hints(device)
  return hints.key_hints(device)
end

-- 面板底部提示：同一句文案按设备二选一（`{ keyboard = "...", gamepad = "..." }`）。
function M.panel_hint(pairs, fallback)
  return hints.panel_hint(pairs, fallback)
end

-- 动作在当前设备的按键名（无该设备绑定返回 nil）。
function M.binding_label(action, device)
  return hints.binding_label(action, device)
end

-- 多个动作的按键名用 " / " 连接；全无绑定返回 nil。
function M.binding_labels(actions, device)
  return hints.binding_labels(actions, device)
end

-- 面板提示片段拼装（`{ { keyboard = ..., gamepad = ..., label = ... }, ... }`）。
function M.panel_line(spec, separator)
  return hints.panel_line(spec, separator)
end

function M.update()
  virtual_pad.update()
  player.ensure()
  -- 模式自愈：读档/切图后 mode 可能停在没有面板的 UI 态（会永久吞输入），
  -- 先复位 Playing 再按状态分发。
  heal_mode()

  -- UI 态优先于世界输入分发（M16-01）：这些分支在 `not Playing` 早退之前。
  if state.is_mode("Dialogue") then
    return update_dialogue()
  end

  if state.is_mode("Shop") then
    return update_shop()
  end

  if state.is_mode("Inventory") then
    return update_inventory()
  end

  if state.is_mode("Quest") then
    return update_quest()
  end

  if state.is_mode("Pause") then
    return update_pause()
  end

  update_context()

  if not state.is_mode("Playing") then
    player.stop()
    return frame()
  end

  -- 战败态：冻结移动与攻击，仅 R 重开（复用 Playing 态，不新增状态机模式）。
  if combat.is_player_dead() then
    player.stop()
    if input.action_pressed("respawn") then
      -- M28：战败结算（扣未入库掉落 50% + 铜钱 5%）+ 回山庄 + 状态回满，
      -- 由一个入口负责，避免"复活了但没回庄""回庄了但没扣惩罚"这类半状态。
      local ok = require("systems.death").apply()
      if not ok then
        combat.respawn()
      end
    end
    hud.draw(M.key_hints())
    return "战败 · 按 R 重新开始"
  end

  -- 受击硬直（M25）：被打断的这段时间不能移动/攻击/交互，但面板键（I/Q/Esc 等）
  -- 已在上面各 mode 分支处理完毕，所以不会把玩家锁在战斗里出不来。
  if not combat.player_can_act() then
    player.stop()
    state.last_action = "hurt"
    hud.draw(M.key_hints())
    return "受击僵直 · " .. tostring(require("systems.combat").player_hurt_frames()) .. " 步"
  end

  update_movement()

  if input.action_pressed("interact") then
    local result = interaction.interact(state.facing)
    if result ~= "move" and result ~= "none" then
      state.last_action = result
    end
  end

  -- 战斗输入（M18-01）：J 普攻（随武器加成）、K 武学（消耗内力）。
  -- 攻击不锁移动（ARPG 习惯）；冷却/前摇/去重由 combat 底座处理。
  -- M25：失败原因写进 last_action（此前返回值被丢弃 → 内力不足/冷却中按下去
  -- 毫无反馈，玩家只能猜），HUD 按 last_action 出中文提示。
  if input.action_pressed("attack") then
    local ok, reason = combat.player_attack("strike")
    if not ok then
      state.last_action = reason
      audio.play_on("ui", "ui_cancel")
    end
  elseif input.action_pressed("skill") then
    local ok, reason = combat.player_attack("palm_gust")
    if not ok then
      state.last_action = reason
      audio.play_on("ui", "ui_cancel")
    end
  end

  -- 送礼输入（M18-04）：G 面向面前 NPC 送出第一件其可收的礼物。
  -- 与 J/K 攻击同样不锁移动（ARPG 习惯）；结果文案进 last_action 供 HUD/验收。
  if input.action_pressed("gift") then
    local ok, info = relationship.gift_facing(state.facing)
    if ok then
      state.last_action = "gift_" .. tostring(info)
    else
      state.last_action = tostring(info)
    end
  end

  -- 建造输入（M18-02）：B 开关建造模式、Q 在建造模式下切换所选建筑、
  -- F 在面前格放置（预览冲突逐格原因写入 last_action）、X 拆除面前建筑、
  -- U 升级面前建筑。建造不锁移动；Q 非建造模式时保持原「修炼」语义。
  if input.action_pressed("build") then
    building.set_build_mode(not building.build_mode())
  elseif input.action_pressed("practice") and building.build_mode() then
    building.cycle_selected()
  end

  if input.action_pressed("place") and building.build_mode() then
    local preview = building.preview(state.facing)
    if not preview.building then
      state.last_action = tostring(preview.reason or "no_selection")
    elseif preview.ok then
      local ok = building.place(preview.building, preview.anchor.x, preview.anchor.y)
      if not ok then
        state.last_action = "build_failed"
      end
    else
      -- 占领冲突：把首个冲突格与原因写进 last_action（HUD/诊断可见）。
      local first = preview.conflicts[1]
      state.last_action = first
          and ("build_blocked_" .. tostring(first.reason) .. "@" .. tostring(first.x) .. "," .. tostring(first.y))
        or "build_blocked"
    end
  end

  if input.action_pressed("demolish") then
    local rec = building.facing_building(state.facing)
    if rec then
      local ok, reason = building.remove(rec.cx, rec.cy)
      if not ok then
        state.last_action = tostring(reason)
      end
    end
  end

  if input.action_pressed("use") then
    local rec = building.facing_building(state.facing)
    if rec then
      local ok, reason = building.upgrade(rec.cx, rec.cy)
      if not ok then
        state.last_action = tostring(reason)
      end
    end
  end

  if input.action_pressed("sleep") then
    state.last_action = "sleep"
    audio.sfx("sfx_sleep")
    -- day.sleep() 依次触发 on_day_end（存档）与 on_day_start（日结算/再存档）
    day.sleep()
  end

  if input.action_pressed("practice") then
    state.inner_power = math.min(100, (state.inner_power or 0) + 5)
    state.last_action = "practice_breath"
  end

  return frame()
end

-- M20-08：把引擎的**命名动作设置**入口转出给 `main.lua`（启动时应用玩家改键）。
-- 语义与 `audio.load_settings` 对齐：文件缺失/缺 `[input]` 段 → `false` + 原因且
-- **保留当前绑定**（即引擎默认，键盘行为与迁移前逐键等价）；版本头/绑定非法 →
-- 引擎侧回退默认并写入可定位诊断。`path` 省略时用 `<project_root>/settings.toml`。
function M.load_settings(path)
  return input.load_settings(path)
end

return M
