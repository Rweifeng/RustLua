-- data/places.lua：《江湖有田》地点注册表（M17-02 NPC 日程）。
--
-- 地点是纯数据：一个名字 → (地图 id, cell, 显示名)。NPC 日程只引用地点名，
-- 由 npc.lua 解析成本图可寻路目标格（A* 由 Rust tilemap.find_path 提供）。
--
-- cell 语义与 tilemap/maps 一致：0-index、y 朝上（row 0 在地图最底）；
-- 世界坐标 = (cx + 0.5) * TILE。**每个 cell 必须落在对应地图的可行走格**，
-- 否则 NPC 会输出"路径端点不可达"诊断（这是刻意的失败可见性）。
--
-- 可行走格事实（来自 scenes/<map>.scene 的 GroundMap solid 位图，manor 是 40x23）：
--   * manor  ：x1..38 / y1..20。**solid 不是手填的表**（M72-07 起）：每一格都来自
--              `tools/building_kit.py` 的一条规则——建筑取"本体∪同名屋顶件的不透明像素
--              覆盖格 − 门洞"、树/道具取锚点那一格、院墙与池水取瓦自己的 kind——
--              所以这里不再抄一份会过期的块表。要读数跑
--              `python tools/building_kit.py scenes/manor.scene`。
--              已知的硬契约：南门门洞 (16,1)(16,2) 必须是洞（山庄↔云泽村出口），
--              x9..13 为贯通南北的走廊，y=3 的田前石路横通东西，池塘是不规则岸线
--              （x34..38 / y3..10，27 格，见 `gen_manor_scene.py` 的 `POND_ROWS`）。
--              manor 的 solid 由 tools/gen_manor_scene.py 生成，
--              改布局后必须重跑它（自带 places.lua 可达性契约自检）。
--   * village：整圈白墙黛瓦院墙；主街/市集广场/中街贯通 y1..14。
--              **solid 同样不是手填的表**（M72-11 起与 manor 同一条规则，见上）：占地来自
--              `tools/building_kit.py` + `tools/gen_village_scene.py`，要读数跑
--              `python tools/building_kit.py scenes/village.scene`。
--              已知的硬契约：南门门洞 (19,1)(19,2) = 村↔山庄出口，山门门洞 (10,20) =
--              村↔青云山出口（门楼压在北墙 y21..22 上，最底那行才是人站的格）。
--              溪水 y16..17（x11..29，x18..21 加宽成水潭）把村北切成两块，**过河只有石桥
--              x24..25**；渡口那块空地 x13..15&y18..20 从沿溪岸的 y18 走廊进，是条死胡同。
--              北墙后 x6..8&y20 那 3 格进不去（药庐/山门/老树/墙四面围死），与 manor 的
--              4 个同类小块同档——复现：`python tools/gen_village_scene.py --out .temp/v.scene`
--              末行"可行走连通块"报出块数与每块大小。
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
  -- 渡口从 (16,19) 搬到池塘西南岸 (35,3)：M72-07 把施工演示（地基+脚手架）按盘上真值
  -- 登记在 x16..19/y19..20，那一格成了建筑占地；而"渡口"本来就该临水，旧格在西北角的
  -- 草地上，一滴水都没有。改的是格不是键名，`schedules/lu_guizang.lua` 不用动。
  manor_dock = { map = "manor", cell = { x = 35, y = 3 }, label = "山庄·渡口" },

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
  -- 南市街口往里一格 (19,3)（M72-11 的新契约把它判红的，见 `gen_village_scene.py:self_check`
  -- 第 6 条）：旧格 (19,2) 与南门出口格 (19,1) 上下紧邻，而出口的交互优先级 20 高于对话 10，
  -- 且它的 query 连"面向的那一格"也算——阿奇 6:00-8:00 站在 (19,2) 时，玩家在他脚下按 E
  -- 若面朝北墙门洞，抢到的就是"返回山庄"而不是对话。这一格是存量隐患（本轮之前没有门覆盖它），
  -- 挪一格仍在"南市街口"的语义区内。**(19,2) 本身不动**：那是 `maps.lua` 的 village.spawn
  -- 与 山庄→村的落点格，属于切图契约而不是 NPC 点位。
  village_gate = { map = "village", cell = { x = 19, y = 3 }, label = "云泽村·南市街口" },
  village_market = { map = "village", cell = { x = 14, y = 3 }, label = "云泽村·集市" },
  village_stall = { map = "village", cell = { x = 20, y = 3 }, label = "云泽村·货摊" },
  village_plaza = { map = "village", cell = { x = 20, y = 5 }, label = "云泽村·广场" },
  -- 医馆/药圃从市集 x22..24 搬回药庐门前（M72-11）：横屏方案A 把杂货铺搬进市集 x21..26
  -- &y6..10，而铺面的 solid 当时没跟着重算。solid 按贴图不透明像素真算之后，那两格成了
  -- 铺子内部——阿瑶的"工作地/家"在一堵墙里。现在两格都在药庐 x3..8&y15..19 的门前石板院。
  -- 医馆取 (8,14) 而不是门正对的 (5,14)：后者与 村→药庐 的出口格 (5,13) 紧邻，
  -- 实测把 `jhyt_m18_stats` / `jhyt_m17_dialogue` 压红（`mode = "Transition", want "Dialogue"`）
  -- ——同上一条的同一条新契约，这里是被它当场抓住的那一处我自己搬的格。
  village_clinic = { map = "village", cell = { x = 8, y = 14 }, label = "云泽村·医馆" },
  village_home_west = { map = "village", cell = { x = 12, y = 8 }, label = "云泽村·沈宅" },
  village_home_south = { map = "village", cell = { x = 16, y = 8 }, label = "云泽村·赵宅" },
  village_smithy = { map = "village", cell = { x = 28, y = 8 }, label = "云泽村·铁匠铺" },
  village_teahouse = { map = "village", cell = { x = 32, y = 8 }, label = "云泽村·茶摊" },
  -- 林宅：搬到民居A 门前东侧 (20,18)（M72-11）。旧格 (24,10) 是杂货铺屋顶那格——
  -- 同上一条的同一个原因。这里选它是因为林小满日程的另一站是茶园 (12,13)，
  -- 走 北岸 y18 → 石桥 x24..25 → 中街 是唯一有条理的路径（渡口那块是死胡同）。
  village_home_north = { map = "village", cell = { x = 20, y = 18 }, label = "云泽村·林宅" },
  village_home_forge = { map = "village", cell = { x = 29, y = 12 }, label = "云泽村·韩宅" },
  village_alley = { map = "village", cell = { x = 28, y = 13 }, label = "云泽村·后巷" },
  village_teafield = { map = "village", cell = { x = 12, y = 13 }, label = "云泽村·茶园" },
  village_pond = { map = "village", cell = { x = 20, y = 14 }, label = "云泽村·池畔" },
  village_shrine = { map = "village", cell = { x = 24, y = 19 }, label = "云泽村·土地庙" },
  village_dock = { map = "village", cell = { x = 14, y = 19 }, label = "云泽村·渡口" },

  -- 阶段 2 新增 NPC 住所（docs/54 §4.8，2026-10-06 1.4）：只追加新地点，
  -- 不动既有地点。格子均落在 village/main 主街（y1..14）或 manor 可行走区内。
  village_cattle_shed = { map = "village", cell = { x = 8, y = 12 }, label = "云泽村·牛棚" },
  -- 药圃：与上面 village_clinic 同一次搬迁（理由写在那里，不重复一遍）。
  village_herb_yard = { map = "village", cell = { x = 9, y = 14 }, label = "云泽村·药圃" },
  village_escort_yard = { map = "village", cell = { x = 18, y = 9 }, label = "云泽村·镖局院" },
  village_merchant_hall = { map = "village", cell = { x = 14, y = 5 }, label = "云泽村·商会" },
  village_ghost_alley = { map = "village", cell = { x = 30, y = 14 }, label = "云泽村·暗巷" },
  manor_sword_hall = { map = "manor", cell = { x = 28, y = 14 }, label = "山庄·演武堂" }
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
