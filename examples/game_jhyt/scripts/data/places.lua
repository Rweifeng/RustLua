-- data/places.lua：《江湖有田》地点注册表（M17-02 NPC 日程）。
--
-- 地点是纯数据：一个名字 → (地图 id, cell, 显示名)。NPC 日程只引用地点名，
-- 由 npc.lua 解析成本图可寻路目标格（A* 由 Rust tilemap.find_path 提供）。
--
-- cell 语义与 tilemap/maps 一致：0-index、y 朝上（row 0 在地图最底）；
-- 世界坐标 = (cx + 0.5) * TILE。**每个 cell 必须落在对应地图的可行走格**，
-- 否则 NPC 会输出"路径端点不可达"诊断（这是刻意的失败可见性）。
--
-- 可行走格事实（来自 scenes/<map>.scene 的 GroundMap solid 位图，40x22）：
--   * main   ：x1..38 / y1..20，挖去西侧实心块 x2..8&y3..11 与
--              东侧实心块 x34..39&y4..9；x1 为贯通南北的走廊。
--   * village：x1..38 / y1..10 全通；y11..15 仅 x1/x10..29/x38；
--              y16..17 在 x18..21 断开（池塘）；y18..20 仅 x1/x10..29/x38。
local M = {}

M.places = {
  -- 山庄 main
  manor_gate = { map = "main", cell = { x = 16, y = 2 }, label = "山庄·门廊" },
  manor_home = { map = "main", cell = { x = 12, y = 6 }, label = "山庄·厢房" },
  manor_home_east = { map = "main", cell = { x = 20, y = 8 }, label = "山庄·东厢" },
  manor_herb_field = { map = "main", cell = { x = 16, y = 5 }, label = "山庄·药田" },
  manor_study = { map = "main", cell = { x = 24, y = 10 }, label = "山庄·书房" },
  manor_sword_ground = { map = "main", cell = { x = 26, y = 12 }, label = "山庄·剑坪" },
  manor_forge = { map = "main", cell = { x = 28, y = 12 }, label = "山庄·工坊" },
  manor_practice = { map = "main", cell = { x = 30, y = 14 }, label = "山庄·练功场" },
  manor_garden = { map = "main", cell = { x = 10, y = 16 }, label = "山庄·后园" },
  manor_dock = { map = "main", cell = { x = 16, y = 19 }, label = "山庄·渡口" },

  -- 云泽村 village
  village_gate = { map = "village", cell = { x = 19, y = 2 }, label = "云泽村·南市街口" },
  village_market = { map = "village", cell = { x = 14, y = 3 }, label = "云泽村·集市" },
  village_stall = { map = "village", cell = { x = 20, y = 3 }, label = "云泽村·货摊" },
  village_plaza = { map = "village", cell = { x = 20, y = 5 }, label = "云泽村·广场" },
  village_clinic = { map = "village", cell = { x = 24, y = 6 }, label = "云泽村·医馆" },
  village_home_west = { map = "village", cell = { x = 12, y = 8 }, label = "云泽村·沈宅" },
  village_home_south = { map = "village", cell = { x = 16, y = 8 }, label = "云泽村·赵宅" },
  village_smithy = { map = "village", cell = { x = 28, y = 8 }, label = "云泽村·铁匠铺" },
  village_teahouse = { map = "village", cell = { x = 32, y = 8 }, label = "云泽村·茶摊" },
  village_home_north = { map = "village", cell = { x = 24, y = 10 }, label = "云泽村·林宅" },
  village_home_forge = { map = "village", cell = { x = 29, y = 12 }, label = "云泽村·韩宅" },
  village_alley = { map = "village", cell = { x = 28, y = 13 }, label = "云泽村·后巷" },
  village_teafield = { map = "village", cell = { x = 12, y = 13 }, label = "云泽村·茶园" },
  village_pond = { map = "village", cell = { x = 20, y = 14 }, label = "云泽村·池畔" },
  village_shrine = { map = "village", cell = { x = 24, y = 19 }, label = "云泽村·土地庙" },
  village_dock = { map = "village", cell = { x = 14, y = 19 }, label = "云泽村·渡口" }
}

function M.get(name)
  return M.places[name]
end

-- 稳定排序的名字列表（诊断/遍历顺序确定）
function M.names()
  local names = {}
  for name in pairs(M.places) do
    names[#names + 1] = name
  end
  table.sort(names)
  return names
end

return M
