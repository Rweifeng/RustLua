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
--   * manor  ：x1..38 / y1..20。实心块 = 正房 x2..8&y5..11、柴房 x4..8&y15..18、
--              池塘 x34..38&y4..9、南门立柱 (15,1)(15,2)(17,1)(17,2)，
--              以及 19 格道具占位（水井/水缸/草堆/晒谷架×2/木牌/竹篱×2/树×4/竹×3）；
--              x9..13 为贯通南北的走廊，y=3 的田前石路横通东西。
--              manor 的 solid 由 tools/gen_manor_scene.py 生成，
--              改布局后必须重跑它（自带 places.lua 可达性契约自检）。
--   * village：整圈白墙黛瓦院墙；主街/市集广场/中街贯通 y1..14（x1..38 基本全通）。
--              实心块 = 药庐 x3..8&y15..19、杂货铺 x31..36&y15..19、铁匠铺 x25..28&y11..12、
--              茶摊 x31..34&y10..11、民居A x16..19&y18..20、民居B x27..30&y18..20、
--              土地庙 x22..23&y19..20，以及 17 处道具占位。
--              溪水在 y16..17（x11..29，x18..21 加宽到 y15..17），**只有石桥 x24..25 能过河**；
--              北岸 y18..20 另有西侧 x9..10 走廊与东侧 x30/x37 回到村内。
--              village 的 solid 由 tools/gen_village_scene.py 生成，
--              改布局后必须重跑它（自带 places.lua 可达性契约自检）。
local M = {}

M.version = 1
M.places = {
  -- 山庄 manor
  manor_gate = { map = "manor", cell = { x = 16, y = 2 }, label = "山庄·门廊" },
  manor_home = { map = "manor", cell = { x = 12, y = 6 }, label = "山庄·厢房" },
  manor_home_east = { map = "manor", cell = { x = 20, y = 8 }, label = "山庄·东厢" },
  manor_herb_field = { map = "manor", cell = { x = 16, y = 5 }, label = "山庄·药田" },
  manor_study = { map = "manor", cell = { x = 24, y = 10 }, label = "山庄·书房" },
  manor_sword_ground = { map = "manor", cell = { x = 26, y = 12 }, label = "山庄·剑坪" },
  manor_forge = { map = "manor", cell = { x = 28, y = 12 }, label = "山庄·工坊" },
  manor_practice = { map = "manor", cell = { x = 30, y = 14 }, label = "山庄·练功场" },
  manor_garden = { map = "manor", cell = { x = 10, y = 16 }, label = "山庄·后园" },
  manor_dock = { map = "manor", cell = { x = 16, y = 19 }, label = "山庄·渡口" },

  -- 青云山道 mountain_path（批次 2；由 tools/gen_wilds_scene.py 生成并自检可达性）
  mountain_gate = { map = "mountain_path", cell = { x = 19, y = 2 }, label = "青云山·山门" },
  mountain_teashed = { map = "mountain_path", cell = { x = 15, y = 8 }, label = "青云山·茶棚" },
  mountain_camp = { map = "mountain_path", cell = { x = 31, y = 6 }, label = "青云山·贼寨" },
  mountain_bridge = { map = "mountain_path", cell = { x = 25, y = 11 }, label = "青云山·溪桥" },
  mountain_herbs = { map = "mountain_path", cell = { x = 12, y = 12 }, label = "青云山·药草丛" },
  mountain_ore = { map = "mountain_path", cell = { x = 28, y = 18 }, label = "青云山·矿石堆" },
  mountain_cave_mouth = { map = "mountain_path", cell = { x = 21, y = 20 }, label = "青云山·洞口" },

  -- 寒泉洞 cold_spring_cave（批次 2）
  cave_entry = { map = "cold_spring_cave", cell = { x = 10, y = 3 }, label = "寒泉洞·洞口" },
  cave_pool = { map = "cold_spring_cave", cell = { x = 7, y = 4 }, label = "寒泉洞·暗河" },
  cave_ore = { map = "cold_spring_cave", cell = { x = 9, y = 8 }, label = "寒泉洞·矿脉" },
  cave_snow_lotus = { map = "cold_spring_cave", cell = { x = 15, y = 7 }, label = "寒泉洞·雪莲" },

  -- 室内（批次 3）
  herb_hut_inside = { map = "interior_herb", cell = { x = 10, y = 4 }, label = "药庐内·药炉" },
  forge_inside = { map = "interior_forge", cell = { x = 10, y = 4 }, label = "铁匠铺内·铁砧" },

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

function M.validate_all()
  local issues = {}
  if M.version ~= 1 then
    issues[#issues + 1] = "data/places.lua: version: unsupported version " .. tostring(M.version)
  end
  local maps = require("data.maps")
  for name, place in pairs(M.places) do
    local path = "data/places.lua:places." .. tostring(name)
    if type(place) ~= "table" then
      issues[#issues + 1] = path .. ": expected object"
    else
      if not maps.get(place.map) then
        issues[#issues + 1] = path .. ".map: unknown map " .. tostring(place.map)
      end
      local cell = place.cell
      if type(cell) ~= "table" or type(cell.x) ~= "number" or type(cell.y) ~= "number" then
        issues[#issues + 1] = path .. ".cell: expected {x, y} numbers"
      elseif cell.x < 0 or cell.x >= 40 or cell.y < 0 or cell.y >= 22 then
        issues[#issues + 1] = path .. ".cell: out of map bounds"
      end
      if type(place.label) ~= "string" or place.label == "" then
        issues[#issues + 1] = path .. ".label: required non-empty string"
      end
    end
  end
  return #issues == 0, issues
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
