-- data/animals.lua：《江湖有田》畜牧动物注册表（阶段 2，docs/33 §4.4，2026-10-06 1.0）。
--
-- 武侠化「药园畜养」：山庄畜舍养鸡鸭牛羊驴，每日饲喂/抚摸/放牧攒好感，
-- 吃饱且好感达标即产出畜牧产物（items_animal.lua，kind=animal_product）。
--
-- 字段：
--   id             动物 id（与键一致）
--   name           显示名
--   house          所属畜舍建筑 id（coop 鸡舍 / barn 畜棚 / stable 马厩，
--                  即 data/buildings.lua 的同名建筑；systems/animal.adopt 据此
--                  查 systems.building.list 是否已建、容量是否够）
--   produce        常规产物物品 id（引用 data/items_animal.lua）
--   produce_cycle  产出周期天数（每 cycle 天产一次；1=日产）
--   feed           每日饲料需求 { item, count }（MVP 统一以谷物 rice 饲喂）
--   spirit_produce 可选：好感极高时的特殊产物（如灵蛋）；缺省无特殊产物
local M = {}

M.version = 1

M.animals = {
  -- 鸡舍（coop）：鸡/鸭，日产蛋。
  bai_feng_ji = {
    id = "bai_feng_ji",
    name = "白凤鸡",
    house = "coop",
    produce = "egg",
    produce_cycle = 1,
    feed = { item = "rice", count = 1 },
    spirit_produce = "spirit_egg"
  },
  lu_hua_ya = {
    id = "lu_hua_ya",
    name = "芦花鸭",
    house = "coop",
    produce = "duck_egg",
    produce_cycle = 1,
    feed = { item = "rice", count = 1 }
  },
  -- 畜棚（barn）：羊/猪/牛，周期产物。
  qing_shan_yang = {
    id = "qing_shan_yang",
    name = "青山羊",
    house = "barn",
    produce = "wool",
    produce_cycle = 2,
    feed = { item = "rice", count = 1 }
  },
  hua_zhu = {
    id = "hua_zhu",
    name = "花猪",
    house = "barn",
    produce = "pork",
    produce_cycle = 3,
    feed = { item = "rice", count = 1 }
  },
  lao_huang_niu = {
    id = "lao_huang_niu",
    name = "老黄牛",
    house = "barn",
    produce = "beef",
    produce_cycle = 3,
    feed = { item = "rice", count = 1 }
  },
  -- 马厩（stable）：青驴为坐骑，厚绒可纺，周期最长。
  qing_lv = {
    id = "qing_lv",
    name = "青驴",
    house = "stable",
    produce = "wool",
    produce_cycle = 4,
    feed = { item = "rice", count = 1 }
  }
}

function M.get(id)
  return M.animals[id]
end

function M.ids()
  local ids = {}
  for id in pairs(M.animals) do
    ids[#ids + 1] = id
  end
  table.sort(ids)
  return ids
end

return M
