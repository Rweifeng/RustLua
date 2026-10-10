-- data/maps.lua：《江湖有田》地图注册表（M17-01 多地图）。
--
-- 每张地图对应 scenes/<id>.scene。字段：
--   display      HUD/日志显示名
--   title        HUD 顶部标题文案（scene 里 HudTitle 显示文本同步）
--   spawn        首次进入/无入口时的默认出生 cell
--   exits        本图出口表：
--     cell       触发格（cell 0-index、y 朝上）：**站在这一格**按 E 触发，
--                或**站在它正前方一格并面朝它**按 E（门洞通常一格宽，玩家常停在门外那格）
--     to         目标地图 id
--     to_cell    目标地图落点 cell（换算为世界坐标经 map.goto 落位）
--     label      prompt 文案
--
-- 世界坐标 = (cx+0.5)*TILE；TILE 与 scene meta tile_size=32 一致。
-- cell 语义与 tilemap/farm 相同：0-index、y 朝上（row 0 在地图最底）。
local M = {}

local TILE = 32

local function cell_to_world(cx, cy)
  return (cx + 0.5) * TILE, (cy + 0.5) * TILE
end

M.TILE = TILE

-- 山庄 → 云泽村：山庄南侧院墙内石路 (16,1)，云泽村南市街 (19,2) 落点。
-- 云泽村 → 山庄：南市街口 (19,1) 站立触发，回山庄药田前 (16,3)。
M.version = 1
M.maps = {
  manor = {
    display = "江湖有田·山庄",
    title = "山庄 · 药田垂直切片",
    spawn = { x = 16, y = 3 }, -- 与 manor.scene JianghuPlayer Transform 一致
    exits = {
      {
        id = "main_to_village",
        cell = { x = 16, y = 1 }, -- 院墙内南侧（玩家在此面对墙按 E）
        to = "village",
        to_cell = { x = 19, y = 2 },
        label = "离开山庄 · 前往云泽村"
      }
    }
  },
  village = {
    display = "江湖有田·云泽村",
    title = "云泽村 · 南市街",
    spawn = { x = 19, y = 2 }, -- 与 village.scene JianghuPlayer Transform 一致
    exits = {
      {
        id = "village_to_main",
        cell = { x = 19, y = 1 }, -- 南市街口（南门内）
        to = "manor",
        to_cell = { x = 16, y = 3 },
        label = "返回山庄"
      },
      -- 批次 2/3：北上青云山、进药庐、进铁匠铺（对应 scenes/village.scene 的三个门洞/门前格）
      {
        id = "village_to_mountain",
        cell = { x = 10, y = 20 }, -- 村北山门门洞（M72-11 复核：贴图那一格不透明占比 0.156，真是洞；山门 sprite 已从 (9,19) 挪到 (9,20) 压住北墙，洞才落在出口格上）
        to = "mountain_path",
        to_cell = { x = 19, y = 2 },
        label = "出村 · 上青云山"
      },
      {
        id = "village_to_herb",
        cell = { x = 5, y = 13 }, -- 药庐门前
        to = "interior_herb",
        to_cell = { x = 10, y = 3 },
        label = "进入药庐"
      },
      {
        id = "village_to_forge",
        -- 门前格从 (26,10) 挪到 (27,10)（M72-11）：横屏方案A 把杂货铺搬进市集 x21..26&y6..10
        -- 之后，(26,10) 是铺子屋顶最后一行（solid 真算之后确实实心）。铁匠铺 x25..28&y11..12
        -- 的门前只剩东侧 x27..28 那一排，走 (27,10)。
        cell = { x = 27, y = 10 },
        to = "interior_forge",
        to_cell = { x = 10, y = 3 },
        label = "进入铁匠铺"
      }
    }
  },
  -- 青云山道：采集、敌人、护送任务（docs/30 §10.1），由 tools/gen_wilds_scene.py 生成
  mountain_path = {
    display = "江湖有田·青云山道",
    title = "青云山道 · 采药与护送",
    spawn = { x = 19, y = 2 }, -- 与 mountain_path.scene JianghuPlayer Transform 一致
    exits = {
      {
        id = "mountain_to_village",
        cell = { x = 19, y = 1 }, -- 南山门内
        to = "village",
        to_cell = { x = 10, y = 19 },
        label = "下山 · 返回云泽村"
      },
      {
        id = "mountain_to_cave",
        cell = { x = 22, y = 20 }, -- 东北洞口
        to = "cold_spring_cave",
        to_cell = { x = 10, y = 3 },
        label = "进入寒泉洞"
      }
    }
  },
  -- 寒泉洞：矿石、稀有药材、第一章线索（小型副本 20×12）
  cold_spring_cave = {
    display = "江湖有田·寒泉洞",
    title = "寒泉洞 · 第一章线索",
    spawn = { x = 10, y = 3 },
    exits = {
      {
        id = "cave_to_mountain",
        cell = { x = 10, y = 1 }, -- 洞口
        to = "mountain_path",
        to_cell = { x = 22, y = 19 },
        label = "离开寒泉洞"
      }
    }
  },
  -- 室内（批次 3，20×12）：药庐 / 铁匠铺
  interior_herb = {
    display = "云泽村·药庐内",
    title = "药庐 · 药材与药炉",
    spawn = { x = 10, y = 3 },
    exits = {
      {
        id = "herb_to_village",
        cell = { x = 10, y = 1 }, -- 门口
        to = "village",
        to_cell = { x = 5, y = 14 },
        label = "走出药庐"
      }
    }
  },
  interior_forge = {
    display = "云泽村·铁匠铺内",
    title = "铁匠铺 · 铁砧与炉火",
    spawn = { x = 10, y = 3 },
    exits = {
      {
        id = "forge_to_village",
        cell = { x = 10, y = 1 },
        to = "village",
        -- 落点与上面的门前格同一格 (27,10)（M72-11）：旧值 (26,11) 在铁匠铺自己的占格里
        -- （x25..28&y11..12 整框不透明，8 格全实心），走出铺子等于把人塞进墙里。
        -- 铁匠铺只有两行高，没有"门前走廊"可用，所以进出共用门前那一格。
        to_cell = { x = 27, y = 10 },
        label = "走出铁匠铺"
      }
    }
  },
  -- 演武场：山庄练武场（40x23，scenes/combat_arena.scene 由 tools/gen_arena_scene.py 生成；
  -- 战斗验收锚点几何见该文件头注释，改动需与 jhyt_m18_combat.rs 三方同步）。
  combat_arena = {
    display = "江湖有田·演武场",
    title = "山庄 · 演武场",
    spawn = { x = 9, y = 5 }, -- 与 combat_arena.scene JianghuPlayer Transform 一致
    exits = {
      {
        id = "arena_to_main",
        cell = { x = 9, y = 1 }, -- 南门门洞内（manor_gate x8..10, y0..2，门洞 (9,1)(9,2)）
        to = "manor",
        to_cell = { x = 16, y = 3 },
        label = "离开演武场 · 返回山庄"
      }
    }
  }
}

-- 出口 id → (from_map_id, exit) 反查
M.exits = {}
for map_id, map in pairs(M.maps) do
  for _, exit in ipairs(map.exits or {}) do
    exit.from = map_id
    M.exits[exit.id] = exit
  end
end

function M.get(map_id)
  return M.maps[map_id]
end

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/maps.lua: version: unsupported version " .. tostring(M.version)
  end
  local seen_exits = {}
  for map_id, map in pairs(M.maps) do
    local path = "data/maps.lua:maps." .. tostring(map_id)
    if type(map) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if type(map.display) ~= "string" or map.display == "" then
        issues[#issues + 1] = path .. ".display: required non-empty string"
      end
      local spawn = map.spawn
      if type(spawn) ~= "table" or type(spawn.x) ~= "number" or type(spawn.y) ~= "number" then
        issues[#issues + 1] = path .. ".spawn: expected {x, y} numbers"
      elseif spawn.x < 0 or spawn.x >= 40 or spawn.y < 0 or spawn.y >= 23 then
        issues[#issues + 1] = path .. ".spawn: out of map bounds"
      end
      if type(map.exits) ~= "table" then
        issues[#issues + 1] = path .. ".exits: expected array"
      else
        for index, exit in ipairs(map.exits) do
          local epath = path .. ".exits[" .. tostring(index) .. "]"
          if type(exit) ~= "table" then
            issues[#issues + 1] = epath .. ": expected object"
          else
            if type(exit.id) ~= "string" or exit.id == "" then
              issues[#issues + 1] = epath .. ".id: required non-empty string"
            elseif seen_exits[exit.id] then
              issues[#issues + 1] = epath .. ".id: duplicate object id " .. exit.id
            else
              seen_exits[exit.id] = true
            end
            if not M.maps[exit.to] then
              issues[#issues + 1] = epath .. ".to: unknown map " .. tostring(exit.to)
            end
            for _, field in ipairs({ "cell", "to_cell" }) do
              local cell = exit[field]
              if type(cell) ~= "table" or type(cell.x) ~= "number" or type(cell.y) ~= "number" then
                issues[#issues + 1] = epath .. "." .. field .. ": expected {x, y} numbers"
              elseif cell.x < 0 or cell.x >= 40 or cell.y < 0 or cell.y >= 23 then
                issues[#issues + 1] = epath .. "." .. field .. ": out of map bounds"
              end
            end
            if type(exit.label) ~= "string" or exit.label == "" then
              issues[#issues + 1] = epath .. ".label: required non-empty string"
            end
          end
        end
      end
    end
  end
  return #issues == 0, issues
end

function M.exit_world(exit)
  return cell_to_world(exit.cell.x, exit.cell.y)
end

function M.to_world(exit)
  return cell_to_world(exit.to_cell.x, exit.to_cell.y)
end

-- 目标地图出口 id（map_id → exit id）：入口落点校验用
function M.entrance_to(to_map)
  for map_id, map in pairs(M.maps) do
    for _, exit in ipairs(map.exits or {}) do
      if exit.to == to_map then
        return map_id, exit
      end
    end
  end
  return nil, nil
end

return M
