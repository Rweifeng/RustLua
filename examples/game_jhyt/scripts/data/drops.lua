-- data/drops.lua：《江湖有田》敌人掉落表（M28 / 计划 G3）。
--
-- 为什么单独一张表而不是塞进 data/enemies.lua：掉落是**产出**口径（每只敌人给什么、
-- 给多少、给的概率），与敌人本体（生命/速度/招式）是两类会各自演进的数值；
-- 分开后 `systems/loot.lua` 只依赖本表，战斗侧不需要知道经济。
--
-- 字段（闭集，多写一个键就报错，避免"写了但不生效"）：
--   enemy   敌人 persist_id（必须存在于 data/enemies.lua；同一敌人只能有一条记录）
--   money   可选，{ min, max } 掉落铜钱（闭区间整数；0/0 等价于不掉钱）
--   items   可选，数组 { { id = 物品id, min, max, chance? } }
--             id     必须存在于 data/items.lua 或 data/tools.lua
--             min/max 闭区间整数（可 0 = 有概率不掉）
--             chance 掉落概率 0 < chance <= 1（缺省 1 = 必掉）
--
-- 口径（docs/33 §6.2 敌人表 + §6.3 死亡与恢复）：
--   * 铜钱**当场入账**，物品先进"当日待入库"袋（`systems/loot.lua`），
--     睡觉跨天时入库；死亡会损失待入库物品的 50%（未入库 = 还没落袋为安）。
--   * 没有记录的敌人 = 不掉落（训练木人就是这种：它是练功靶，不该产出资源）。
local M = {}

M.version = 1

M.ENTRY_KEYS = { enemy = true, money = true, items = true }

M.drops = {
  -- 演武场灰狼：MVP 里"打猎"的第一个来源（狼皮是山道猎获订单的交付物之一）。
  {
    enemy = "enemy_wolf",
    money = { min = 0, max = 3 },
    items = {
      { id = "wolf_pelt", min = 1, max = 1 }
    }
  },
  -- 青云山道野猪：两下能打死（docs/46 §5），补足猎获订单需要的兽肉。
  {
    enemy = "enemy_wild_boar",
    money = { min = 2, max = 6 },
    items = {
      { id = "boar_meat", min = 1, max = 2 }
    }
  },
  -- 山贼喽啰：docs/33 §6.2 的口径（铜钱 8–15、木材 0–2）——木材在实现里是
  -- `timber`（木料，M18-02 的建筑升级材料），因此这里按实现写 timber。
  {
    enemy = "enemy_bandit",
    money = { min = 8, max = 15 },
    items = {
      { id = "timber", min = 0, max = 2 }
    }
  },
  -- 毒蝎：docs/33 §6.2 写的是"草药 0–2"，但物品注册表里没有"草药"这一项，
  -- 只有药材 `lingzhi`；这里按实现口径写 lingzhi（文档同步修正）。
  {
    enemy = "enemy_scorpion",
    items = {
      { id = "lingzhi", min = 0, max = 2 }
    }
  },
  -- 玄冥会徒：docs/33 §6.2 的"铜钱 15–25"。
  {
    enemy = "enemy_xuanming",
    money = { min = 15, max = 25 }
  },
  -- 山魈首领：第一章 Boss，掉主线物品（山魈内丹）+ 一笔铜钱。
  {
    enemy = "enemy_ape_boss",
    money = { min = 30, max = 60 },
    items = {
      { id = "ape_core", min = 1, max = 1 }
    }
  }
}

--- 某敌人的掉落记录（无记录 = 不掉落）。
function M.get(enemy_id)
  for _, entry in ipairs(M.drops) do
    if entry.enemy == enemy_id then
      return entry
    end
  end
  return nil
end

--- 稳定排序的列表（按 enemy 升序），供存档/诊断与 headless 遍历。
function M.list()
  local list = {}
  for _, entry in ipairs(M.drops) do
    list[#list + 1] = entry
  end
  table.sort(list, function(a, b)
    return tostring(a.enemy) < tostring(b.enemy)
  end)
  return list
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/drops.lua: version: unsupported version " .. tostring(M.version)
  end
  local enemies = require("data.enemies")
  local items = require("data.items")
  local tools = require("data.tools")
  local function known_item(id)
    return items[id] ~= nil or tools[id] ~= nil
  end
  local function fail(message)
    issues[#issues + 1] = message
  end
  local function check_range(path, range, allow_zero, allowed_extra)
    if type(range) ~= "table" then
      fail(path .. ": expected { min, max }")
      return
    end
    for key in pairs(range) do
      if key ~= "min" and key ~= "max" and not (allowed_extra and allowed_extra[key]) then
        fail(path .. "." .. tostring(key) .. ": unknown key (expected min/max)")
      end
    end
    local min, max = tonumber(range.min), tonumber(range.max)
    if not min or min ~= math.floor(min) or min < 0 then
      fail(path .. ".min: expected integer >= 0")
      return
    end
    if not max or max ~= math.floor(max) or max < 0 then
      fail(path .. ".max: expected integer >= 0")
      return
    end
    if min > max then
      fail(path .. ": min " .. tostring(min) .. " > max " .. tostring(max))
    end
    if not allow_zero and max <= 0 then
      fail(path .. ": max must be positive")
    end
  end

  if type(M.drops) ~= "table" then
    fail("data/drops.lua: drops: expected array")
    return #issues == 0, issues
  end

  local seen = {}
  for index, entry in ipairs(M.drops) do
    local path = "data/drops.lua:drops[" .. tostring(index) .. "]"
    if type(entry) ~= "table" then
      fail(path .. ": expected object")
    else
      for key in pairs(entry) do
        if not M.ENTRY_KEYS[key] then
          fail(path .. "." .. tostring(key) .. ": unknown key (expected enemy/money/items)")
        end
      end
      local enemy = entry.enemy
      if type(enemy) ~= "string" or enemy == "" then
        fail(path .. ".enemy: required non-empty string")
      elseif enemies.get(enemy) == nil then
        fail(path .. ".enemy: unknown enemy " .. tostring(enemy))
      elseif seen[enemy] then
        fail(path .. ".enemy: duplicate entry for " .. tostring(enemy))
      else
        seen[enemy] = true
      end
      local produced = false
      if entry.money ~= nil then
        check_range(path .. ".money", entry.money, true)
        local max = tonumber((type(entry.money) == "table") and entry.money.max or nil) or 0
        produced = produced or max > 0
      end
      if entry.items ~= nil then
        if type(entry.items) ~= "table" or #entry.items == 0 then
          fail(path .. ".items: expected non-empty array")
        else
          for item_index, drop in ipairs(entry.items) do
            local item_path = path .. ".items[" .. tostring(item_index) .. "]"
            if type(drop) ~= "table" then
              fail(item_path .. ": expected object")
            else
              for key in pairs(drop) do
                if key ~= "id" and key ~= "min" and key ~= "max" and key ~= "chance" then
                  fail(item_path .. "." .. tostring(key) .. ": unknown key (expected id/min/max/chance)")
                end
              end
              if type(drop.id) ~= "string" or drop.id == "" then
                fail(item_path .. ".id: required non-empty string")
              elseif not known_item(drop.id) then
                fail(item_path .. ".id: unknown item " .. tostring(drop.id))
              end
              check_range(item_path, drop, true, { id = true, chance = true })
              local max = tonumber(drop.max) or 0
              produced = produced or max > 0
              if drop.chance ~= nil then
                local chance = tonumber(drop.chance)
                if not chance or chance <= 0 or chance > 1 then
                  fail(item_path .. ".chance: expected number in (0, 1]")
                end
              end
            end
          end
        end
      end
      if not produced then
        fail(path .. ": produces nothing (money.max 与 items[].max 都是 0)")
      end
    end
  end
  return #issues == 0, issues
end

return M
