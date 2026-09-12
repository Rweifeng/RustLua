-- data/enemies.lua：《江湖有田》敌人注册表（M18-01 纯数据）。
--
-- 新增一个敌人 = 本文件加一条记录，无需改动 enemy.lua / combat.lua /
-- Rust 核心（与 data/npcs.lua 同约定）。
--
-- 字段：
--   name            显示名（战斗日志/诊断）
--   persist_id      稳定 PersistId：切图/重载按此去重重建（仿 NPC）
--   map             所属地图 id（引用 data/maps.lua）
--   home            家格 {x, y}（cell 0-index、y 朝上；出生/重置落位）
--   sprite          SpriteComponent 字符串（texture=__white__ + tint；sorting_layer=actor）
--   hp              最大/初始气血
--   speed           追击移动速度（世界单位/秒；0 = 不移动，如训练木人）
--   damage          每次攻击伤害（0 = 不攻击）
--   aggro_radius    索敌半径（世界单位；0 = 永不索敌）
--   attack_range    近战攻击距离（世界单位；0 = 不攻击）
--   attack_cooldown 两次攻击最小间隔（固定步数）
--   skill           攻击招式 id（引用 data/skills.lua；hitbox 形状/窗口取自招式）
--
-- 表顺序无关；enemy.lua 按 persist_id 稳定排序遍历。
local M = {}

M.version = 1

M.enemies = {
  -- 演武场：不索敌不移动的训练靶（命中/去重/装备加成的确定性验收对象）。
  {
    name = "训练木人",
    persist_id = "enemy_dummy",
    map = "combat_arena",
    home = { x = 14, y = 5 },
    sprite = "texture=__white__;size=22x22;tint=0.72,0.45,0.30,1.0;sorting_layer=actor",
    hp = 30,
    speed = 0,
    damage = 0,
    aggro_radius = 0,
    attack_range = 0,
    attack_cooldown = 0,
    skill = "strike"
  },
  -- 演武场：会追击与攻击的灰狼（A* 追击/受击/战败验收对象）。
  {
    name = "灰狼",
    persist_id = "enemy_wolf",
    map = "combat_arena",
    home = { x = 2, y = 9 },
    sprite = "texture=__white__;size=24x18;tint=0.55,0.58,0.62,1.0;sorting_layer=actor",
    hp = 20,
    speed = 64,
    damage = 3,
    aggro_radius = 240,
    attack_range = 34,
    attack_cooldown = 72,
    skill = "strike"
  }
}

function M.get(persist_id)
  for _, enemy in ipairs(M.enemies) do
    if enemy.persist_id == persist_id then
      return enemy
    end
  end
  return nil
end

-- 稳定排序的列表（按 persist_id 升序），供 enemy.lua 遍历/存档。
function M.list()
  local list = {}
  for _, enemy in ipairs(M.enemies) do
    list[#list + 1] = enemy
  end
  table.sort(list, function(a, b)
    return a.persist_id < b.persist_id
  end)
  return list
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/enemies.lua: version: unsupported version " .. tostring(M.version)
  end
  local maps = require("data.maps")
  local skills = require("data.skills")
  local seen = {}
  for index, enemy in ipairs(M.enemies) do
    local path = "data/enemies.lua:enemies[" .. tostring(index) .. "]"
    if type(enemy) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      local id = enemy.persist_id
      if type(id) ~= "string" or id == "" then
        issues[#issues + 1] = path .. ".persist_id: required non-empty string"
      elseif seen[id] then
        issues[#issues + 1] = path .. ".persist_id: duplicate object id " .. id
      else
        seen[id] = true
      end
      if type(enemy.name) ~= "string" or enemy.name == "" then
        issues[#issues + 1] = path .. ".name: required non-empty string"
      end
      if type(enemy.map) ~= "string" or not maps.get(enemy.map) then
        issues[#issues + 1] = path .. ".map: unknown map " .. tostring(enemy.map)
      end
      local home = enemy.home
      if type(home) ~= "table" or type(home.x) ~= "number" or type(home.y) ~= "number" then
        issues[#issues + 1] = path .. ".home: expected {x, y} numbers"
      elseif home.x < 0 or home.x >= 40 or home.y < 0 or home.y >= 22 then
        issues[#issues + 1] = path .. ".home: out of map bounds"
      end
      if type(enemy.sprite) ~= "string" or enemy.sprite == "" then
        issues[#issues + 1] = path .. ".sprite: required non-empty string"
      end
      if type(enemy.hp) ~= "number" or enemy.hp <= 0 then
        issues[#issues + 1] = path .. ".hp: expected positive number"
      end
      for _, field in ipairs({ "speed", "damage", "aggro_radius", "attack_range", "attack_cooldown" }) do
        local value = tonumber(enemy[field])
        if not value or value < 0 then
          issues[#issues + 1] = path .. "." .. field .. ": expected non-negative number"
        end
      end
      if type(enemy.skill) ~= "string" or not skills.get(enemy.skill) then
        issues[#issues + 1] = path .. ".skill: unknown skill " .. tostring(enemy.skill)
      end
    end
  end
  return #issues == 0, issues
end

return M
