-- data/solar_terms.lua：《江湖有田》二十四节气注册表（阶段3「节气历法」，docs/33 2.1）。
--
-- 口径（先读 docs/33 §2.1 时间日历后定）：每季 28 天、四季 112 天循环（与
-- data/festivals.lua 的 season_length 同源）。传统二十四节气在这里**每季取 6 个**
-- （立春→大寒，共 24 个），按季内第几天钉死；节气日**单日生效**（不像节日是 3 天窗口），
-- 所以"今天是什么节气"是 `day` 的纯函数（见 systems/solar_terms.lua 的 M.today），
-- 既不查随机数、也不入存档——存档只写分数与锚点，节气随日数一起回来。
--
-- 为什么节气日压在 8 个节日窗口的正中央（季内日 9/16 等）：阶段2 的节日（春分茶会
-- 春 8-10、清明踏青 春 15-17、……、除夕守岁 冬 26-28）已落地且**不得改动**；
-- 节气是节日的"天文原点"，把春分钉在春 9（茶会窗口正中）、冬至钉在冬 16（封山窗口正中），
-- 节日就成了"围绕该节气的那几天活动"，两套时间轴互不打架。8 个节日与节气的对应关系
-- 只读地写在每条的 festival_link 上（systems/solar_terms.lua 提供查询）。
--
-- 字段：
--   id/name       稳定标识 / 玩家看到的中文名（二十四节气原名）
--   season        所属季节（spring/summer/autumn/winter）
--   day_in_season 季内第几天（1..season_length）：该节气**仅当天**生效
--   modifiers     季节修饰表：只允许 modifier_keys 里的键；空表 = 该节气无加成。
--                 这些修饰目前是**纯查询面**（阶段3）：gather/shop 的当日加成仍由
--                 world/festival.lua 的 effects 表消费（不改 gather/shop），节气修饰
--                 先以数据 + 查询接口落地，待 Organizer 接线后再被真实采集/出货消费。
--   festival_link 可选：这个节气是哪个阶段2 节日（data/festivals.lua 的 id）的天文原点。
--                 只读对照，不改 festivals.lua。
--
-- 修饰键白名单与语义（modifier_keys）：
--   gather_mult    采集数量整数倍（1 = 无加成；对齐 festival effects.gather.mult）
--   crop_bonus     作物生长加成倍数（1 = 无加成；纯查询面，farm 未消费）
--   fish_bonus     钓鱼加成倍数（1 = 无加成；纯查询面，fishing 未消费）
--   sell_bonus     出货加价百分比（0 = 无加成；对齐 festival effects.sell_bonus.bonus）
--   energy_bonus   当日体力恢复加成（0 = 无加成；纯查询面）
local M = {}

M.version = 1

-- 与 data/festivals.lua 同源：季长只写一处（改季长时节日与节气一起动）。
M.season_length = require("data.festivals").season_length

M.seasons = { spring = true, summer = true, autumn = true, winter = true }

-- 修饰键白名单：mult 类（倍数，整数 >= 1）/ bonus 类（百分比或点数，整数 >= 0）。
M.modifier_keys = {
  gather_mult = "mult",
  crop_bonus = "mult",
  fish_bonus = "mult",
  sell_bonus = "bonus",
  energy_bonus = "bonus",
}

-- 8 个阶段2 节日 id 的闭集（仅用于 festival_link 校验，只读对照）。
M.festival_ids = {}
for _, f in ipairs(require("data.festivals").festivals) do
  M.festival_ids[f.id] = true
end

M.terms = {
  -- ---------------- 春 ----------------
  { id = "lichun",   name = "立春", season = "spring", day_in_season = 1,
    modifiers = { crop_bonus = 2 } },
  { id = "yushui",   name = "雨水", season = "spring", day_in_season = 4,
    modifiers = { energy_bonus = 5 } },
  { id = "jingzhe",  name = "惊蛰", season = "spring", day_in_season = 7,
    modifiers = { gather_mult = 2 } },
  { id = "chunfen",  name = "春分", season = "spring", day_in_season = 9,
    modifiers = { sell_bonus = 10 }, festival_link = "teafair" },
  { id = "qingming", name = "清明", season = "spring", day_in_season = 16,
    modifiers = { gather_mult = 2 }, festival_link = "qingming" },
  { id = "guyu",     name = "谷雨", season = "spring", day_in_season = 23,
    modifiers = { crop_bonus = 2 } },
  -- ---------------- 夏 ----------------
  { id = "lixia",    name = "立夏", season = "summer", day_in_season = 1,
    modifiers = { fish_bonus = 2 } },
  { id = "xiaoman",  name = "小满", season = "summer", day_in_season = 4,
    modifiers = { crop_bonus = 2 } },
  { id = "mangzhong", name = "芒种", season = "summer", day_in_season = 9,
    modifiers = { gather_mult = 2 }, festival_link = "duanwu" },
  { id = "xiazhi",   name = "夏至", season = "summer", day_in_season = 16,
    modifiers = { energy_bonus = 10 }, festival_link = "xiazhi" },
  { id = "xiaoshu",  name = "小暑", season = "summer", day_in_season = 21,
    modifiers = { fish_bonus = 2 } },
  { id = "dashu",    name = "大暑", season = "summer", day_in_season = 26,
    modifiers = { sell_bonus = 10 } },
  -- ---------------- 秋 ----------------
  { id = "liqiu",    name = "立秋", season = "autumn", day_in_season = 1,
    modifiers = { gather_mult = 2 } },
  { id = "chushu",   name = "处暑", season = "autumn", day_in_season = 4,
    modifiers = { energy_bonus = 5 } },
  { id = "bailu",    name = "白露", season = "autumn", day_in_season = 9,
    modifiers = { fish_bonus = 2 }, festival_link = "midautumn" },
  { id = "qiufen",   name = "秋分", season = "autumn", day_in_season = 13,
    modifiers = { sell_bonus = 10 } },
  { id = "hanlu",    name = "寒露", season = "autumn", day_in_season = 16,
    modifiers = { gather_mult = 2 }, festival_link = "chongyang" },
  { id = "shuangjiang", name = "霜降", season = "autumn", day_in_season = 23,
    modifiers = { energy_bonus = 5 } },
  -- ---------------- 冬 ----------------
  { id = "lidong",   name = "立冬", season = "winter", day_in_season = 1,
    modifiers = { energy_bonus = 5 } },
  { id = "xiaoxue",  name = "小雪", season = "winter", day_in_season = 4,
    modifiers = { sell_bonus = 5 } },
  { id = "daxue",    name = "大雪", season = "winter", day_in_season = 9,
    modifiers = { energy_bonus = 5 } },
  { id = "dongzhi",  name = "冬至", season = "winter", day_in_season = 16,
    modifiers = { energy_bonus = 10 }, festival_link = "dongzhi" },
  { id = "xiaohan",  name = "小寒", season = "winter", day_in_season = 21,
    modifiers = { sell_bonus = 5 } },
  { id = "dahan",    name = "大寒", season = "winter", day_in_season = 27,
    modifiers = { sell_bonus = 10 }, festival_link = "chuxi" },
}

--- 节气 id 的升序列表（校验报错文案提示用，与 festivals.id_keys 同形）。
function M.id_keys()
  local out = {}
  for _, entry in ipairs(M.terms) do
    if type(entry.id) == "string" then
      out[#out + 1] = entry.id
    end
  end
  table.sort(out)
  return out
end

function M.get(id)
  for _, entry in ipairs(M.terms) do
    if entry.id == id then
      return entry
    end
  end
  return nil
end

--- 给定季节与季内第几天（1..season_length）的节气条目；那天没有节气返回 nil。
--- 纯查表：同一 (season, day) 永远给同一个答案，headless 验收可直接钉死取值。
function M.at(season, day_in_season)
  if type(season) ~= "string" or type(day_in_season) ~= "number" then
    return nil
  end
  for _, entry in ipairs(M.terms) do
    if entry.season == season and entry.day_in_season == day_in_season then
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
    issues[#issues + 1] = "data/solar_terms.lua:version: unsupported version " .. tostring(M.version)
  end
  local length = tonumber(M.season_length) or 0
  local seen_ids = {}
  local occupied = {}
  for index, entry in ipairs(M.terms) do
    local path = "data/solar_terms.lua:terms[" .. tostring(index) .. "]"
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
      if not is_integer(entry.day_in_season)
        or entry.day_in_season < 1 or entry.day_in_season > length then
        issues[#issues + 1] = path .. ".day_in_season: expected integer within 1.."
          .. tostring(length)
      end
      -- 同一季节内同一天不能有两个节气（单日生效，撞日就是纯数据错误）。
      if M.seasons[entry.season] and is_integer(entry.day_in_season) then
        local key = entry.season .. ":" .. tostring(entry.day_in_season)
        if occupied[key] then
          issues[#issues + 1] = path .. ": 季内第 " .. tostring(entry.day_in_season)
            .. " 天已被「" .. tostring(occupied[key]) .. "」占用"
        else
          occupied[key] = entry.name or entry.id
        end
      end
      if type(entry.modifiers) ~= "table" then
        issues[#issues + 1] = path .. ".modifiers: expected table (empty table is fine)"
      else
        for key, value in pairs(entry.modifiers) do
          if not M.modifier_keys[key] then
            issues[#issues + 1] = path .. ".modifiers." .. tostring(key) .. ": unknown modifier key"
          else
            local kind = M.modifier_keys[key]
            if not is_integer(value) then
              issues[#issues + 1] = path .. ".modifiers." .. tostring(key) .. ": expected integer"
            elseif kind == "mult" and value < 1 then
              issues[#issues + 1] = path .. ".modifiers." .. tostring(key) .. ": mult must be >= 1"
            elseif kind == "bonus" and value < 0 then
              issues[#issues + 1] = path .. ".modifiers." .. tostring(key) .. ": bonus must be >= 0"
            end
          end
        end
      end
      if entry.festival_link ~= nil then
        if type(entry.festival_link) ~= "string" or not M.festival_ids[entry.festival_link] then
          issues[#issues + 1] = path .. ".festival_link: unknown festival id "
            .. tostring(entry.festival_link)
        end
      end
    end
  end
  -- 四季每季恰 6 个节气（24 节气每季均分）——少了说明漏排，多了说明重排。
  local per_season = {}
  for _, entry in ipairs(M.terms) do
    if type(entry) == "table" and M.seasons[entry.season] then
      per_season[entry.season] = (per_season[entry.season] or 0) + 1
    end
  end
  for season in pairs(M.seasons) do
    if per_season[season] ~= 6 then
      issues[#issues + 1] = "data/solar_terms.lua:terms: season " .. season
        .. " should have exactly 6 solar terms, got " .. tostring(per_season[season] or 0)
    end
  end
  return #issues == 0, issues
end

return M
