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
--   sprite          SpriteComponent 字符串（texture=assets/art/enemies/<id>/static_<dir>.png；
--                   sorting_layer=actor）。单帧图由 tools/gen_enemy_art.py 生成；
--                   同一目录下还有 1×N 动画条带，等引擎补 animation.attach 后可切换。
--   hp              最大/初始气血
--   speed           追击移动速度（世界单位/秒；0 = 不移动，如训练木人）
--   damage          每次攻击伤害（0 = 不攻击）
--   aggro_radius    索敌半径（世界单位；0 = 永不索敌）
--   attack_range    近战攻击距离（世界单位；0 = 不攻击）
--   attack_cooldown 两次攻击最小间隔（固定步数）
--   skill           攻击招式 id（引用 data/skills.lua；hitbox 形状/窗口取自招式）
--   knockback_resist 可选，0..1：被击退距离的抵抗系数（1 = 完全不动；默认取
--                   data/combat.lua 的 enemy.knockback_resist）
--   hitstun_resist   可选，0..1：被硬直步数的抵抗系数（1 = 完全不打断；同上默认）
--   status           可选，命中玩家后挂上的**瞬时状态**种类（引用 data/combat.lua 的
--                   status 表的键，如 "poison"）；步长与每跳伤害都在那张表里，
--                   敌人只声明"挂哪一种"。写了未知种类直接判错（打错字的状态是无声的）。
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
    -- art 与 sprite 指向同一套贴图（static_<方向>.png），art 额外挂上序列帧；
    -- M23 起 art 引用由 data/registry_check.lua 交叉校验（"生成了美术但没接线"
    -- 会被加载期校验挡住）。
    art = "training_dummy",
    sprite = "texture=assets/art/enemies/training_dummy/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 30,
    speed = 0,
    damage = 0,
    aggro_radius = 0,
    attack_range = 0,
    attack_cooldown = 0,
    skill = "strike",
    -- 木桩钉在地上：击退/硬直全免疫（M25）。练功房的价值就是"打它它不动"，
    -- 否则玩家会被自己的普攻推着满场追木人。
    knockback_resist = 1,
    hitstun_resist = 1
  },
  -- 演武场：会追击与攻击的灰狼（A* 追击/受击/战败验收对象）。
  {
    name = "灰狼",
    persist_id = "enemy_wolf",
    map = "combat_arena",
    home = { x = 2, y = 9 },
    art = "gray_wolf",
    sprite = "texture=assets/art/enemies/gray_wolf/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 20,
    speed = 64,
    damage = 3,
    aggro_radius = 240,
    attack_range = 34,
    attack_cooldown = 72,
    skill = "strike"
  },
  -- ---- 批次 2/4：青云山道与寒泉洞的敌人（docs/30 §8.4）----
  -- art 字段引用 data/enemy_art.lua 的形象与动画（由 tools/gen_enemy_art.py 生成）；
  -- enemy.lua 在 spawn 后用 `animation.attach` 挂上序列帧。
  -- 山贼喽啰：近战追击，低生命（第一章"在山道击败山贼"的目标）。
  {
    name = "山贼喽啰",
    persist_id = "enemy_bandit",
    map = "mountain_path",
    home = { x = 31, y = 6 },
    art = "bandit_grunt",
    sprite = "texture=assets/art/enemies/bandit_grunt/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 18,
    speed = 58,
    damage = 3,
    aggro_radius = 220,
    attack_range = 34,
    attack_cooldown = 78,
    skill = "strike"
  },
  -- 毒蝎：短距离突刺，带中毒。M34-03 起这句注释有了实现：命中玩家后挂 `status.poison`
  -- （600 步内每 120 步扣 3 点气血，口径 docs/33 §10 的"每 2 秒损失 3 生命 / 10 秒"）。
  {
    name = "毒蝎",
    persist_id = "enemy_scorpion",
    map = "mountain_path",
    home = { x = 12, y = 12 },
    art = "poison_scorpion",
    sprite = "texture=assets/art/enemies/poison_scorpion/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 12,
    speed = 44,
    damage = 2,
    aggro_radius = 150,
    attack_range = 28,
    attack_cooldown = 90,
    skill = "strike",
    -- 命中后给目标挂的瞬时状态种类（步长/伤害见 data/combat.lua 的 status 表）。
    status = "poison"
  },
  -- 玄冥会徒：格挡与投掷（当前用同一招式的更远判定近似）。
  {
    name = "玄冥会徒",
    persist_id = "enemy_xuanming",
    map = "mountain_path",
    home = { x = 18, y = 17 },
    art = "xuanming_adept",
    sprite = "texture=assets/art/enemies/xuanming_adept/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 26,
    speed = 50,
    damage = 4,
    aggro_radius = 260,
    attack_range = 56,
    attack_cooldown = 96,
    skill = "strike"
  },
  -- 野猪（M19-01）：青云山道的采集区游荡敌人。攻击低、血薄，
  -- 让「进山采集 → 遭遇 → 战斗」的 MVP 剧本可用练习剑两下解决。
  -- 形象暂借既有 gray_wolf 贴图（纯数据新增，不改 enemy.lua / combat.lua）。
  {
    name = "野猪",
    persist_id = "enemy_wild_boar",
    map = "mountain_path",
    home = { x = 21, y = 19 },
    sprite = "texture=assets/art/enemies/gray_wolf/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 12,
    speed = 56,
    damage = 3,
    aggro_radius = 200,
    attack_range = 34,
    attack_cooldown = 84,
    skill = "strike"
  },
  -- 山魈首领：第一章 Boss（厚血、重击；48×48 大体型）。
  {
    name = "山魈首领",
    persist_id = "enemy_ape_boss",
    map = "cold_spring_cave",
    home = { x = 10, y = 6 },
    art = "mountain_ape_boss",
    sprite = "texture=assets/art/enemies/mountain_ape_boss/static_s.png;size=48x48;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 80,
    speed = 38,
    damage = 8,
    aggro_radius = 300,
    attack_range = 48,
    attack_cooldown = 120,
    skill = "palm_gust"
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
  -- 状态种类的定义在战斗全局表里（M34-03）：这里只做引用校验，不复制数值。
  local combat_data = require("data.combat")
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
      -- 受击抵抗（M25）：可选字段，但一旦写了就必须是 [0,1] 的数字，
      -- 否则"木人不会被推走"这类手感约定会静默失效。
      for _, field in ipairs({ "knockback_resist", "hitstun_resist" }) do
        local value = enemy[field]
        if value ~= nil and (type(value) ~= "number" or value < 0 or value > 1) then
          issues[#issues + 1] = path .. "." .. field .. ": expected number in [0, 1]"
        end
      end
      -- 命中施加的瞬时状态（M34-03）：可选，但引用的是 data/combat.lua 的 status 键，
      -- 打错字的效果和打错字的对话分支一样安静（敌人照常攻击、状态永远不出现），所以必查。
      if enemy.status ~= nil then
        if type(enemy.status) ~= "string" or not combat_data.status_entry(enemy.status) then
          issues[#issues + 1] = path .. ".status: unknown status kind " .. tostring(enemy.status)
            .. " (known: " .. table.concat(combat_data.status_kinds(), "/") .. ")"
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
