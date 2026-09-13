-- data/appearances.lua：《江湖有田》主角**形象系统**的可选形象表。
--
-- 形象本体（序列帧、单帧、clip 名）在 data/characters.lua（由
-- tools/gen_character_sprites.py 生成）；本表只回答"玩家能选哪些、怎么称呼、
-- 什么时候解锁、默认是哪个"。这样加新形象 = 生成器加一个角色 + 这里加一行。
--
-- 字段：
--   id           引用 data/characters.lua 里 role="player" 的角色 id
--   name         中文显示名（暂停菜单/创建角色界面直接显示）
--   note         一句话说明（给玩家看的设计意图）
--   unlock       解锁条件："default" = 开局可用；"prologue" = 第一章后；
--                "coin:<n>" = 花费 n 文（预留给后续"易容/换装"玩法）
--   default      默认形象（全表只能有一个）
--
-- 与存档的关系：玩家当前形象存 state.appearance（随 save_manager 的 "state"
-- provider 落盘），读档后由 player.apply() 重新挂到 JianghuPlayer 上。

-- characters 是纯数据模块（不反向依赖本表），直接 require 即可；
-- validate_all 需要它做"形象 id 必须存在且 role=player"的交叉校验。
local characters = require("data.characters")

local M = {}

M.version = 1

M.appearances = {
  player_male = {
    name = "青衫少侠",
    note = "素色交领短打 + 竹青护腕束腰，背负木剑。默认形象。",
    unlock = "default",
    default = true
  },
  player_female = {
    name = "青衣女侠",
    note = "同款配色、不同轮廓（发髻/衣摆），动作帧数与男装完全一致。",
    unlock = "default"
  }
}

-- 默认形象 id（无 default 标记时退回第一个）。
function M.default_id()
  local first
  for id, entry in pairs(M.appearances) do
    if first == nil then
      first = id
    end
    if entry.default then
      return id
    end
  end
  return first
end

-- 稳定顺序列表（按 id 升序），供 UI 与验收遍历。
function M.list()
  local out = {}
  for id, entry in pairs(M.appearances) do
    out[#out + 1] = {
      id = id,
      name = entry.name,
      note = entry.note,
      unlock = entry.unlock,
      default = entry.default == true
    }
  end
  table.sort(out, function(a, b)
    return a.id < b.id
  end)
  return out
end

function M.get(id)
  return M.appearances[id]
end

-- 按顺序取下一个形象（暂停菜单"形象"项用它轮换）。
function M.next_id(current)
  local ordered = M.list()
  if #ordered == 0 then
    return nil
  end
  for index, entry in ipairs(ordered) do
    if entry.id == current then
      return ordered[index % #ordered + 1].id
    end
  end
  return ordered[1].id
end

-- 校验：id 必须在 data/characters.lua 里存在且 role="player"。
-- 参数可选（默认用 data.characters），validate_data 用无参形式统一调用。
function M.validate_all(characters_module)
  local art = characters_module or characters
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/appearances.lua: version: unsupported version " .. tostring(M.version)
  end
  local defaults = 0
  local count = 0
  for id, entry in pairs(M.appearances) do
    count = count + 1
    if type(id) ~= "string" or id == "" then
      issues[#issues + 1] = "data/appearances.lua: 形象 id 必须是非空字符串"
    end
    if type(entry.name) ~= "string" or entry.name == "" then
      issues[#issues + 1] = "data/appearances.lua:" .. tostring(id) .. ".name: required non-empty string"
    end
    if type(entry.unlock) ~= "string" or entry.unlock == "" then
      issues[#issues + 1] = "data/appearances.lua:" .. tostring(id) .. ".unlock: required non-empty string"
    end
    if entry.default then
      defaults = defaults + 1
    end
    if art then
      local character = art.get and art.get(id) or nil
      if not character then
        issues[#issues + 1] = "data/appearances.lua:" .. tostring(id)
          .. ": 在 data/characters.lua 里找不到该角色"
      elseif character.role ~= "player" then
        issues[#issues + 1] = "data/appearances.lua:" .. tostring(id)
          .. ".role: 期望 player，实际 " .. tostring(character.role)
      end
    end
  end
  if count == 0 then
    issues[#issues + 1] = "data/appearances.lua: 至少需要一个可选形象"
  end
  if defaults > 1 then
    issues[#issues + 1] = "data/appearances.lua: default 只能有一个（当前 " .. defaults .. " 个）"
  end
  return #issues == 0, issues
end

M.validate = M.validate_all

return M
