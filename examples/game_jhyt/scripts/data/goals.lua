-- data/goals.lua：《江湖有田》七日目标/里程碑（M27 / 计划 G1，纯数据）。
--
-- MVP 剧本（docs/46）是一条线性流程，但玩家**不知道**"今天该干什么"：
-- 只有 `data/tutorial.lua` 的 10 步一次性引导，引导结束后 HUD 只剩一行 last_action。
-- 本表给出「七日目标」：每天 3–4 条目标 + 当天全清奖励，第 7 天走完即达成七日里程碑。
--
-- 数据结构：
--   day          第几天（1..7，与 clock.now().day 对齐）
--   title        当天主题（HUD 目标行的抬头）
--   hint         一句话提示（可选，验收与调试面板用）
--   goals[]      当天目标，顺序即显示顺序
--     id         全局唯一（存档变量名 `goals_p_<id>` 依赖它唯一）
--     text       玩家看到的文案（中文、≤ 20 字，HUD 一行放得下）
--     match      达成条件（见下）
--     count      需要的次数（正整数，默认 1）
--   reward       当天全部目标达成后的奖励；item / money / inner_power 三选一
--
-- `match.kind` 支持两类条件（`systems/goals.lua` 实现）：
--   * 事件类（由 last_action / 跨天 / 切图事件驱动）
--       action          last_action 精确等于 value
--       action_prefix   last_action 以 value 开头（如 plant_ / water_ / buy_）
--       day_start       跨天（睡觉到天亮）——**每天最后一条必须是它**，
--                       日结算就挂在这个事件上（见 validate_all 的结构断言）
--   * 状态类（随时可判定，切天/接档后立刻能对上，避免"目标在下一天开始时
--     其实早就满足却要玩家再跑一趟"）
--       enter_map       当前地图 == value
--       quest_accepted  任务已接（非 not_started）
--       quest_delivered 任务已交付（completed）
--       kills           累计击杀数 >= count
--       item            背包里某物品数量 >= count
--       money           金钱 >= count
--
-- 与其它系统的关系（单一来源原则）：
--   * 目标**只读**现有系统状态（任务/背包/金钱/战斗统计/地图），不复制数值；
--   * 奖励走既有 API（inventory.add / state.money / state.inner_power）；
--   * 目标本身不产生副作用，唯一写入是进度计数与当日结算结果（state.vars 标量）。
local M = {}

M.version = 1

M.days = {
  {
    day = 1,
    title = "第 1 天 · 安顿药田",
    hint = "顾长风说过，山庄的药田不能荒着。",
    goals = {
      { id = "d1_till", text = "翻一块药田", match = { kind = "action", value = "till" } },
      { id = "d1_plant", text = "种下 1 株作物", match = { kind = "action_prefix", value = "plant_" } },
      { id = "d1_water", text = "给作物浇一次水", match = { kind = "action_prefix", value = "water_" } },
      { id = "d1_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { item = "seed_rice", count = 2 }
  },
  {
    day = 2,
    title = "第 2 天 · 初入云泽村",
    hint = "村里的集市什么都有；山庄药圃边上也有野药可采。",
    goals = {
      { id = "d2_village", text = "进云泽村看看", match = { kind = "enter_map", value = "village" } },
      { id = "d2_gather", text = "采集 1 次草药", match = { kind = "action_prefix", value = "gather_" } },
      { id = "d2_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { money = 30 }
  },
  {
    day = 3,
    title = "第 3 天 · 拜见师父",
    hint = "药田有了起色，去回禀顾长风；顺路在铺子里添置些家什。",
    goals = {
      { id = "d3_mainline", text = "向顾长风复命", match = { kind = "quest_delivered", value = "mainline_arrival" } },
      { id = "d3_buy", text = "在铺子里买 1 样东西", match = { kind = "action_prefix", value = "buy_" } },
      { id = "d3_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { inner_power = 10 }
  },
  {
    day = 4,
    title = "第 4 天 · 入山采药",
    hint = "青云山道的药草最肥，多采两株回来。",
    goals = {
      { id = "d4_mountain", text = "上青云山道", match = { kind = "enter_map", value = "mountain_path" } },
      { id = "d4_gather", text = "采集 2 次草药", count = 2, match = { kind = "action_prefix", value = "gather_" } },
      { id = "d4_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { item = "lingzhi", count = 1 }
  },
  {
    day = 5,
    title = "第 5 天 · 受托炼药",
    hint = "沈青萝的订单要灵芝，山庄药庐也该盖起来了。",
    goals = {
      { id = "d5_order", text = "接下药庐的订单", match = { kind = "quest_accepted", value = "order_herb" } },
      { id = "d5_build", text = "盖起 1 座建筑", match = { kind = "action_prefix", value = "build_" } },
      { id = "d5_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { money = 50 }
  },
  {
    day = 6,
    title = "第 6 天 · 江湖试手",
    hint = "演武场的木人桩和灰狼，正好练练手脚。",
    goals = {
      { id = "d6_arena", text = "去演武场", match = { kind = "enter_map", value = "combat_arena" } },
      { id = "d6_kill", text = "击败 1 名对手", match = { kind = "kills" } },
      { id = "d6_sleep", text = "回屋睡到天亮", match = { kind = "day_start" } }
    },
    reward = { money = 60 }
  },
  {
    day = 7,
    title = "第 7 天 · 七日之约",
    hint = "炼出第一剂药，就算在山庄站住了脚。",
    goals = {
      { id = "d7_collect", text = "炼成 1 剂药并收取", match = { kind = "action_prefix", value = "craft_collect_" } },
      { id = "d7_pill", text = "背包里有 1 剂回复药", match = { kind = "item", value = "heal_pill" } },
      { id = "d7_sleep", text = "睡满第七夜", match = { kind = "day_start" } }
    },
    reward = { money = 80 }
  }
}

-- 七日里程碑奖励：7 天全部达成（`goals_day_<n> == "completed"`）后发一次。
M.milestone = {
  title = "七日之约达成",
  money = 200,
  note = "山庄的日子算是立住了。"
}

M.KINDS = {
  action = true,
  action_prefix = true,
  day_start = true,
  enter_map = true,
  quest_accepted = true,
  quest_delivered = true,
  kills = true,
  item = true,
  money = true
}

-- 需要 `match.value` 的条件类型（其余类型只看 count / 当前状态）。
M.VALUE_KINDS = {
  action = true,
  action_prefix = true,
  enter_map = true,
  quest_accepted = true,
  quest_delivered = true,
  item = true
}

function M.total_days()
  return #M.days
end

function M.days_list()
  return M.days
end

-- 第 n 天的定义；越界返回 nil。
function M.day(n)
  local index = tonumber(n)
  if not index then
    return nil
  end
  index = math.floor(index)
  if index < 1 or index > #M.days then
    return nil
  end
  return M.days[index]
end

function M.milestone_def()
  return M.milestone
end

-- 校验入口（登记在 scripts/validate_data.lua；`jhyt_m23_data_validation` 会枚举到）。
-- 只做结构与交叉引用检查，不产生副作用。
function M.validate_all()
  local issues = {}
  local function fail(message)
    issues[#issues + 1] = message
  end

  if type(M.days) ~= "table" or #M.days == 0 then
    fail("data.goals.days 不能为空")
    return #issues == 0, issues
  end
  if #M.days ~= 7 then
    fail("data.goals.days 必须是 7 天，实际 " .. tostring(#M.days))
  end

  local seen_day, seen_id = {}, {}
  for index, day in ipairs(M.days) do
    local where = "days[" .. tostring(index) .. "]"
    if type(day) ~= "table" then
      fail(where .. " 必须是表")
    else
      if tonumber(day.day) ~= index then
        fail(where .. ".day 必须等于序号 " .. tostring(index) .. "（实际 " .. tostring(day.day) .. "）")
      end
      if seen_day[day.day] then
        fail(where .. ".day 重复：" .. tostring(day.day))
      end
      seen_day[day.day] = true
      if type(day.title) ~= "string" or day.title == "" then
        fail(where .. ".title 必须是非空字符串")
      end
      if day.hint ~= nil and type(day.hint) ~= "string" then
        fail(where .. ".hint 必须是字符串")
      end
      local goals = day.goals
      if type(goals) ~= "table" or #goals == 0 then
        fail(where .. ".goals 不能为空")
      else
        for gi, goal in ipairs(goals) do
          local gwhere = where .. ".goals[" .. tostring(gi) .. "]"
          if type(goal) ~= "table" then
            fail(gwhere .. " 必须是表")
          else
            if type(goal.id) ~= "string" or goal.id == "" then
              fail(gwhere .. ".id 必须是非空字符串")
            elseif seen_id[goal.id] then
              fail(gwhere .. ".id 重复（存档变量按 id 命名，必须全局唯一）：" .. tostring(goal.id))
            else
              seen_id[goal.id] = true
            end
            if type(goal.text) ~= "string" or goal.text == "" then
              fail(gwhere .. ".text 必须是非空字符串")
            end
            if goal.count ~= nil then
              local count = tonumber(goal.count)
              if not count or count < 1 or count ~= math.floor(count) then
                fail(gwhere .. ".count 必须是正整数")
              end
            end
            local match = goal.match
            if type(match) ~= "table" then
              fail(gwhere .. ".match 必须是表")
            else
              local kind = match.kind
              if not M.KINDS[kind] then
                fail(gwhere .. ".match.kind 非法：" .. tostring(kind))
              elseif M.VALUE_KINDS[kind] and (type(match.value) ~= "string" or match.value == "") then
                fail(gwhere .. ".match.value 必须是非空字符串（kind=" .. tostring(kind) .. "）")
              end
            end
          end
        end
        -- 结构不变量：每天最后一条必须是跨天目标，否则当天不会结算。
        local last = goals[#goals]
        if type(last) == "table" and (type(last.match) ~= "table" or last.match.kind ~= "day_start") then
          fail(where .. " 的最后一条目标必须是 day_start（日结算挂在该事件上）")
        end
      end
      local reward = day.reward
      if reward ~= nil then
        if type(reward) ~= "table" then
          fail(where .. ".reward 必须是表")
        else
          local kinds = 0
          if reward.item ~= nil then
            kinds = kinds + 1
            if type(reward.item) ~= "string" or reward.item == "" then
              fail(where .. ".reward.item 必须是非空字符串")
            else
              local ok_items, items = pcall(require, "data.items")
              local ok_tools, tools = pcall(require, "data.tools")
              local known = (ok_items and type(items) == "table" and items[reward.item] ~= nil)
                or (ok_tools and type(tools) == "table" and tools[reward.item] ~= nil)
              if not known then
                fail(where .. ".reward.item 不在 data.items/data.tools：" .. tostring(reward.item))
              end
            end
            local count = tonumber(reward.count)
            if not count or count < 1 or count ~= math.floor(count) then
              fail(where .. ".reward.count 必须是正整数")
            end
          end
          if reward.money ~= nil then
            kinds = kinds + 1
            local amount = tonumber(reward.money)
            if not amount or amount <= 0 then
              fail(where .. ".reward.money 必须为正数")
            end
          end
          if reward.inner_power ~= nil then
            kinds = kinds + 1
            local amount = tonumber(reward.inner_power)
            if not amount or amount <= 0 then
              fail(where .. ".reward.inner_power 必须为正数")
            end
          end
          if kinds ~= 1 then
            fail(where .. ".reward 必须且只能给出 item/money/inner_power 之一")
          end
        end
      end
    end
  end

  local milestone = M.milestone
  if type(milestone) ~= "table" then
    fail("data.goals.milestone 必须是表")
  else
    if type(milestone.title) ~= "string" or milestone.title == "" then
      fail("data.goals.milestone.title 必须是非空字符串")
    end
    local amount = tonumber(milestone.money)
    if not amount or amount <= 0 then
      fail("data.goals.milestone.money 必须为正数")
    end
  end

  return #issues == 0, issues
end

return M
