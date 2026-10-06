return {
  title = "江湖有田",
  description = "武侠农庄垂直切片初始化数据",
  version = 1,
  plots = {
    { x = 13, y = 6, crop = nil, stage = 0, watered = false },
    { x = 14, y = 6, crop = nil, stage = 0, watered = false }
  },
  plot_radius = 48,
  land = {
    { x0 = 14, x1 = 23, y0 = 4, y1 = 13, type = "medicinal" },
    -- 温室田（docs/33 §4.1.6，2026-10-05 0.7）：山庄西南 6×6，**必须置于 field
    -- 区之前**——land_type 按数组顺序首匹配，重叠区（x28..33,y8..13 原属 field）
    -- 必须优先命中 greenhouse。温室田接受任何土地类型的作物，但四季可种需
    -- 地热药圃建筑（data/buildings.lua greenhouse）已建。
    { x0 = 28, x1 = 33, y0 = 8, y1 = 13, type = "greenhouse" },
    { x0 = 24, x1 = 33, y0 = 4, y1 = 13, type = "field" }
  }}
