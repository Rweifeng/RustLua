# 节气历法（阶段3 「节气历法」 / M55）接线补丁文本

> 本文件给出 Organizer 集成所需的**共享文件补丁文本**（这些文件我无编辑权，只交 diff）。
> 已落地的自有文件：`scripts/data/solar_terms.lua`、`scripts/systems/solar_terms.lua`、
> `engine/crates/game/examples/jhyt_m55_solarterm.rs`。
> 验收：`jhyt_m55_solarterm_ok`；data/systems 两侧 validate_all 空；validate_data strict 空。
>
> 设计口径（先读 docs/33 §2.1 时间日历后定）：每季 28 天、四季 112 天循环。传统二十四节气
> 在本游戏里**每季取 6 个**（立春→大寒，共 24 个），按"季内第几天"**单日生效**（不像阶段2
> 节日是 3 天窗口）。节气日刻意压在 8 个节日窗口正中央（春分=春9/茶会8-10、清明=春16/踏青15-17、
> 冬至=冬16/封山15-17、大寒=冬27/守岁26-28……），节气是节日的天文原点，两套时间轴只读对照。

---

## 0. 我已直接落地的两个既有树阻塞修复（请 Organizer 知悉，均未改任何数值/锚点）

1. `scripts/data/items_mine.lua` 末尾 `local items = {...}` 多了一个闭合 `}`（启动即
   `unexpected symbol near '}'`，挡在 main.lua require 链上）。删了一个多余 `}`，数值逐字未动。
2. `scripts/data/items_fish.lua` 合并进主表时**不覆盖既有物品**（防撞名守护）：鱼 id `"peach"`
   （桃花鱼）与果树产物 `items.peach`（桃子，kind=fruit）撞 id，既有物品优先，否则 registry_check
   把果树收获物误判成 kind=fish/sell=30。鱼物品仍保留在 ext.items 里供钓鱼 sell 双写校验。

---

## 1. `scripts/validate_data.lua`（validators() 清单追加一行）

紧跟 `{ "festivals", require("data.festivals"), "data.festivals" }` 之后：
```lua
    -- 2.1 追加："solarterm"（data/solar_terms.lua）：二十四节气注册表。守时间轴自洽——
    -- 24 个节气每季恰 6 个、季内日 1..28 不撞日、修饰键在白名单内、festival_link 指向真实节日。
    { "solarterm", require("data.solar_terms"), "data.solar_terms" },
```
> 注：`systems/solar_terms.lua` 是纯派生查询面（capture/restore/reset 空实现），自身无可校验状态，
> validate_all 恒空，**不登记**进此清单（与 world/festival.lua 同形）。

---

## 2. `scripts/verify.ps1`（验收样例清单追加一行）

在样例清单（`jhyt_m49_festival` 附近）追加：
```powershell
"jhyt_m55_solarterm"
```

---

## 3. `scripts/main.lua`（可选 1 处；纯派生，不接也能跑）

节气是 day 的纯函数、不入档，无需 save provider / 新档 reset。仅建议在 `on_day_start()`
里紧跟 `fishing.on_day_start()` 之后加一行日志/挂点（让节气随跨天显式重算一次，幂等）：
```lua
  -- 节气历法（docs/33 2.1，阶段3）：纯派生，回当日节气 id 供日志/HUD；不落状态。
  pcall(function() require("systems.solar_terms").on_day_start() end)
```
> 修饰消费方接线（把 season_modifiers() 真正喂给 gather/shop）**本阶段不做**——修饰先以
> 数据 + 查询面落地，待后续里程碑再接 systems/gather.lua 与 systems/shop.lua（二者均在保护清单内）。

---

## 4. `docs/33-江湖有田-数值设计案.md`（只交新节草案 + 版本记录行，不改既有数值）

### 4a. 版本记录行（置于 `## 0. 版本记录` 顶部、0.9 行之前）
```
- **2.1（2026-10-06）**：节气历法落地——把传统二十四节气映射到 112 天循环（每季 28 天×四季，
  每季取 6 个、共 24 个，按季内第几天单日生效），并给每个节气配季节修饰
  （gather_mult/crop_bonus/fish_bonus/sell_bonus/energy_bonus，纯查询面）；节气日压在 8 个
  节日窗口正中央（春分↔春分茶会、清明↔清明踏青、冬至↔冬至封山、大寒↔除夕守岁……），
  与节日只读对照不改 festivals.lua；新增 `data/solar_terms.lua`（24 条目 + validate_all）+
  `systems/solar_terms.lua`（today/season_modifiers 纯派生查询，capture/restore/reset 空实现），
  由 `jhyt_m55_solarterm` 断言（注册24/逐日24命中/修饰查询/8节日对应/纯函数/validate_all空）。
```

### 4b. §2.1 新节草案（置于 §2.1 时间与日历之后，或 §2.1 表后补一段）
```
**节气历法的落地口径（阶段3，docs/33 2.1）**：二十四节气在 112 天循环里**每季取 6 个**
（立春→大寒共 24 个），按"季内第几天"**单日生效**（不是节日那样的 3 天窗口）。节气是 `day`
的纯函数（`systems/solar_terms.lua` 的 today(day)），不查随机数、不入存档。24 个节气日刻意落在
8 个阶段2 节日窗口的正中央——春分=春9（茶会8-10）、清明=春16（踏青15-17）、芒种=夏9（端午8-10）、
夏至=夏16（演武15-17）、白露=秋9（中秋8-10）、寒露=秋16（重阳15-17）、冬至=冬16（封山15-17）、
大寒=冬27（守岁26-28）。每个节气带一张修饰表（gather_mult/crop_bonus/fish_bonus/sell_bonus/
energy_bonus），本阶段为**纯查询面**，待后续里程碑接入采集/种植/钓鱼/出货真实消费。
```

---

## 5. 模块查询面（供后续接线 / HUD 用）

- `require('systems.solar_terms').today(day)` → 节气条目 {id,name,season,day_in_season,modifiers,festival_link}，无节气日返回 nil
- `.id(day)` / `.name(day)` → 当天节气 id/中文名（无则 ""）
- `.modifiers(day)` → 当天节气的原始修饰表（无则空表）
- `.season_modifiers(day)` → 聚合查询面（带默认值：mult 类=1、bonus 类=0），消费方直接读
- `.festival_links()` → 24 节气→8 节日的只读对照（升序）
- `.on_day_start()` → 回当日 id（纯派生挂点）；capture/restore/reset 空实现
