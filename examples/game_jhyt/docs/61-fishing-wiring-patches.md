# 钓鱼系统（§4.5 / 阶段 2 M45）接线补丁文本

> 本文件给出 Organizer 集成所需的**共享文件补丁文本**（这些文件我无编辑权，只交 diff）。
> 已落地的自有文件：`scripts/data/fish.lua`、`scripts/data/items_fish.lua`、
> `scripts/systems/fishing.lua`、`engine/crates/game/examples/jhyt_m45_fishing.rs`。
> 验收：`jhyt_m45_fishing_ok`；`systems/fishing.lua` validate_all 空；M42 回归绿。

---

## 1. `scripts/main.lua`（3 处）

### 1a. require（顶部，紧跟 mastery require 之后）
```lua
-- 钓鱼（docs/33 §4.5，2026-10-06 1.1）：4 水域 23 鱼种 + 3 阶钓竿抛竿判定。
local fishing = require("systems.fishing")
```

### 1b. save 注册（register_save_providers() 内，紧跟 mastery 注册之后）
```lua
  -- 钓鱼（docs/33 §4.5，1.1）：钓获统计随存档恢复。
  save_manager.register("fishing", fishing.capture, fishing.restore)
```

### 1c. new_game() 复位（紧跟 `death.reset()` 之后）
```lua
  -- 钓鱼（docs/33 §4.5，1.1）：新档清空钓获统计。
  fishing.reset()
```

### 1d. on_day_start() 挂点（紧跟 `weather.on_day_start()` 之后，纯派生刷新临时钓点）
```lua
  -- 钓鱼（docs/33 §4.5，1.1）：雨季涨水临时钓点随天气/季节重算开放表。
  fishing.on_day_start()
```

---

## 2. `scripts/validate_data.lua`（validators() 清单追加一行）

紧跟 `{ "mastery", require("systems.mastery"), "systems.mastery" }` 之后：
```lua
    -- 1.1 追加："fishing"（data/fish.lua + systems/fishing.lua）：4 水域 23 鱼种、
    -- 钓竿 3 阶、sell 双写一致性校验。守结构不变量（鱼种≥20、难度 1..5、归属水域存在、
    -- data/fish.lua.sell == data/items_fish.lua 同名物品 sell）。
    { "fishing", require("systems.fishing"), "systems.fishing" },
```

---

## 3. `scripts/verify.ps1`（验收样例清单追加一行）

在样例清单（`jhyt_m42_shipping_box` / `jhyt_m43_mastery` 附近）追加：
```powershell
"jhyt_m45_fishing"
```

---

## 4. `docs/33-江湖有田-数值设计案.md`（只交新节草案 + 版本记录行，不改既有数值）

### 4a. 版本记录行（置于 `## 0. 版本记录` 顶部、0.9 行之前）
```
- **1.1（2026-10-06）**：钓鱼落地——新增 §4.5 钓鱼（4 水域：村溪入门/山湖进阶/
  寒泉洞暗河稀有/雨季涨水临时；23 鱼种，difficulty 1..5，sell 10..200 文，覆盖四季；
  钓竿 3 阶竹/精铁/寒江，family=fishing_rod 与锄壶同 tool 槽，energy 8/5/3；竿阶 t
  可钓 difficulty ≤ t*2；晨/昏/夜分钟时段窗口；临时钓点随夏日雨天刷新）；
  新增 `data/fish.lua`（水域/鱼种）+ `data/items_fish.lua`（鱼物品 + 钓竿扩展注册）+
  `systems/fishing.lua`（抛竿判定/体力时间消耗/capture/restore/reset/validate_all），
  `on_day_start` 挂点刷新临时钓点，鱼可入镖局货箱结算，由 `jhyt_m45_fishing` 断言。
```

### 4b. §4.5 新节草案（置于 §4.4 之后；docs/33 当前 §4 农业与生产内）
```
### 4.5 钓鱼（1.1 新增，改造计划 §4.4）

四钓点、23 鱼种，下竿消耗体力与时间（每竿 +10 分钟），渔获入镖局货箱按 sell 结算。

| 钓点 | 定位 | 开放条件 | 鱼种难度 |
| --- | --- | --- | --- |
| 村溪 village_brook | 入门 | 全年、不限天气 | 1–2 |
| 山湖 mountain_lake | 进阶 | 全年、不限天气 | 2–3 |
| 寒泉洞暗河 cave_river | 稀有 | 仅冬季 | 4–5 |
| 雨季涨水潭 flood_pool | 临时 | 仅夏季 + 雨天 | 3–4 |

- 钓竿 3 阶：竹钓竿(tier1,耗体8)/精铁钓竿(tier2,耗体5)/寒江钓竿(tier3,耗体3)；
  family=fishing_rod，与锄/壶/斧/镐**共用同一个 tool 槽**（装备钓竿顶掉锄头，收竿切回）。
- 竿阶门槛：竿阶 t 可钓 difficulty ≤ t×2 的鱼（t=1→≤2、t=2→≤4、t=3→≤5）；
  低阶竿钓高阶鱼直接失败（rod_too_low）。
- 时段窗口：晨≈7:00–12:00、昏≈17:00–22:00、夜≈22:00–24:00（当日分钟区间），
  留空=全天；季节/时段不符的鱼当日出不来。
- 特色种：桃花鱼（春日村溪）、雪莲鱼（冬季暗河极稀有，难度 5）、寒泉银鱼。
- 暂不做：鱼饵/咬线小游戏/水花动画/鱼图（阶段 4 美术管线）；钓竿购买途径（铁铺/
  行商）随商店季节机制里程碑。
```

---

## 5. 可选：鱼进入商店收购（zhao_stall buys）

**已落地、无需此补丁即可卖鱼**：渔获走镖局货箱（shipping.add，已由 M45 ③断言，
shipping.item_def 已加扩展物品回退）。若要让货摊也收鱼，需两步配套（二者必须同提，
否则 `shop.validate_all` 会因 buys 项找不到 sell 价而报错）：

- `systems/shop.lua` 的 `item_def` 追加与 shipping 同款扩展回退
  （`items[id] or tools[id] or ext_item_def(id)`，纯追加、不改既有解析）；
- `data/shops.lua` zhao_stall.buys 数组追加 23 个鱼 id（crucian/carp/.../swangeel）。

---

## 6. 我已直接落地的一处共享回退（请 Organizer 知悉）

`systems/shipping.lua` 的 `item_def` 已加**纯追加**扩展物品回退
（`items[id] or tools[id] or ext_item_def(id)`，遍历与 inventory.lua 同一批可选模块）。
目的：渔获（注册在 `data/items_fish.lua` 扩展，不在 `data.items`）能入镖局货箱。
**既有物品仍优先命中 data.items/data.tools，顺序与数值不变**——M42 回归已绿
（rice/lingzhi/seed_rice/seed_mint/hoe 解析与断言逐字节不变）。
