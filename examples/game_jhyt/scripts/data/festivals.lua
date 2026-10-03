-- data/festivals.lua：《江湖有田》节气事件注册表（M34-02 纯数据）。
--
-- 口径来自 docs/30 §12 的裁定：**不做完整中国农历算法**，节气先作为事件标签。
-- 于是"今天是什么节气"是 `day` 的**纯函数**（见 world/calendar.lua 的 M.jieqi），
-- 既不查随机数、也不入存档——存档只写分数与锚点，节气随日数一起回来。
--
-- 字段：
--   id/name      稳定标识 / 玩家看到的中文称号（docs/30 §12 的七条）
--   season       所属季节（spring/summer/autumn/winter，与 world/calendar.lua 同源）
--   from/to      季内第几天开始生效（闭区间，1..season_length；一次事件持续几天）
--   effects      效果表，只允许 effect_keys 里的键；**空表就是"今天只有一句对话"**
--   unlock       可选（M34-05）：这场事件的门槛 { building = "<建筑 id>", level = N }——
--                该建筑**建到过** N 级才听得到（读 systems/building.lua 记进 state.vars 的
--                历史最高等级，见 data/buildings.lua 的 M.level_var）。
--                写在事件这一侧而不是建筑的等级行里："春分茶会要几级茶棚"是这条事件的属性，
--                建筑只管自己有几级；反过来写就得扫描全部建筑才知道门在谁身上。
--                world/festival.lua 的 M.available() 是唯一消费者（对话条件走它）。
--
-- 效果键与消费方（新增键必须同时有消费方，否则 validate_all 报错——与
-- data/relationships.lua 的 price_off 同一规矩）：
--   gather        systems/gather.lua：`mult` = 该图采集数量整数倍；
--                 `one_time_daily` = 一次性节点（respawn_days <= 0）在此期间按日再刷新
--   sell_bonus    systems/shop.lua：出售这些物品时按 `bonus` 百分比加价（药材行情）
--   exit_closed   world/map_manager.lua：列出的出口 id 在此期间**按 E 不能走**
--
-- 为什么 春分/夏至/中秋 三条 effects 是空的：它们的出口是对话分支
-- （`cond = { festival = "<id>" }`，见 data/dialogues/*.lua），加分/加声望/发订单
-- 都写在对话数据里，不需要第二套效果表。
local M = {}

M.version = 1

-- 每季天数：world/calendar.lua 的换季长度读这个值（改这里 = 改整条时间轴，
-- 四季循环与 farm 的播种季节日一起动，故不要单独调）。
M.season_length = 28

M.seasons = { spring = true, summer = true, autumn = true, winter = true }

M.effect_keys = { gather = true, sell_bonus = true, exit_closed = true }

M.festivals = {
  {
    id = "teafair",
    name = "春分茶会",
    season = "spring",
    from = 8,
    to = 10,
    -- 春分是七条里唯一带**订单**的：沈青萝在这三天才会提起茶叶单子（order_tea）。
    effects = {},
    -- 会期之外还要过一道设施门（docs/33 §7.2 茶棚第三档"解锁节气事件"，M34-05）：
    -- 茶棚没建到 3 级，这三天里也听不到这单生意。
    unlock = { building = "tea_shed", level = 3 }
  },
  {
    id = "qingming",
    name = "清明踏青",
    season = "spring",
    from = 15,
    to = 17,
    -- 山道采集收益翻倍（docs/30 §12"清明踏青 | 山道采集收益提高"）。
    effects = { gather = { maps = { "mountain_path" }, mult = 2 } }
  },
  {
    id = "duanwu",
    name = "端午药集",
    season = "summer",
    from = 8,
    to = 10,
    -- 药材行情：这四味是三家店 buys 里全部的药材（lingzhi/herb/polygonum/ginseng）。
    effects = { sell_bonus = { items = { "lingzhi", "herb", "polygonum", "ginseng" }, bonus = 20 } }
  },
  {
    id = "xiazhi",
    name = "夏至演武",
    season = "summer",
    from = 15,
    to = 17,
    effects = {}
  },
  {
    id = "midautumn",
    name = "中秋灯会",
    season = "autumn",
    from = 8,
    to = 10,
    effects = {}
  },
  {
    id = "chongyang",
    name = "重阳登高",
    season = "autumn",
    from = 15,
    to = 17,
    -- 山区稀有资源刷新：一次性点（野果/柴堆/矿石/山庄药草）在这三天按日再刷。
    effects = { gather = { maps = { "mountain_path" }, one_time_daily = true } }
  },
  {
    id = "dongzhi",
    name = "冬至封山",
    season = "winter",
    from = 15,
    to = 17,
    -- 只封"继续往山里走"的那一格（山道 → 寒泉洞）；回村的方向必须始终可用，
    -- 否则玩家会被关在山上（M34-02 计划风险项里点名的一条）。
    effects = { exit_closed = { "mountain_to_cave" } }
  }
}

--- 节气 id 的升序列表（校验器报错文案里的"已知节气"提示，与 relationships.tier_keys 同形）。
function M.id_keys()
  local out = {}
  for _, entry in ipairs(M.festivals) do
    if type(entry.id) == "string" then
      out[#out + 1] = entry.id
    end
  end
  table.sort(out)
  return out
end

function M.get(id)
  for _, entry in ipairs(M.festivals) do
    if entry.id == id then
      return entry
    end
  end
  return nil
end

--- 给定季节与季内第几天（1..season_length）落在哪个节气；不在任何窗口里返回 nil。
--- 纯查表：同一 (season, day) 永远给同一个答案，headless 验收可以直接钉死取值。
function M.at(season, day_in_season)
  if type(season) ~= "string" or type(day_in_season) ~= "number" then
    return nil
  end
  for _, entry in ipairs(M.festivals) do
    if entry.season == season and day_in_season >= entry.from and day_in_season <= entry.to then
      return entry
    end
  end
  return nil
end

function M.validate_all()
  local issues = {}
  local function is_integer(value)
    return type(value) == "number" and value == math.floor(value)
  end
  if M.version ~= 1 then
    issues[#issues + 1] = "data/festivals.lua:version: unsupported version " .. tostring(M.version)
  end
  if not is_integer(M.season_length) or (M.season_length or 0) < 2 then
    issues[#issues + 1] = "data/festivals.lua:season_length: expected integer >= 2"
  end
  if type(M.festivals) ~= "table" or next(M.festivals) == nil then
    issues[#issues + 1] = "data/festivals.lua:festivals: expected non-empty array"
    return false, issues
  end
  local maps = require("data.maps")
  local items = require("data.items")
  local buildings = require("data.buildings")
  local seen_ids = {}
  local occupied = {}
  for index, entry in ipairs(M.festivals) do
    local path = "data/festivals.lua:festivals[" .. tostring(index) .. "]"
    if type(entry) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      local id = entry.id
      if type(id) ~= "string" or id == "" then
        issues[#issues + 1] = path .. ".id: required non-empty string"
      elseif seen_ids[id] then
        issues[#issues + 1] = path .. ".id: duplicate object id " .. tostring(id)
      else
        seen_ids[id] = true
      end
      if type(entry.name) ~= "string" or entry.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if not M.seasons[entry.season] then
        issues[#issues + 1] = path .. ".season: unknown season " .. tostring(entry.season)
      end
      local from = entry.from
      local to = entry.to
      local length = tonumber(M.season_length) or 0
      if not is_integer(from) or from < 1 or from > length then
        issues[#issues + 1] = path .. ".from: expected integer within 1.." .. tostring(length)
      end
      if not is_integer(to) or to < 1 or to > length then
        issues[#issues + 1] = path .. ".to: expected integer within 1.." .. tostring(length)
      end
      if is_integer(from) and is_integer(to) and from > to then
        issues[#issues + 1] = path .. ": from must not exceed to"
      end
      if M.seasons[entry.season] and is_integer(from) and is_integer(to) then
        for day = from, to do
          local key = entry.season .. ":" .. tostring(day)
          if occupied[key] then
            issues[#issues + 1] = path
              .. ": 季内第 " .. tostring(day) .. " 天已被「" .. tostring(occupied[key])
              .. "」占用，同一天不能有两个节气事件"
          else
            occupied[key] = entry.name or entry.id
          end
        end
      end
      if type(entry.effects) ~= "table" then
        issues[#issues + 1] = path .. ".effects: expected table (empty table is fine)"
      else
        for key, value in pairs(entry.effects) do
          if not M.effect_keys[key] then
            issues[#issues + 1] = path .. ".effects." .. tostring(key) .. ": unknown effect key"
          else
            M.validate_effect(path, key, value, issues, maps, items)
          end
        end
      end
      -- 设施门槛（M34-05）：引用不存在的建筑、或写一个那栋建筑根本升不到的等级，
      -- 后果是这场事件**永远**听不到——不报出来就只会变成一条谁也查不到的死条件。
      if entry.unlock ~= nil then
        local unlock = entry.unlock
        if type(unlock) ~= "table" then
          issues[#issues + 1] = path .. ".unlock: expected object { building, level }"
        else
          for key in pairs(unlock) do
            if key ~= "building" and key ~= "level" then
              issues[#issues + 1] = path .. ".unlock." .. tostring(key) .. ": unknown unlock key"
            end
          end
          local def = buildings.get(unlock.building)
          if not def then
            issues[#issues + 1] = path .. ".unlock.building: unknown building "
              .. tostring(unlock.building)
          elseif not is_integer(unlock.level) or unlock.level < 1 then
            issues[#issues + 1] = path .. ".unlock.level: expected integer >= 1"
          elseif unlock.level > buildings.max_level(def) then
            issues[#issues + 1] = path .. ".unlock.level: " .. tostring(unlock.level)
              .. " exceeds max level " .. tostring(buildings.max_level(def))
              .. " of building " .. tostring(unlock.building)
          end
        end
      end
    end
  end
  local covered = {}
  for _, entry in ipairs(M.festivals) do
    if M.seasons[entry.season] then
      covered[entry.season] = true
    end
  end
  for season in pairs(M.seasons) do
    if not covered[season] then
      issues[#issues + 1] = "data/festivals.lua:festivals: season " .. season
        .. " has no festival event（四季每季至少要有一个节气事件）"
    end
  end
  return #issues == 0, issues
end

--- 单个效果键的结构校验。拆成模块方法是为了让负例用例能单独调用它
--- （jhyt_m34_festival 的 ⑥ 章要逐键构造非法数据）。
function M.validate_effect(path, key, value, issues, maps, items)
  local function is_integer(v)
    return type(v) == "number" and v == math.floor(v)
  end
  if key == "gather" then
    if type(value) ~= "table" then
      issues[#issues + 1] = path .. ".effects.gather: expected table"
      return
    end
    local list = value.maps
    if type(list) ~= "table" or #list < 1 then
      issues[#issues + 1] = path .. ".effects.gather.maps: expected non-empty array of map ids"
    else
      for _, map_id in ipairs(list) do
        if not maps.get(map_id) then
          issues[#issues + 1] = path .. ".effects.gather.maps: unknown map " .. tostring(map_id)
        end
      end
    end
    if value.mult ~= nil and (not is_integer(value.mult) or value.mult < 2) then
      issues[#issues + 1] = path .. ".effects.gather.mult: expected integer >= 2"
    end
    if value.one_time_daily ~= nil and value.one_time_daily ~= true then
      issues[#issues + 1] = path .. ".effects.gather.one_time_daily: expected true or absent"
    end
    if value.mult == nil and value.one_time_daily == nil then
      issues[#issues + 1] = path .. ".effects.gather: needs either mult or one_time_daily"
    end
  elseif key == "sell_bonus" then
    if type(value) ~= "table" then
      issues[#issues + 1] = path .. ".effects.sell_bonus: expected table"
      return
    end
    local list = value.items
    if type(list) ~= "table" or #list < 1 then
      issues[#issues + 1] = path .. ".effects.sell_bonus.items: expected non-empty array of item ids"
    else
      for _, item_id in ipairs(list) do
        local def = items[item_id]
        if type(def) ~= "table" then
          issues[#issues + 1] = path .. ".effects.sell_bonus.items: unknown item " .. tostring(item_id)
        elseif (tonumber(def.sell) or 0) <= 0 then
          issues[#issues + 1] = path .. ".effects.sell_bonus.items: " .. tostring(item_id)
            .. " has no sell price to bonus (+20% of 0 is 0)"
        end
      end
    end
    if not is_integer(value.bonus) or value.bonus < 1 or value.bonus > 100 then
      issues[#issues + 1] = path .. ".effects.sell_bonus.bonus: expected integer within 1..100"
    end
  elseif key == "exit_closed" then
    if type(value) ~= "table" or #value < 1 then
      issues[#issues + 1] = path .. ".effects.exit_closed: expected non-empty array of exit ids"
    else
      for _, exit_id in ipairs(value) do
        if not maps.exits[exit_id] then
          issues[#issues + 1] = path .. ".effects.exit_closed: unknown exit " .. tostring(exit_id)
        end
      end
    end
  end
end

return M
