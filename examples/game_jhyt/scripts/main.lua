-- main.lua：开始画面(title) 与 山庄(main) 双场景分发。
-- 引擎切场景保留 Lua VM（require 缓存/模块局部变量存活），因此：
--   * 标题 on_click 把 entry 写入模块局部，山庄 on_create 再消费；
--   * 标题画面冻结时钟（set_rate(0)），既不推日，也不会误触发 on_day_start 存档；
--   * 只有真正进入山庄才 setup 背包/注册存档/动玩家，读档失败优雅降级为新档。
local state = require("state")
local player = require("player")
local inventory = require("inventory")
local input = require("input")
local save_manager = require("save_manager")
local farm = require("farm")

-- "new" | "continue"：标题按钮点击写入，山庄 on_create 消费
local entry

local function day_label()
  return "jianghu_youtian_day_" .. tostring(clock.now().day)
end

-- B-2 注册中心：读档时按注册表整体恢复
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
  save_manager.register("scene", function()
    return { name = scene.reload() }
  end, function(data)
    if type(data) == "table" then
      log.info("[scene] 存档场景：" .. tostring(data.name))
    end
  end)
  -- 农田地块明细：farm.init 在场景重建后从 map.state 还原，capture/restore
  -- 只保证 save blob 与 map.state 同时落盘/取回，自身不重复持有逻辑。
  save_manager.register("farm", farm.capture, farm.restore)
end

local function new_game()
  state.reset()
  clock.set(1, 0)
end

-- 从标题进入山庄：新档直接初始化；继续档先读档，无有效存档则优雅降级为新档。
local function enter_farm()
  inventory.setup()
  register_save_providers()
  if entry == "continue" then
    if not save_manager.load() then
      log.warn("[save] 无有效存档，按新档开始")
      entry = "new"
      new_game()
    end
  else
    new_game()
  end
  player.ensure()
  farm.init()
  state.set_mode("Playing")
  clock.set_rate(2)
  log.info("进入山庄：第 " .. tostring(clock.now().day) .. " 天 (mode=" .. tostring(entry) .. ")")
end

function on_create()
  if scene.reload() == "main" then
    enter_farm()
  else
    clock.set_rate(0) -- 标题冻结时钟，永不推日/误存档
    log.info("标题画面就绪：" .. tostring(scene.reload()))
  end
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
    scene.load("main")
  elseif name == "BtnExit" then
    app.quit()
  end
end

function update()
  if scene.reload() == "main" then
    return input.update()
  end
  return day_label()
end

-- 引擎约定：day.sleep() 触发 on_day_end（本次存档）→ 到清晨 → on_day_start。
-- on_day_start 内先做农田日结算（昨日浇水的作物成长+1）再存盘，保证读档即
-- “新一天醒来”且已结算的地块状态。仅山庄挂档，标题冻结时钟不会走到这里。
function on_day_start()
  if scene.reload() ~= "main" then
    return
  end
  farm.settle_day()
  log.info("新的一天：" .. tostring(clock.now().day))
  save_manager.save()
end

return state
