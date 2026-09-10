-- data/maps.lua：《江湖有田》地图注册表（M17-01 多地图）。
--
-- 每张地图对应 scenes/<id>.scene。字段：
--   display      HUD/日志显示名
--   title        HUD 顶部标题文案（scene 里 HudTitle 显示文本同步）
--   spawn        首次进入/无入口时的默认出生 cell
--   exits        本图出口表：
--     cell       玩家需**站立**的触发格（cell 0-index、y 朝上；按下 E 触发）
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
M.maps = {
  main = {
    display = "江湖有田·山庄",
    title = "山庄 · 药田垂直切片",
    spawn = { x = 16, y = 3 }, -- 与 main.scene JianghuPlayer Transform 一致
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
        to = "main",
        to_cell = { x = 16, y = 3 },
        label = "返回山庄"
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
