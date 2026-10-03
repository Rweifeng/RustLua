-- Web↔PC API 一致性门 · L3 行为契约用例（lua_api_contract.lua）
--
-- 为什么需要这一层：wasm 里的 Lua 注册层已实测与桌面**逐字节相同**（41 张表 / 275 个函数，
-- 见 scripts/wasm-lua/verify-no-modules-bridge.mjs 的 L1），但 `web-runtime.js` 的宿主补丁层
-- **覆盖了 57 个引擎已注册的实现**（见 plugins/webgl/lua_api_overrides.json）。名字两边都对得上，
-- 所以"比名字"的门抓不到分叉——分叉在返回值形状与状态归属。已实测的一例：
--   引擎版 clock.now() -> day,hour,minute,minute_of_day,paused,rate
--   打补丁后 clock.now() -> day,minute,minute_of_day（少三个字段，Lua 读到 nil 且不报错）
--
-- 这份文件把"两侧必须相同"的语义写成可执行断言：**同一份 Lua**，桌面运行时与 web（补丁后）
-- 运行时各跑一遍，输出规范化文本比对。
--   桌面：cargo run -q -p engine --example run_lua_contract [--write]
--   web ：node scripts/wechat-minigame-harness.js <发布目录> --check-api-contract
--
-- 写用例的规则（否则门会变成噪声源）：
--   1. 只用**不依赖场景/GPU/文件系统**就能跑的 API（纯状态类）；
--   2. 输出必须确定：表的键集要排序、错误文本要剥掉位置前缀、不打印 id 与地址；
--   3. 只写"两侧理应相同"的期望——平台确实不同的行为（如 Canvas 2D 的 UI 度量、
--      `input.last_device` 这种**设备拓扑**）**不写进来**，而不是写进来再豁免；
--   4. 某条用例两侧不一致 = 真 bug，修实现，不改期望。
--
-- 每条用例返回一行 `case-id | 结果`，`__contract_run()` 拼成整篇文本。

local function sorted_keys(t)
  local ks = {}
  for k, _ in pairs(t or {}) do ks[#ks + 1] = tostring(k) end
  table.sort(ks)
  return table.concat(ks, ',')
end

-- 规范化错误文本：引擎与补丁的报错前缀不同（`[string "chunk"]:12:`、`runtime error:` 等），
-- 位置信息本来就不该进契约；只留下判据用的消息体。
local function norm_error(msg)
  msg = tostring(msg)
  msg = msg:gsub('^%[string "[^"]*"%]:%d+: ', '')
  msg = msg:gsub('^runtime error: ', '')
  msg = msg:gsub('^attempt to call a nil value.+', 'CALL_NIL')
  msg = msg:gsub('%f[%w]EntityId%(%d+:g%d+%)', 'EntityId(N)')
  msg = msg:gsub('%f[%w]EntityId%(%d+:%d+%)', 'EntityId(N)')
  -- mlua 的 `bad argument #1 to 'x' (type expected)` 里带着**调用栈**（多行、行号两侧不同），
  -- 契约必须一条一行：折成空格、截断，只留首行的判据部分。
  msg = msg:gsub('[\r\n\t]+', ' ')
  msg = msg:match('^(.-)%s*stack traceback:.-$') or msg
  msg = msg:gsub('%d+', 'N')
  return msg
end

-- 记录"调用结果形状"：成功 → 键集；失败 → 规范化后的错误。
local function shape(call)
  local ok, value = pcall(call)
  if not ok then return 'ERR(' .. norm_error(value) .. ')' end
  if value == nil then return 'nil' end
  if type(value) ~= 'table' then return 'scalar(' .. type(value) .. ')=' .. tostring(value) end
  return 'keys(' .. sorted_keys(value) .. ')'
end

local cases = {}

-- ---- clock / calendar / day：返回值形状与边界（这一组已知分叉，正是本门要钉住的）----

cases['clock.now.shape'] = function()
  return shape(function() return clock.now() end)
end

cases['clock.after_set'] = function()
  clock.set(3, 700)
  return shape(function() return clock.now() end) .. '/day=' .. tostring(clock.now().day)
    .. '/minute=' .. tostring(clock.now().minute)
end

cases['clock.rate_and_pause'] = function()
  clock.set_rate(2.5)
  clock.pause(true)
  return 'rate=' .. tostring(clock.now().rate) .. '/paused=' .. tostring(clock.now().paused)
end

cases['clock.set.invalid'] = function()
  local ok = pcall(function() clock.set(0, 0) end)
  return 'day0=' .. tostring(ok)
end

cases['clock.add.negative_clamp'] = function()
  clock.set(2, 30)
  clock.add(-9999)
  return 'day=' .. tostring(clock.now().day) .. '/minute=' .. tostring(clock.now().minute)
end

cases['clock.sleep_to_morning'] = function()
  clock.set(5, 1000)
  local new_day = clock.sleep_to_morning()
  return 'new_day=' .. tostring(new_day) .. '/minute=' .. tostring(clock.now().minute)
end

cases['calendar.now.shape'] = function()
  return shape(function() return calendar.now() end)
end

cases['calendar.roundtrip'] = function()
  calendar.set(14, 3, 2, 600)
  local c = calendar.now()
  return 'day=' .. tostring(c.day) .. '/season=' .. tostring(c.season)
    .. '/year=' .. tostring(c.year) .. '/absolute_day=' .. tostring(c.absolute_day)
end

cases['calendar.set.out_of_range'] = function()
  local ok = pcall(function() calendar.set(29, 1, 1, 0) end)
  return 'day29=' .. tostring(ok)
end

cases['day.now.shape'] = function()
  return shape(function() return day.now() end)
end

-- ---- item：定义表的默认值与钳位（补丁版与引擎版各自维护一份定义表）----

cases['item.define.defaults'] = function()
  item.define('契约物品', { name = '契约' })
  return shape(function() return item.def('契约物品') end)
end

cases['item.define.roundtrip'] = function()
  item.define('c_sword', { name = '剑', description = 'd', kind = 'weapon', texture = 't.png', stack = 5, buy = 10, sell = 3 })
  local d = item.def('c_sword')
  return 'name=' .. tostring(d.name) .. '/kind=' .. tostring(d.kind) .. '/stack=' .. tostring(d.stack)
    .. '/buy=' .. tostring(d.buy) .. '/sell=' .. tostring(d.sell)
end

cases['item.define.clamp_stack'] = function()
  item.define('c_clamp', { stack = 0 })
  return 'stack=' .. tostring(item.def('c_clamp').stack)
end

cases['item.def.missing'] = function()
  return 'nil=' .. tostring(item.def('c_not_defined_at_all'))
end

cases['item.define.empty_id'] = function()
  return 'ok=' .. tostring(pcall(function() item.define('', { name = 'x' }) end))
end

-- ---- event：分发顺序与退订（两侧各存一份订阅表）----

cases['event.emit.order'] = function()
  local log_rows = {}
  event.on('契约.事件', function() log_rows[#log_rows + 1] = 'a' end)
  event.on('契约.事件', function() log_rows[#log_rows + 1] = 'b' end)
  event.emit('契约.事件')
  return table.concat(log_rows, '>')
end

cases['event.emit.payload'] = function()
  local seen = 'none'
  event.on('契约.带参', function(data) seen = tostring(data) end)
  event.emit('契约.带参', 'payload')
  return 'seen=' .. seen
end

cases['event.unsubscribe.by_callback'] = function()
  local count = 0
  local handler = function() count = count + 1 end
  event.on('契约.退订', handler)
  event.emit('契约.退订')
  event.unsubscribe('契约.退订', handler)
  event.emit('契约.退订')
  return 'count=' .. count
end

cases['event.off_alias'] = function()
  local count = 0
  local handler = function() count = count + 1 end
  event.subscribe('契约.off', handler)
  event.off('契约.off', handler)
  event.emit('契约.off')
  return 'count=' .. count
end

-- ---- container：背包复用（键 `container:<id>`）----

cases['container.open.get.close'] = function()
  local key = container.open(9001, 6)
  local same = container.get(9001)
  container.close(9001)
  return 'key=' .. tostring(key) .. '/same=' .. tostring(key == same)
    .. '/after_close=' .. tostring(container.get(9001))
end

-- ---- shop：目录与价格读取 ----

cases['shop.price'] = function()
  shop.set_catalog('契约店', { c_sword = { buy = 10, sell = 3 } })
  return shape(function() return shop.price('契约店', 'c_sword') end)
    .. '/buy=' .. tostring(shop.price('契约店', 'c_sword').buy)
    .. '/missing=' .. tostring(shop.price('契约店', 'nope'))
    .. '/no_catalog=' .. tostring(shop.price('没这家店', 'c_sword'))
end

-- ---- interact：prompt 状态与半径校验（query 依赖实体位置，只校验参数契约）----

cases['interact.prompt_roundtrip'] = function()
  interact.prompt('按 E 拾取')
  local got = tostring(interact.get_prompt())
  interact.clear_prompt()
  return 'got=' .. got .. '/cleared=' .. tostring(interact.get_prompt())
end

cases['interact.query.negative_radius'] = function()
  return 'ok=' .. tostring(pcall(function() interact.query(0, 0, -1) end))
end

-- ---- input：动作表与无设备时的取值 ----
--
-- 动作名**不硬编码**：`move_up` 这类名字来自引擎默认 action map，而两侧都可能被项目配置改过。
-- 所以先由 `input.actions.list` 把清单本身钉住，下面的用例再从 `input.actions()` 取前两个名字——
-- 清单一致 ⇒ 取到的名字一致 ⇒ 取值断言可移植。
local function action_names()
  local names = input.actions()
  return names[1], names[2]
end

cases['input.actions.list'] = function()
  local names = input.actions()
  local sorted = {}
  for _, name in ipairs(names) do sorted[#sorted + 1] = tostring(name) end
  table.sort(sorted)
  return 'n=' .. #names .. '/names(' .. table.concat(sorted, ',') .. ')'
end

cases['input.action.no_device'] = function()
  local first, second = action_names()
  return 'pressed=' .. tostring(input.action_pressed(first))
    .. '/released=' .. tostring(input.action_released(first))
    .. '/axis=' .. tostring(input.action_axis(first, second))
    .. '/repeat=' .. tostring(input.action_repeat(first))
end

cases['input.bindings.shape'] = function()
  local first = action_names()
  return 'n=' .. #input.bindings(first)
end

-- ---- tag / light：需要活体实体，两侧都应当拒绝（错误文本必须一致）----

cases['tag.get.unknown_entity'] = function()
  return 'ok=' .. tostring(pcall(function() tag.get(999999, 'k') end))
end

cases['light.get.unknown_entity'] = function()
  return 'ok=' .. tostring(pcall(function() light.get(999999) end))
end

-- ---- ui：动态 UI 的注册/读回形状（绘制后端不同，这里只契约"表里有没有这项能力"）----

cases['ui.text.set_get'] = function()
  -- 只契约 **PC 保证的字段子集**：web 的记录额外带 x/y/scale（Canvas 2D 侧的绘制状态），
  -- 那是超集不是分叉——读 `.text` / `.visible` 的消费者两边一致就够了。
  -- 把"键集必须完全相同"写成期望会把合法的宿主侧扩展一直判红，那种红最后只会被无视。
  -- 参数按引擎签名 `(id, text)` 给（api/ui.rs 的 ui.text 就收两个，位置由 text_at 负责）。
  ui.text('契约文本', '你好')
  local got = ui.get('契约文本') or {}
  return 'text=' .. tostring(got.text) .. '/visible=' .. tostring(got.visible)
end

cases['ui.scroll.lifecycle'] = function()
  ui.scroll_to_visible('契约滚动', 30, 100)
  local offset = ui.scroll_offset('契约滚动')
  ui.scroll_close('契约滚动')
  return 'offset=' .. tostring(offset)
end

cases['ui.focus.shape'] = function()
  return 'open=' .. tostring(pcall(function() ui.focus_open('契约焦点', 3) end))
    .. '/count=' .. tostring(select(1, pcall(function() return ui.focus_count('契约焦点') end)))
end

function __contract_run()
  local names = {}
  for name, _ in pairs(cases) do names[#names + 1] = name end
  table.sort(names)
  local rows = {}
  for _, name in ipairs(names) do
    local ok, result = pcall(cases[name])
    if ok then
      rows[#rows + 1] = name .. ' | ' .. tostring(result)
    else
      rows[#rows + 1] = name .. ' | CASE_ERROR(' .. norm_error(result) .. ')'
    end
  end
  return table.concat(rows, '\n')
end
