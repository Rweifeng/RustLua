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
-- Boss 二阶段的专属字段（M34-04b ⑤；只写在**主人**那一行）：
--   phase2_below     气血阈值：`hp <= phase2_below` 即进入二阶段。**阶段是从 hp 推出来的**，
--                   没有独立计数器，所以存/读档、切图、受击硬直都不会让 Boss 退回一阶段
--                   （写了 >= hp 的数字二阶段永远进不去 → validate_all 判错）。
--   charge_skill     二阶段的冲撞招式 id（引用 data/skills.lua，且必须是出判定窗的 kind：
--                   kind=movement/guard 撞不到人，那是"没有消费方的假冲撞"）。
--   charge_speed     冲撞期间的位移速度（世界单位/秒；与 speed 分开，因为"走"和"撞"是两手）。
--   charge_steps     冲撞最多走多少固定步（撞到 solid 格会提前结束）。
--   summon_group     召唤组名（与下面 summoned 行的 summon_group 配对；**组里有几行 = 场上同时
--                   存在的召唤物上界**，所以只有这一处事实，没有第二份 max_alive 会跟它漂移）。
--   summon_interval  二阶段每多少固定步尝试补满一次空槽位。
--
-- 召唤物行（`summoned = true`）的专属字段：
--   summoned         声明"这一行不随进图生成"：enemy.lua 的 sync/tick/capture/reset 都只把它
--                   当**槽位**，只有主人二阶段召出时才落地。它因此不入档、切图不残留。
--   summon_owner     主人的 persist_id（必须是同一组里带 summon_group 的那一行）。
--   summon_group     组名，与主人的 summon_group 相同。
--   summon_offset    召出时相对**主人所在格**的偏移 {x, y}（全 0 或超界判错）；
--                   偏移格是 solid 时退回主人脚下那一格。
--   summon_ttl       存活上限（固定步）：到期自动离场。写这个数字是因为"召唤物不入档"——
--                   没有寿命的召唤物会一直占着场上名额，读档后又变成无主实体。
--
-- 会站防的敌人（M34-04b ⑤‴）：
--   guard_skill      格挡招式 id（引用 data/skills.lua，且必须是 kind=guard 的那一类）。
--                   抬起的时机**不建新计时器**：enemy.lua 只在"玩家贴进 attack_range 且
--                   自己这一刀还在冷却（next_attack 未到）"的空档里站着防，刀一就绪就换手打。
--                   减免比例与正面角度都读那条 guard 招式记录（与玩家守心诀同一份 absorb），
--                   所以敌人侧没有第二个"挡多少"的数字；敌人站防不吃玩家内力（§6.2 口径③）。
--                   写在不出刀的行上（damage/attack_range/attack_cooldown 有一个为 0）永远举不起
--                   盾，那是死键 → validate_all 判错。
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
  -- 玄冥会徒：会站防的敌人（M34-04b ⑤‴）。以前只有"同一招式的更远判定"这个近似——
  -- 现在它在自己两刀之间的空档里真的举起格挡，正面命中按 §6.1 守心诀那条记录的 absorb 减。
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
    skill = "strike",
    guard_skill = "guard_heart"
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
  -- M34-04b ⑤ 起它有了 docs/30 §6 承诺的第二阶段：气血掉到一半以下后开始**冲撞**与
  -- **召唤幼崽**。两组数字都写在这里（不在 enemy.lua 的 if 里），因为它们是玩法旋钮：
  -- 冲撞靠 ape_charge 的判定窗扫过路径撞人，召唤的上界则由下面那两行槽位决定。
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
    skill = "palm_gust",
    phase2_below = 40,
    charge_skill = "ape_charge",
    charge_speed = 150,
    charge_steps = 40,
    summon_group = "ape_whelps",
    summon_interval = 300
  },
  -- 山魈幼崽（召唤槽位 1/2）：只在 Boss 二阶段被召出，本身不入档、切图不残留。
  -- 伤害/血量按"杂兵"定（玩家两刀一个），形象暂借山魈首领的贴图缩到 32×32
  -- （与野猪借灰狼贴图同一手法：纯数据新增，不生成新美术）。
  -- 掉落**故意不配**（data/drops.lua 没有这两行）：召唤物是 Boss 战的过程产物，
  -- 若给铜钱就成了"刷 Boss 召唤物"的无限循环；docs/33 §6.2 那两行的掉落列因此是「无」。
  {
    name = "山魈幼崽",
    persist_id = "enemy_ape_whelp_1",
    map = "cold_spring_cave",
    -- 槽位行不靠 home 落位（召出时按 summon_offset 相对主人算），这一格只是初始值。
    home = { x = 10, y = 6 },
    art = "mountain_ape_boss",
    sprite = "texture=assets/art/enemies/mountain_ape_boss/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 10,
    speed = 60,
    damage = 3,
    aggro_radius = 240,
    attack_range = 32,
    attack_cooldown = 60,
    skill = "strike",
    summoned = true,
    summon_owner = "enemy_ape_boss",
    summon_group = "ape_whelps",
    summon_offset = { x = -2, y = 0 },
    summon_ttl = 600
  },
  {
    name = "山魈幼崽",
    persist_id = "enemy_ape_whelp_2",
    map = "cold_spring_cave",
    home = { x = 10, y = 6 },
    art = "mountain_ape_boss",
    sprite = "texture=assets/art/enemies/mountain_ape_boss/static_s.png;size=32x32;tint=1.000,1.000,1.000,1.000;sorting_layer=actor",
    hp = 10,
    speed = 60,
    damage = 3,
    aggro_radius = 240,
    attack_range = 32,
    attack_cooldown = 60,
    skill = "strike",
    summoned = true,
    summon_owner = "enemy_ape_boss",
    summon_group = "ape_whelps",
    summon_offset = { x = 2, y = 0 },
    summon_ttl = 600
  },
}

function M.get(persist_id)
  for _, enemy in ipairs(M.enemies) do
    if enemy.persist_id == persist_id then
      return enemy
    end
  end
  return nil
end

-- 敌人记录里合法的键（与 data/skills.lua 的 skill_keys 同一条不变量）：写了别的键 =
-- 这个键没有任何消费方。召唤/Boss 阶段那批新键尤其要查——`summon_ttl` 打成 `summon_tttl`
-- 的表现是"召唤物永远不死"，正是最难查的那种安静 bug。
M.enemy_keys = {
  name = true, persist_id = true, map = true, home = true, art = true, sprite = true,
  hp = true, speed = true, damage = true, aggro_radius = true, attack_range = true,
  attack_cooldown = true, skill = true, status = true,
  knockback_resist = true, hitstun_resist = true,
  -- 站防（普通行也可以用，不是 Boss 专属）
  guard_skill = true,
  -- Boss 二阶段（主人行专属）
  phase2_below = true, charge_skill = true, charge_speed = true, charge_steps = true,
  summon_group = true, summon_interval = true,
  -- 召唤物行专属
  summoned = true, summon_owner = true, summon_offset = true, summon_ttl = true
}

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
  -- 召唤组（M34-04b ⑤）要先建两张索引：主人行与槽位行是**互相引用**的
  -- （槽位行写 summon_owner，主人行写 summon_group），而表顺序无关，
  -- 所以不能靠"遍历时已经见过主人"这种巧合来判引用是否存在。
  local by_id = {}
  local group_slots = {}
  for _, enemy in ipairs(M.enemies) do
    if type(enemy) == "table" then
      if type(enemy.persist_id) == "string" then
        by_id[enemy.persist_id] = enemy
      end
      if enemy.summoned == true and type(enemy.summon_group) == "string" then
        group_slots[enemy.summon_group] = (group_slots[enemy.summon_group] or 0) + 1
      end
    end
  end
  local seen = {}
  for index, enemy in ipairs(M.enemies) do
    local path = "data/enemies.lua:enemies[" .. tostring(index) .. "]"
    if type(enemy) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      for field in pairs(enemy) do
        if not M.enemy_keys[field] then
          issues[#issues + 1] = path .. "." .. tostring(field) .. ": unknown enemy key"
        end
      end
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
      -- 站防的敌人（M34-04b ⑤‴）：引用的招式必须是 kind=guard 的那一类。指到伤害招式上，
      -- enemy.lua 会照样把它"举起来"，而 combat 侧读不到 absorb —— 于是这条数据写没写都一样，
      -- 玩家只觉这一刀忽然不减了。打错字在加载期报，不留给手感。
      if enemy.guard_skill ~= nil then
        local guard = skills.get(enemy.guard_skill)
        if not guard then
          issues[#issues + 1] = path .. ".guard_skill: unknown skill " .. tostring(enemy.guard_skill)
        elseif guard.kind ~= "guard" then
          issues[#issues + 1] = path .. ".guard_skill: " .. tostring(enemy.guard_skill)
            .. " kind=" .. tostring(guard.kind) .. " 不是格挡招式（没有 absorb 可减）"
        elseif (tonumber(enemy.damage) or 0) <= 0
          or (tonumber(enemy.attack_range) or 0) <= 0
          or (tonumber(enemy.attack_cooldown) or 0) <= 0 then
          -- 空档来自"它出一刀之后的冷却"，所以不攻击的行写了 guard_skill 也永远举不起盾。
          issues[#issues + 1] = path .. ".guard_skill: 这一行不出刀（damage/attack_range/attack_cooldown"
            .. " 有一个是 0），永远站不起防"
        end
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
      -- ---- Boss 二阶段与召唤槽位（M34-04b ⑤）----
      -- 这组校验存在的理由不是"字段格式要好看"，而是这批数字**错了不会响**：
      -- phase2_below 写得比 hp 大 → 二阶段永远进不去；summon_ttl 打错一个字母 →
      -- 召唤物永远不死；summon_group 拼错 → Boss 每 summon_interval 步什么也没召出来。
      -- 四种表现都是"Boss 看起来只是变弱了"，所以必须在加载期就报出来。
      if enemy.summoned ~= nil then
        if enemy.summoned ~= true then
          issues[#issues + 1] = path .. ".summoned: expected true (only used to mark a summon slot)"
        end
        local owner = by_id[enemy.summon_owner]
        if not owner then
          issues[#issues + 1] = path .. ".summon_owner: unknown enemy " .. tostring(enemy.summon_owner)
        else
          if owner.summon_group ~= enemy.summon_group then
            issues[#issues + 1] = path .. ".summon_group: does not match owner "
              .. tostring(enemy.summon_owner) .. " group " .. tostring(owner.summon_group)
          end
          if owner.map ~= enemy.map then
            issues[#issues + 1] = path .. ".map: summon slot lives on " .. tostring(enemy.map)
              .. " but its owner is on " .. tostring(owner.map) .. "（永远不会被召出）"
          end
        end
        if type(enemy.summon_group) ~= "string" or enemy.summon_group == "" then
          issues[#issues + 1] = path .. ".summon_group: required non-empty string on a summon slot"
        end
        local offset = enemy.summon_offset
        if type(offset) ~= "table"
          or type(offset.x) ~= "number" or math.floor(offset.x) ~= offset.x
          or type(offset.y) ~= "number" or math.floor(offset.y) ~= offset.y then
          issues[#issues + 1] = path .. ".summon_offset: expected {x, y} integers (cells)"
        elseif offset.x == 0 and offset.y == 0 then
          issues[#issues + 1] = path .. ".summon_offset: {0, 0} puts every slot on the owner's cell"
        elseif math.abs(offset.x) > 4 or math.abs(offset.y) > 4 then
          issues[#issues + 1] = path .. ".summon_offset: out of the boss's reach (|dx|,|dy| <= 4)"
        end
        local ttl = tonumber(enemy.summon_ttl)
        if not ttl or ttl < 1 or math.floor(ttl) ~= ttl then
          issues[#issues + 1] = path .. ".summon_ttl: expected positive integer (fixed steps)"
        end
      elseif type(enemy.summon_group) == "string" then
        -- 主人行：组里必须真有槽位，否则 summon_interval 是一个"每 5 秒什么也不发生"的旋钮。
        local slots = group_slots[enemy.summon_group] or 0
        if slots <= 0 then
          issues[#issues + 1] = path .. ".summon_group: no summon slot references "
            .. enemy.summon_group .. "（上界是 0，召不出东西）"
        end
        local interval = tonumber(enemy.summon_interval)
        if not interval or interval < 1 or math.floor(interval) ~= interval then
          issues[#issues + 1] = path .. ".summon_interval: expected positive integer (fixed steps)"
        end
        local below = tonumber(enemy.phase2_below)
        local max_hp = tonumber(enemy.hp)
        if not below or below <= 0 then
          issues[#issues + 1] = path .. ".phase2_below: expected positive hp threshold"
        elseif max_hp and below >= max_hp then
          issues[#issues + 1] = path .. ".phase2_below: " .. tostring(below)
            .. " >= hp " .. tostring(max_hp) .. "（二阶段永远进不去）"
        end
        local charge = skills.get(enemy.charge_skill)
        if not charge then
          issues[#issues + 1] = path .. ".charge_skill: unknown skill " .. tostring(enemy.charge_skill)
        elseif not skills.uses_hitbox(charge) then
          issues[#issues + 1] = path .. ".charge_skill: " .. tostring(enemy.charge_skill)
            .. " kind=" .. tostring(charge.kind) .. " 不出判定窗，撞不到人"
        end
        local charge_speed = tonumber(enemy.charge_speed)
        if not charge_speed or charge_speed <= 0 then
          issues[#issues + 1] = path .. ".charge_speed: expected positive number (world units/sec)"
        end
        local steps = tonumber(enemy.charge_steps)
        if not steps or steps < 1 or math.floor(steps) ~= steps then
          issues[#issues + 1] = path .. ".charge_steps: expected positive integer (fixed steps)"
        end
      else
        -- 既不是槽位也不是主人：二阶段那批键一个都不该出现（否则就是打错字的死键，
        -- 上面的 unknown enemy key 抓不到它——它确实在白名单里，但没有消费方）。
        for _, field in ipairs({ "phase2_below", "charge_skill", "charge_speed", "charge_steps",
          "summon_interval", "summon_owner", "summon_offset", "summon_ttl" }) do
          if enemy[field] ~= nil then
            issues[#issues + 1] = path .. "." .. field
              .. ": 这一行既不是 Boss 阶段行也不是召唤槽位，这个数字没有消费方"
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

return M
