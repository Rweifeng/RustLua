-- data/npcs.lua：《江湖有田》NPC 注册表（M17-02 纯数据）。
--
-- 新增一个 NPC = 本文件加一条记录 + data/schedules/<id>.lua 一个日程文件，
-- 无需改动 npc.lua / schedule.lua / Rust 核心（验收项①）。
--
-- 字段：
--   name       显示名（诊断文案）
--   persist_id 稳定 PersistId：切图/重载不复制实体的唯一依据
--   home       兜底地点名（引用 data/places.lua），日程缺失时落位
--   sprite     SpriteComponent 字符串（texture=__white__ 内建白贴图 + tint；sorting_layer=actor）
--   speed      逐帧 transform 移动速度（世界单位/秒）
--   schedule   日程 id → data/schedules/<id>.lua
--   faction    所属声望阵营（引用 data/relationships.lua 的 factions，M34-01）：
--              商店按**店主阵营**的声望档位给折扣，因此这一字段是折扣的归属依据；
--              可为 nil（无阵营：不参与声望折扣），未知阵营由校验器报错
--
-- 表顺序无关；npc.lua 按 persist_id 稳定排序遍历，诊断/存档结果确定。
local M = {}
local places = require("data.places")

M.version = 1
M.npcs = {
  -- 云泽村
  {
    name = "沈青萝",
    persist_id = "npc_shen_qingluo",
    character = "shen_qingluo",
    home = "village_home_west",
    sprite = "texture=__white__;size=22x22;tint=0.86,0.42,0.55,1.0;sorting_layer=actor",
    speed = 48,
    schedule = "shen_qingluo",
    faction = "village"
  },
  {
    name = "赵铁算盘",
    persist_id = "npc_zhao_tiesuanpan",
    character = "zhao_tiesuanpan",
    home = "village_home_south",
    sprite = "texture=__white__;size=22x22;tint=0.85,0.68,0.25,1.0;sorting_layer=actor",
    speed = 44,
    schedule = "zhao_tiesuanpan",
    faction = "village"
  },
  {
    name = "林小满",
    persist_id = "npc_lin_xiaoman",
    character = "lin_xiaoman",
    home = "village_home_north",
    sprite = "texture=__white__;size=22x22;tint=0.42,0.72,0.45,1.0;sorting_layer=actor",
    speed = 44,
    schedule = "lin_xiaoman",
    faction = "village"
  },
  {
    name = "韩铁山",
    persist_id = "npc_han_tieshan",
    character = "han_tieshan",
    home = "village_home_forge",
    sprite = "texture=__white__;size=24x24;tint=0.55,0.55,0.62,1.0;sorting_layer=actor",
    speed = 40,
    schedule = "han_tieshan",
    faction = "village"
  },
  -- 山庄
  {
    name = "顾长风",
    persist_id = "npc_gu_changfeng",
    character = "gu_changfeng",
    home = "manor_home",
    sprite = "texture=__white__;size=24x24;tint=0.28,0.45,0.62,1.0;sorting_layer=actor",
    speed = 42,
    schedule = "gu_changfeng",
    faction = "sect"
  },
  {
    name = "苏晚晴",
    persist_id = "npc_su_wanqing",
    character = "su_wanqing",
    home = "manor_home_east",
    sprite = "texture=__white__;size=22x22;tint=0.55,0.72,0.85,1.0;sorting_layer=actor",
    speed = 52,
    schedule = "su_wanqing",
    faction = "sect"
  },
  -- 云游（跨图：山庄 <-> 云泽村）
  {
    name = "玄鹤",
    persist_id = "npc_xuanhe",
    character = "xuanhe",
    home = "manor_home",
    sprite = "texture=__white__;size=22x22;tint=0.92,0.92,0.96,1.0;sorting_layer=actor",
    speed = 56,
    schedule = "xuanhe",
    faction = "sect"
  },
  -- 批次 4：首发 NPC 扩充（docs/30 §4.2）
  {
    name = "阿七",
    persist_id = "npc_a_qi",
    character = "a_qi",
    home = "village_plaza",
    sprite = "texture=__white__;size=22x22;tint=0.70,0.35,0.24,1.0;sorting_layer=actor",
    speed = 56,
    schedule = "a_qi",
    faction = "village"
  },
  -- 司徒魁是玄冥会执事（敌对立场）：MVP 既无玄冥会阵营，也没有他的产业，
  -- 故 faction 留空 = 不参与任何声望折扣。
  {
    name = "司徒魁",
    persist_id = "npc_situ_kui",
    character = "situ_kui",
    home = "village_alley",
    sprite = "texture=__white__;size=24x24;tint=0.16,0.14,0.19,1.0;sorting_layer=actor",
    speed = 38,
    schedule = "situ_kui"
  },
  {
    name = "陆归藏",
    persist_id = "npc_lu_guizang",
    character = "lu_guizang",
    home = "manor_study",
    sprite = "texture=__white__;size=24x24;tint=0.43,0.48,0.45,1.0;sorting_layer=actor",
    speed = 34,
    schedule = "lu_guizang",
    faction = "sect"
  }
}

function M.get(persist_id)
  for _, npc in ipairs(M.npcs) do
    if npc.persist_id == persist_id then
      return npc
    end
  end
  return nil
end

-- 稳定排序的列表（按 persist_id 升序），供 npc.lua 遍历/存档。
function M.list()
  local list = {}
  for _, npc in ipairs(M.npcs) do
    list[#list + 1] = npc
  end
  table.sort(list, function(a, b)
    return a.persist_id < b.persist_id
  end)
  return list
end

function M.validate_all()
  local issues = {}
  local seen = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/npcs.lua: version: unsupported version " .. tostring(M.version)
  end
  for index, npc in ipairs(M.npcs) do
    local path = "data/npcs.lua:npcs[" .. tostring(index) .. "]"
    if type(npc) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      local id = npc.persist_id
      if type(id) ~= "string" or id == "" then
        issues[#issues + 1] = path .. ".persist_id: required non-empty string"
      elseif seen[id] then
        issues[#issues + 1] = path .. ".persist_id: duplicate object id " .. id
      else
        seen[id] = true
      end
      if type(npc.name) ~= "string" or npc.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if type(npc.home) ~= "string" or not require("data.places").get(npc.home) then
        issues[#issues + 1] = path .. ".home: unknown place " .. tostring(npc.home)
      end
      if type(npc.schedule) ~= "string" or npc.schedule == "" then
        issues[#issues + 1] = path .. ".schedule: required non-empty string"
      else
        local ok, schedule = pcall(require, "data.schedules." .. npc.schedule)
        if not ok or type(schedule) ~= "table" then
          issues[#issues + 1] = path .. ".schedule: unknown schedule " .. npc.schedule
        elseif schedule.version ~= 1 then
          issues[#issues + 1] = "data/schedules/" .. npc.schedule .. ".lua:version: unsupported version " .. tostring(schedule.version)
        end
      end
      if type(npc.speed) ~= "number" or npc.speed <= 0 then
        issues[#issues + 1] = path .. ".speed: expected positive number"
      end
    end
  end
  return #issues == 0, issues
end

return M
